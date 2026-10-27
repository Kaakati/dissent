//! Community relay servers. Owners and admins name them in a signed `SetRelays` event. They travel
//! in invites and in the DHT record, and every member adds them to its node's relay map. A joiner
//! has them before its first sync. Each relay's certificate is checked against the public roots
//! when dialled. See `docs/design/06-threat-model.md`.

use std::collections::BTreeSet;
use std::time::Duration;

use dissent_events::{valid_relays, ChannelId, Payload};
use dissent_identity::CommunityId;
use dissent_net::MAX_RELAY_LIST;
use url::Url;

use crate::engine::Engine;
use crate::world::World;
use crate::error::{CoreError, Result};

/// Most relay servers this node keeps, across all communities and the user's own choices. A hostile
/// community cannot fill the relay map beyond this.
const MAX_NODE_RELAYS: usize = MAX_RELAY_LIST;
/// How long a relay from an invite or a community record stays without a community naming it, in ms.
/// Long enough for a join to sync the community's events, which name its relays from then on.
const TRANSIENT_RELAY_MS: u64 = 10 * 60 * 1000;
/// A relay a member hosts for a community, for the relay list. `verified` is true only when the app counts the
/// host, which needs a recent passing check (`boosts::verified_hosts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedRelay {
    pub url: String,
    pub host: String,
    pub verified: bool,
}

/// Longest a join waits for this node to reach a relay before it syncs anyway.
const RELAY_WAIT: Duration = Duration::from_secs(10);

impl Engine {
    /// Sets the relay servers for `community`. Owners and admins only. The new list reaches every
    /// member through the community's events, and is added to this node at once.
    pub async fn set_community_relays(&self, community: CommunityId, relays: Vec<String>) -> Result<()> {
        if !valid_relays(&relays) {
            return Err(CoreError::Invalid(
                "relays must be at most four https URLs".into(),
            ));
        }
        self.refuse_local_relays(&relays)?;
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::SetRelays {
                relays: relays.clone(),
            },
        )
        .await?;
        // The community's relays are now these, so the old ones leave this node and the new ones join.
        self.reconcile_relays(!self.in_call()).await;
        self.spawn_publish_records();
        Ok(())
    }

    /// The relay servers the community currently names.
    pub fn community_relays(&self, community: CommunityId) -> Vec<String> {
        self.read()
            .communities
            .get(&community)
            .map(|c| c.state().relays.clone())
            .unwrap_or_default()
    }

    /// Every relay the communities this device is in currently name, each once, in community order.
    pub fn joined_community_relays(&self) -> Vec<String> {
        let read = self.read();
        let mut out: Vec<String> = Vec::new();
        for community in read.communities.values() {
            for relay in &community.state().relays {
                if !out.contains(relay) {
                    out.push(relay.clone());
                }
            }
        }
        out
    }

    /// The relays members host for the communities this device is in. Each has its host's name, and is marked
    /// verified only when this app counts that host.
    pub fn hosted_relays(&self) -> Vec<HostedRelay> {
        let now = crate::engine::now_ms();
        let w = self.read();
        let mut out = Vec::new();
        for c in w.communities.values() {
            let published = World::published_hosts(c);
            let verified = crate::boosts::verified_hosts(&published, &w.relay_checks, now);
            for (root, relays) in &published {
                for url in relays.iter().filter(|u| dissent_events::valid_relay_url(u)) {
                    out.push(HostedRelay {
                        url: url.clone(),
                        host: w.display_name(root),
                        verified: verified.contains(root),
                    });
                }
            }
        }
        out
    }

    /// Whether this device may dial relay `url`, judged by its text: an https URL with a public host,
    /// or a loopback one on a node that runs on loopback alone. A private or local address would make
    /// every member dial into their own network for whoever published it.
    pub(crate) fn relay_allowed(&self, url: &str) -> bool {
        dissent_events::valid_relay_url(url)
            && (dissent_events::public_relay_host(url)
                || (self.inner.loopback_relays && dissent_events::loopback_relay_host(url)))
    }

    /// Refuses to publish relays this device would not dial. Replay still accepts such a relay from
    /// another app, so members also check before they dial one.
    pub(crate) fn refuse_local_relays(&self, relays: &[String]) -> Result<()> {
        match relays.iter().find(|url| !self.relay_allowed(url)) {
            Some(url) => Err(CoreError::Invalid(format!(
                "{url} is not a public address: a relay must be reachable from the internet"
            ))),
            None => Ok(()),
        }
    }

    /// Adds relays this device needs for a moment: an invite's, or a community record's, before the
    /// community's own events name them. They stay for `TRANSIENT_RELAY_MS`, then go at the next
    /// reconcile unless a community names them. Then reconciles, so a community's `SetRelays` takes
    /// effect at once.
    pub(crate) async fn adopt_relays(&self, urls: &[String]) {
        {
            let now = crate::engine::now_ms();
            let mut w = self.write();
            for url in urls {
                w.relays_transient.insert(url.clone(), now);
            }
        }
        // Dropping a relay could move a call's path, so a call only adds.
        self.reconcile_relays(!self.in_call()).await;
    }

    /// The relays this device should have added to its node, best first: the ones its communities
    /// name, the ones this person hosts, hosted relays that pass their check (`boosts`), and recent
    /// transient ones. Duplicates are dropped, and so are relays it may not dial by their text.
    fn wanted_relays(&self, now: u64) -> Vec<String> {
        let w = self.read();
        let mut named = Vec::new();
        let mut own = Vec::new();
        for c in w.communities.values() {
            let state = c.state();
            named.extend(state.relays.iter().cloned());
            if let Some((_, urls)) = state.relay_hosts.get(&w.me) {
                own.extend(urls.iter().cloned());
            }
        }
        let published: BTreeSet<String> = w
            .communities
            .values()
            .flat_map(|c| World::published_hosts(c).into_values().flatten())
            .collect();
        let passing = w.relay_checks.iter().filter(|(url, ok)| {
            published.contains(*url) && now.saturating_sub(**ok) <= crate::boosts::HOST_GRACE_MS
        });
        let transient = w
            .relays_transient
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) <= TRANSIENT_RELAY_MS);
        let mut out: Vec<String> = Vec::new();
        for url in named
            .into_iter()
            .chain(own)
            .chain(passing.map(|(url, _)| url.clone()))
            .chain(transient.map(|(url, _)| url.clone()))
        {
            if !out.contains(&url) && self.relay_allowed(&url) {
                out.push(url);
            }
        }
        out
    }

    /// Makes the relays this engine added to its node equal to `wanted_relays`, within
    /// `MAX_NODE_RELAYS` with the configured ones counted: it adds the missing ones and, with
    /// `remove`, drops the ones no longer wanted. Relays the node was configured with are never
    /// touched. A name that resolves now to a private address is skipped, and one that does not
    /// resolve now is kept, since iroh resolves it again when it dials.
    pub(crate) async fn reconcile_relays(&self, remove: bool) {
        if !self.inner.community_relays {
            return;
        }
        let Some(node) = self.inner.node.get().cloned() else {
            return;
        };
        let _one_at_a_time = self.inner.relay_reconcile.lock().await;
        let now = crate::engine::now_ms();
        self.write()
            .relays_transient
            .retain(|_, at| now.saturating_sub(*at) <= TRANSIENT_RELAY_MS);
        let wanted = self.wanted_relays(now);
        let adopted: BTreeSet<String> = self.read().relays_adopted.clone();
        let configured: Vec<Url> = node
            .relays()
            .into_iter()
            .filter(|u| !adopted.iter().any(|a| Url::parse(a).ok().as_ref() == Some(u)))
            .collect();

        let mut keep: Vec<(String, Url)> = Vec::new();
        let room = MAX_NODE_RELAYS.saturating_sub(configured.len());
        for s in &wanted {
            if keep.len() >= room {
                break;
            }
            let Ok(url) = Url::parse(s) else {
                continue;
            };
            if configured.contains(&url) || keep.iter().any(|(_, u)| *u == url) {
                continue;
            }
            if !adopted.contains(s)
                && !dissent_events::loopback_relay_host(s)
                && crate::boosts::resolves_privately(&url).await
            {
                tracing::warn!(relay = %url, "a relay resolves to a private network address; not using it");
                continue;
            }
            keep.push((s.clone(), url));
        }

        let gone: Vec<String> = if remove {
            adopted.iter().filter(|a| !keep.iter().any(|(k, _)| k == *a)).cloned().collect()
        } else {
            Vec::new()
        };
        let gone_urls: Vec<Url> = gone.iter().filter_map(|u| Url::parse(u).ok()).collect();
        if !gone_urls.is_empty() {
            node.remove_relays(&gone_urls).await;
        }
        // Without `remove`, adopted relays still count toward the cap.
        let mut held = adopted.len().saturating_sub(gone.len());
        let mut added: Vec<String> = Vec::new();
        let mut add_urls: Vec<Url> = Vec::new();
        for (s, url) in keep {
            if adopted.contains(&s) {
                continue;
            }
            if configured.len() + held >= MAX_NODE_RELAYS {
                break;
            }
            held += 1;
            added.push(s);
            add_urls.push(url);
        }
        if !add_urls.is_empty() {
            node.add_relays(add_urls).await;
        }
        let mut w = self.write();
        for url in &gone {
            w.relays_adopted.remove(url);
        }
        w.relays_adopted.extend(added);
    }

    /// On startup, adds the relays this device should use. See `reconcile_relays`.
    pub(crate) async fn adopt_all_relays(&self) {
        self.reconcile_relays(true).await;
    }

    /// Waits, briefly, until this node is connected to a relay. Returns at once when it has none.
    pub(crate) async fn await_relay(&self) {
        let Some(node) = self.inner.node.get().cloned() else {
            return;
        };
        let _ = tokio::time::timeout(RELAY_WAIT, node.online()).await;
    }
}
