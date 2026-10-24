//! Community relay servers. Owners and admins name them in a signed `SetRelays` event. They travel
//! in invites and in the DHT record, and every member adds them to its node's relay map. A joiner
//! has them before its first sync. Each relay's certificate is checked against the public roots
//! when dialled. See `docs/design/06-threat-model.md`.

use std::time::Duration;

use dissent_events::{valid_relays, ChannelId, Payload};
use dissent_identity::CommunityId;
use url::Url;

use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// Most relay servers this node keeps, across all communities. A hostile community cannot fill
/// the relay map beyond this.
const MAX_NODE_RELAYS: usize = 16;
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
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::SetRelays {
                relays: relays.clone(),
            },
        )
        .await?;
        self.adopt_relays(&relays).await;
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

    /// Adds relay servers to this node's relay map. Malformed entries are skipped, and the node
    /// keeps at most `MAX_NODE_RELAYS` in all.
    pub(crate) async fn adopt_relays(&self, urls: &[String]) {
        let Some(node) = self.inner.node.get().cloned() else {
            return;
        };
        let known = node.relays();
        let mut room = MAX_NODE_RELAYS.saturating_sub(known.len());
        let mut wanted: Vec<Url> = Vec::new();
        for s in urls {
            if room == 0 {
                break;
            }
            let Ok(url) = Url::parse(s) else {
                continue;
            };
            if known.contains(&url) || wanted.contains(&url) || !dissent_events::valid_relay_url(s) {
                continue;
            }
            wanted.push(url);
            room -= 1;
        }
        if !wanted.is_empty() {
            node.add_relays(wanted).await;
        }
    }

    /// On startup, adds the relays of every community this device is in.
    pub(crate) async fn adopt_all_relays(&self) {
        let all: Vec<String> = {
            let w = self.read();
            w.communities
                .values()
                .flat_map(|c| c.state().relays.clone())
                .collect()
        };
        self.adopt_relays(&all).await;
    }

    /// Waits, briefly, until this node is connected to a relay. Returns at once when it has none.
    pub(crate) async fn await_relay(&self) {
        let Some(node) = self.inner.node.get().cloned() else {
            return;
        };
        let _ = tokio::time::timeout(RELAY_WAIT, node.online()).await;
    }
}
