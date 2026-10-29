//! Server boosts: recognition and higher ceilings for a server whose members host relays for it.
//! Nothing is paid and nothing basic is gated: level 0 is today's app. Each app counts the hosts it
//! can verify itself, so there is no central authority, and two apps may briefly disagree. See
//! `docs/design/12-boosts.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_events::{valid_relays, ChannelId, Payload};
use dissent_identity::{CommunityId, DeviceId, RootId};
use url::Url;
use serde::Serialize;

use crate::community::Community;
use crate::engine::{not_found, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::views::EngineEvent;
use crate::world::World;

/// How long a relay may fail its checks before its host stops counting.
pub const HOST_GRACE_MS: u64 = 72 * 60 * 60 * 1000;

/// The answer to a relay check asked while this member is in a call, which no probe interrupts.
pub const MEMBER_BUSY: &str = "member is busy, try again";

/// How long a probed relay waits before it is probed again. A pass counts for `HOST_GRACE_MS`.
pub const REPROBE_MS: u64 = 60 * 60 * 1000;
/// How long one ping through a relay may take. Kept short: while a probe runs, the relay holds this
/// device's main connection as an inactive duplicate, and packets for it wait on QUIC's retransmission.
pub const PROBE_WAIT: Duration = Duration::from_secs(3);
/// Most member devices one probe tries on a relay before it gives up for this round.
const PROBE_TARGETS: usize = 2;

/// The fewest verified hosts each level needs, from level 0 up.
pub const LEVEL_HOSTS: [usize; 4] = [0, 1, 3, 5];

/// Relay checks one requester may ask this device for, per `RELAY_CHECK_WINDOW`. Each check makes this
/// device dial a relay, so the limit keeps it from being used to flood one.
pub const RELAY_CHECKS_PER_WINDOW: usize = 6;
/// The window `RELAY_CHECKS_PER_WINDOW` counts over.
pub const RELAY_CHECK_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Most requesters tracked at once. The oldest are dropped first, so memory stays bounded.
const MAX_TRACKED_ASKERS: usize = 1024;
/// How many members a host asks for a check before it gives up.
const CHECK_MEMBERS_TRIED: usize = 3;

/// Relay checks this device ran for others, per requester. In memory only.
#[derive(Default)]
pub(crate) struct RelayAsks {
    asks: Mutex<BTreeMap<DeviceId, VecDeque<Instant>>>,
}

impl RelayAsks {
    /// Records a check asked for by `from`, and says whether this device still runs it.
    pub(crate) fn admit(&self, from: DeviceId, now: Instant) -> bool {
        let mut asks = self.asks.lock().unwrap_or_else(PoisonError::into_inner);
        if asks.len() >= MAX_TRACKED_ASKERS {
            asks.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < RELAY_CHECK_WINDOW));
        }
        // The cap is hard: a new requester evicts the one that asked longest ago.
        while asks.len() >= MAX_TRACKED_ASKERS && !asks.contains_key(&from) {
            let oldest = asks.iter().min_by_key(|(_, times)| times.back().copied()).map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    asks.remove(&id);
                }
                None => break,
            }
        }
        let times = asks.entry(from).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= RELAY_CHECK_WINDOW) {
            times.pop_front();
        }
        if times.len() >= RELAY_CHECKS_PER_WINDOW {
            return false;
        }
        times.push_back(now);
        true
    }
}

/// What a level lets a call use. Level 0 is what every server has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ceilings {
    /// The most a voice channel's bitrate may be set to, in kbps.
    pub voice_kbps: u32,
    /// The largest camera frame, and its frame rate.
    pub video: VideoCeiling,
    /// The largest screen-share frame, and its frame rate. Level 0's is the 1080p15 the screen ladder
    /// already had, so a boost never lowers screen sharing.
    pub screen: VideoCeiling,
    /// The most people a call keeps as a mesh before it is forwarded.
    pub mesh_limit: usize,
}

/// The largest frame a sender may encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoCeiling {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

const P720_30: VideoCeiling = VideoCeiling { width: 1280, height: 720, fps: 30 };
const P1080_15: VideoCeiling = VideoCeiling { width: 1920, height: 1080, fps: 15 };
const P1080_30: VideoCeiling = VideoCeiling { width: 1920, height: 1080, fps: 30 };
const P1080_60: VideoCeiling = VideoCeiling { width: 1920, height: 1080, fps: 60 };

/// The ceilings of each level, indexed by level. One table, so the levels and limits stay together.
pub const CEILINGS: [Ceilings; 4] = [
    // Level 0 keeps today's 96 kbps, the most a voice channel could be set to before boosts.
    Ceilings { voice_kbps: 96, video: P720_30, screen: P1080_15, mesh_limit: 6 },
    Ceilings { voice_kbps: 96, video: P1080_30, screen: P1080_30, mesh_limit: 6 },
    Ceilings { voice_kbps: 128, video: P1080_30, screen: P1080_30, mesh_limit: 8 },
    Ceilings { voice_kbps: 256, video: P1080_60, screen: P1080_60, mesh_limit: 10 },
];

/// The level `hosts` verified hosts earn.
pub fn level_for(hosts: usize) -> u8 {
    LEVEL_HOSTS.iter().rposition(|min| hosts >= *min).unwrap_or(0) as u8
}

/// The ceilings of `level`, clamped to the table.
pub fn ceilings(level: u8) -> Ceilings {
    CEILINGS[usize::from(level).min(CEILINGS.len() - 1)]
}

/// The hosts this app verifies: each root with at least one published relay that passed its checks
/// within `HOST_GRACE_MS`. One root counts once, however many relays it hosts. `last_ok` holds the
/// last time each relay URL passed, in ms.
pub fn verified_hosts(published: &BTreeMap<RootId, Vec<String>>, last_ok: &BTreeMap<String, u64>, now_ms: u64) -> BTreeSet<RootId> {
    published
        .iter()
        .filter(|(_, relays)| {
            relays.iter().any(|url| {
                last_ok
                    .get(url)
                    .is_some_and(|ok| now_ms.saturating_sub(*ok) <= HOST_GRACE_MS)
            })
        })
        .map(|(root, _)| *root)
        .collect()
}

/// A community's boost as the views show it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BoostView {
    pub level: u8,
    /// The hosts this app verifies, with the relays of theirs that passed.
    pub hosts: Vec<HostView>,
    /// How many hosts the next level needs, or `None` at the top level.
    pub next_level_hosts: Option<usize>,
    /// The ceilings this level gives: voice kbps, the largest frame, and the mesh limit.
    pub voice_kbps: u32,
    pub video_width: u32,
    pub video_height: u32,
    pub video_fps: u32,
    pub mesh_limit: usize,
    /// Whether members, and not only admins and the owner, may publish the relays they host.
    pub members_can_host: bool,
    /// Whether this person may publish their relays now.
    pub can_publish: bool,
}

/// One verified host of a community.
#[derive(Clone, Debug, Serialize)]
pub struct HostView {
    pub root_id: String,
    pub display_name: String,
    pub relays: Vec<String>,
}

impl World {
    /// The relays each current member who is a person says they host for `c`.
    pub(crate) fn published_hosts(c: &Community) -> BTreeMap<RootId, Vec<String>> {
        let state = c.state();
        state
            .relay_hosts
            .iter()
            .filter(|(root, (_, relays))| {
                !relays.is_empty()
                    && !state.banned.contains(root)
                    && state.members.get(root).is_some_and(|m| !m.left && !m.storage)
            })
            .map(|(root, (_, relays))| (*root, relays.clone()))
            .collect()
    }

    /// The hosts of `c` this app verifies now. See [`verified_hosts`].
    pub(crate) fn boost_hosts(&self, c: &Community, now_ms: u64) -> BTreeSet<RootId> {
        verified_hosts(&Self::published_hosts(c), &self.relay_checks, now_ms)
    }

    /// The boost level of `c`, as this app verifies it.
    pub(crate) fn boost_level(&self, c: &Community, now_ms: u64) -> u8 {
        level_for(self.boost_hosts(c, now_ms).len())
    }

    pub(crate) fn boost_view(&self, c: &Community, now_ms: u64) -> BoostView {
        let published = Self::published_hosts(c);
        let hosts = verified_hosts(&published, &self.relay_checks, now_ms);
        let level = level_for(hosts.len());
        let ceil = ceilings(level);
        let fresh = |url: &String| self.relay_checks.get(url).is_some_and(|ok| now_ms.saturating_sub(*ok) <= HOST_GRACE_MS);
        BoostView {
            level,
            hosts: hosts
                .iter()
                .map(|root| HostView {
                    root_id: root.to_string(),
                    display_name: self.display_name(root),
                    relays: published[root].iter().filter(|u| fresh(u)).cloned().collect(),
                })
                .collect(),
            next_level_hosts: LEVEL_HOSTS.get(usize::from(level) + 1).copied(),
            voice_kbps: ceil.voice_kbps,
            video_width: ceil.video.width,
            video_height: ceil.video.height,
            video_fps: ceil.video.fps,
            mesh_limit: ceil.mesh_limit,
            members_can_host: c.state().members_host_relays,
            can_publish: c.state().members_host_relays || matches!(c.role_of(&self.me), "owner" | "admin"),
        }
    }

    /// The communities this app verifies `root` as a host of: shared ones only.
    pub(crate) fn hosted_by(&self, root: &RootId, now_ms: u64) -> Vec<crate::views::CommunityRef> {
        self.communities
            .values()
            .filter(|c| self.boost_hosts(c, now_ms).contains(root))
            .map(|c| crate::views::CommunityRef {
                id: c.id().to_string(),
                name: c.name().to_string(),
            })
            .collect()
    }
}

impl Engine {
    /// Publishes the relays this person hosts for `community`: at most four https URLs. An empty list
    /// withdraws. Each member's app verifies them before counting this person as a host.
    pub async fn set_hosted_relays(&self, community: CommunityId, relays: Vec<String>) -> Result<()> {
        if !valid_relays(&relays) {
            return Err(CoreError::Invalid("relays must be at most four https URLs".into()));
        }
        self.refuse_local_relays(&relays)?;
        self.author_publish(community, ChannelId::CONTROL, Payload::RelayHostSet { relays: relays.clone() })
            .await?;
        // The host's own app uses its relays, so members can reach the host through them to check them.
        self.reconcile_relays(!self.in_call()).await;
        self.emit(EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

    /// Lets members, and not only admins and the owner, publish the relays they host for
    /// `community`. Owners and admins only, which replay enforces. Off by default.
    pub async fn set_members_host_relays(&self, community: CommunityId, on: bool) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::MembersHostRelays { on })
            .await?;
        self.emit(EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

    /// Pings `target` through `url` alone (`Node::ping_via_relay`) and returns the round trip in ms, or
    /// why it failed. Records nothing. `RelayCheck` answers a host with it, pinging the host itself.
    /// During a call it runs no probe and answers `MEMBER_BUSY`.
    pub async fn probe_relay(&self, url: &str, target: DeviceId) -> std::result::Result<u64, String> {
        if !valid_relays(&[url.to_string()]) {
            return Err("not an https relay address".into());
        }
        if self.in_call() {
            return Err(MEMBER_BUSY.into());
        }
        let relay = Url::parse(url).map_err(|e| e.to_string())?;
        let node = self.inner.node.get().cloned().ok_or("this device has no network yet")?;
        node.ping_via_relay(&relay, target, PROBE_WAIT)
            .await
            .map_err(|e| format!("could not reach you through this relay: {e}"))
    }

    /// The boost of `community`, as this app verifies it.
    pub fn boost(&self, community: CommunityId) -> Result<BoostView> {
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
        Ok(w.boost_view(c, now_ms()))
    }

    /// The ceilings `community`'s level gives this app's calls. Level 0's outside a community.
    pub(crate) fn call_ceilings(&self, community: Option<CommunityId>) -> Ceilings {
        let w = self.read();
        let level = community
            .and_then(|id| w.communities.get(&id))
            .map_or(0, |c| w.boost_level(c, now_ms()));
        let forced = self.inner.boost_level_override.load(std::sync::atomic::Ordering::Relaxed);
        if community.is_some() && forced != u8::MAX {
            return ceilings(forced);
        }
        ceilings(level)
    }

    /// Probes one relay a member hosts for one of our communities, in turn, on the maintenance tick. The
    /// relay passes when a ping to a member device on it completes through that relay alone
    /// (`Node::ping_via_relay`): the relay admitted this member's key and carried the packets. A pass
    /// is recorded with the time, and a host whose relays have not passed for longer than
    /// `HOST_GRACE_MS` stops counting. A relay with no member device known to be on it is not probed.
    pub(crate) async fn check_relay_hosts(&self) {
        self.forget_unpublished_relays();
        // With "Use relays offered by my servers" off, this device dials no server relay, so it
        // probes none and counts no host.
        if !self.inner.community_relays {
            return;
        }
        // A probe holds this device's key on the relay for a moment, and dropping a relay could move a
        // call's path. Never during a call: the tick tries again later.
        if self.in_call() {
            return;
        }
        // Relays that stopped counting, or that no community names any more, leave the node.
        self.reconcile_relays(true).await;
        let Some((url, targets)) = self.next_relay_probe(now_ms()) else {
            return;
        };
        let Some(node) = self.inner.node.get().cloned() else {
            return;
        };
        let Ok(relay) = Url::parse(&url) else {
            return;
        };
        // The probe runs on its own short-lived endpoint pinned to this relay, so nothing is added to
        // this node until the relay passes. Even this node's home relay is probed: a net report
        // measures a relay without being admitted by it, so only a ping that went through it proves
        // the relay carries this member's traffic.
        let mut passed = false;
        match self.relay_address_ok(&url).await {
            Ok(()) => {
                for device in targets.into_iter().take(PROBE_TARGETS) {
                    if passed || self.in_call() {
                        break;
                    }
                    match node.ping_via_relay(&relay, device, PROBE_WAIT).await {
                        Ok(_) => passed = true,
                        Err(e) => tracing::debug!(relay = %url, device = %device, error = %e, "relay probe failed"),
                    }
                }
            }
            Err(why) => tracing::debug!(relay = %url, reason = %why, "hosted relay not probed"),
        }
        {
            let mut w = self.write();
            if passed {
                w.relay_checks.insert(url.clone(), now_ms());
            }
            w.relay_probed.insert(url.clone(), now_ms());
        }
        if passed {
            // A passing relay is now wanted, so the reconcile adds it.
            self.reconcile_relays(false).await;
        }
    }

    /// Whether this member may connect to hosted relay `url` now: its text passes `relay_allowed`, and
    /// every address its name resolves to is public. A loopback relay on a loopback-only node passes.
    pub(crate) async fn relay_address_ok(&self, url: &str) -> std::result::Result<(), String> {
        if !self.relay_allowed(url) {
            return Err("this relay's address is on a private network, so it was not checked".into());
        }
        if self.inner.loopback_relays && dissent_events::loopback_relay_host(url) {
            return Ok(());
        }
        public_address(url).await
    }

    /// Every hosted relay a current host publishes in one of this device's communities.
    fn published_relay_urls(&self) -> BTreeSet<String> {
        let w = self.read();
        w.communities
            .values()
            .flat_map(|c| World::published_hosts(c).into_values().flatten())
            .collect()
    }

    /// Forgets the checks and probe times of relays no current host publishes any more, so the maps
    /// hold only relays in some community's current `RelayHostSet`s.
    fn forget_unpublished_relays(&self) {
        let published = self.published_relay_urls();
        let mut w = self.write();
        w.relay_checks.retain(|url, _| published.contains(url));
        w.relay_probed.retain(|url, _| published.contains(url));
    }

    /// The one relay to probe this tick, and the member devices known to sit on it, its host's first.
    /// Relays are taken in turn: one never probed first, then the one probed longest ago, skipping any
    /// probed within `REPROBE_MS`.
    fn next_relay_probe(&self, now: u64) -> Option<(String, Vec<DeviceId>)> {
        let w = self.read();
        let mut relays: BTreeMap<String, Vec<DeviceId>> = BTreeMap::new();
        let peers = w.addresses.peers();
        for c in w.communities.values() {
            for (host, urls) in World::published_hosts(c) {
                for url in urls {
                    let Ok(parsed) = Url::parse(&url) else {
                        continue;
                    };
                    let on_relay = |device: &DeviceId| peers.iter().any(|p| p.id == *device && p.relay.as_ref() == Some(&parsed));
                    let entry = relays.entry(url).or_default();
                    // The host's own devices first: a host's app usually picks its own relay as home.
                    for device in w.directory.devices_of(&host) {
                        if device != w.device && on_relay(&device) && !entry.contains(&device) {
                            entry.push(device);
                        }
                    }
                    for device in w.member_devices(c) {
                        if on_relay(&device) && !entry.contains(&device) {
                            entry.push(device);
                        }
                    }
                }
            }
        }
        relays
            .into_iter()
            .filter(|(url, _)| w.relay_probed.get(url).is_none_or(|at| now.saturating_sub(*at) >= REPROBE_MS))
            .min_by_key(|(url, _)| w.relay_probed.get(url).copied().unwrap_or(0))
    }

    /// Records that `url` passed its check at `at_ms`. For tests, which run without relays.
    pub fn record_relay_check(&self, url: &str, at_ms: u64) {
        self.write().relay_checks.insert(url.to_string(), at_ms);
    }

    /// Answers a `RelayCheck` from `from`. It runs only for a current, non-storage member of `community`
    /// who uses server relays, and only for a relay that member publishes for this community. It also
    /// runs within the per-requester limit, and only for a public address. It records nothing, so it
    /// never counts toward a level.
    pub(crate) async fn answer_relay_check(&self, from: DeviceId, community: CommunityId, url: String) -> Response {
        let published: Vec<String> = {
            let w = self.read();
            // An unknown community, an unknown device and a non-member get the same refusal.
            let refused = || Response::Error(crate::request_gates::REFUSED.into());
            let Some(c) = w.communities.get(&community) else {
                return refused();
            };
            let Some(root) = w.directory.root_of(&from) else {
                return refused();
            };
            let member = c.is_member(&root) && !c.state().members.get(&root).is_some_and(|m| m.storage);
            if !member {
                return refused();
            }
            c.state()
                .relay_hosts
                .get(&root)
                .map(|(_, urls)| urls.clone())
                .unwrap_or_default()
        };
        if let Some(reason) = check_refusal(self.inner.community_relays, &published, &url) {
            return Response::Error(reason.into());
        }
        // The limit comes before the busy answer, so asking again and again cannot track this device's calls.
        if !self.inner.relay_asks.admit(from, Instant::now()) {
            return Response::Error("too many relay checks from this device; try again later".into());
        }
        // A call is busy: a probe now could cause an audio blip. The same rule holds for the tick.
        if self.in_call() {
            return Response::RelayCheck(Err(MEMBER_BUSY.into()));
        }
        if let Err(reason) = public_address(&url).await {
            return Response::Error(reason);
        }
        // The responder pings the asker through the asker's relay. It passes only if the relay admits this
        // device and carries the packets to the asker, and it adds nothing to this node's relay list.
        Response::RelayCheck(self.probe_relay(&url, from).await)
    }

    /// Asks members of `community` to run the relay check on `url`, from their own devices. Tries up to
    /// `CHECK_MEMBERS_TRIED` of them and returns the first pass, as the latency in milliseconds. A
    /// failure gives the last reason. The answer is feedback for the host only: it never counts toward a
    /// level, and nothing waits on it.
    pub async fn relay_check(&self, community: CommunityId, url: &str) -> std::result::Result<u64, String> {
        let devices: Vec<DeviceId> = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or("unknown community")?;
            w.member_devices(c).into_iter().take(CHECK_MEMBERS_TRIED).collect()
        };
        if devices.is_empty() {
            return Err("no other member to run the check".into());
        }
        let mut last = String::from("no member answered");
        for device in devices {
            let req = Request::RelayCheck {
                community,
                url: url.to_string(),
            };
            match self.call(device, &req).await {
                Ok(Response::RelayCheck(Ok(ms))) => return Ok(ms),
                // A busy member is not a failure of the relay: the next member is asked.
                Ok(Response::RelayCheck(Err(reason))) if reason == MEMBER_BUSY => {
                    last = "try again later".into();
                }
                Ok(Response::RelayCheck(Err(reason))) => last = reason,
                Ok(Response::Error(reason)) => last = reason,
                Ok(_) => last = "the member answered oddly".into(),
                Err(e) => last = format!("a member could not be reached: {e}"),
            }
        }
        Err(last)
    }
}

/// The reason this member refuses a relay check, or `None` when it may run it. A member who does not use
/// server relays runs none, since a check connects to the address and reveals the member's IP. A member
/// runs a check only for a relay that the asker publishes for this community.
pub(crate) fn check_refusal(uses_server_relays: bool, published: &[String], url: &str) -> Option<&'static str> {
    if !uses_server_relays {
        return Some(NO_SERVER_RELAYS);
    }
    let wanted = Url::parse(url).ok();
    let is_published = published.iter().any(|p| Url::parse(p).ok() == wanted && wanted.is_some());
    if !is_published {
        return Some(NOT_PUBLISHED);
    }
    None
}

/// The reason a member refused a check, for a member who does not use server relays.
pub const NO_SERVER_RELAYS: &str = "this member doesn't use server relays";
/// The reason a member refused a check, for a relay the asker does not publish.
pub const NOT_PUBLISHED: &str = "that relay is not one this member publishes for this server";

pub(crate) use dissent_events::relay::is_public_address;

/// Whether `url`'s host name resolves now to any address that is not public. A name that does not
/// resolve, or an IP literal, gives false: the literal is judged by `public_relay_host` instead.
pub(crate) async fn resolves_privately(url: &Url) -> bool {
    let (Some(url::Host::Domain(name)), Some(port)) = (url.host(), url.port_or_known_default()) else {
        return false;
    };
    match tokio::time::timeout(PROBE_WAIT, tokio::net::lookup_host((name, port))).await {
        Ok(Ok(addrs)) => addrs.into_iter().any(|a| !is_public_address(a.ip())),
        _ => false,
    }
}

/// Checks that a relay address leads to public addresses only, before this member connects to it. The
/// address must be https. Every address its host name resolves to must be public, since one private
/// record is enough to reach a member's network. The resolution is checked here, and the connection
/// resolves again, so a name that changes between the two can still point elsewhere.
pub(crate) async fn public_address(url: &str) -> std::result::Result<(), String> {
    use std::net::{IpAddr, SocketAddr};
    use url::Host;
    let parsed = Url::parse(url).map_err(|_| "not a valid relay address".to_string())?;
    if parsed.scheme() != "https" {
        return Err("only https relays can be checked".into());
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = match parsed.host() {
        Some(Host::Ipv4(v4)) => vec![IpAddr::V4(v4)],
        Some(Host::Ipv6(v6)) => vec![IpAddr::V6(v6)],
        Some(Host::Domain(name)) => tokio::net::lookup_host((name, port))
            .await
            .map_err(|_| "the relay's address did not resolve".to_string())?
            .map(|a: SocketAddr| a.ip())
            .collect(),
        None => return Err("the relay address has no host".into()),
    };
    if addrs.is_empty() {
        return Err("the relay's address did not resolve".into());
    }
    if addrs.iter().all(|ip| is_public_address(*ip)) {
        Ok(())
    } else {
        Err("this relay's address is on a private network, so it was not checked".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(n: u8) -> RootId {
        RootId([n; 32])
    }

    #[test]
    fn levels_follow_the_host_thresholds() {
        let levels: Vec<u8> = (0..8).map(level_for).collect();
        assert_eq!(levels, vec![0, 1, 1, 2, 2, 3, 3, 3]);
    }

    #[test]
    fn ceilings_rise_with_the_level_and_level_zero_is_todays_app() {
        assert_eq!(ceilings(0).voice_kbps, 96, "level 0 never lowers what a channel could use before");
        assert_eq!(ceilings(0).video, P720_30);
        assert_eq!(ceilings(0).mesh_limit, 6);
        assert_eq!(
            [1, 2, 3].map(|l| (ceilings(l).voice_kbps, ceilings(l).video.height, ceilings(l).video.fps, ceilings(l).mesh_limit)),
            [(96, 1080, 30, 6), (128, 1080, 30, 8), (256, 1080, 60, 10)]
        );
        assert_eq!(ceilings(9), ceilings(3), "a level past the table is the top level");
        for pair in CEILINGS.windows(2) {
            assert!(pair[1].voice_kbps >= pair[0].voice_kbps && pair[1].mesh_limit >= pair[0].mesh_limit);
        }
    }

    #[test]
    fn a_root_counts_once_however_many_relays_it_hosts() {
        let now = 10 * HOST_GRACE_MS;
        let published = BTreeMap::from([
            (root(1), vec!["https://a.example/".into(), "https://b.example/".into(), "https://c.example/".into()]),
            (root(2), vec!["https://d.example/".into()]),
            (root(3), vec!["https://never-checked.example/".into()]),
        ]);
        let last_ok = BTreeMap::from([
            ("https://a.example/".to_string(), now),
            ("https://b.example/".to_string(), now),
            ("https://c.example/".to_string(), now),
            ("https://d.example/".to_string(), now - 1000),
        ]);
        let hosts = verified_hosts(&published, &last_ok, now);
        assert_eq!(hosts, BTreeSet::from([root(1), root(2)]));
        assert_eq!(level_for(hosts.len()), 1);
    }

    #[test]
    fn a_relay_that_fails_for_more_than_72_hours_stops_counting() {
        let checked = 1_000_000;
        let published = BTreeMap::from([(root(1), vec!["https://a.example/".to_string()])]);
        let last_ok = BTreeMap::from([("https://a.example/".to_string(), checked)]);
        assert_eq!(verified_hosts(&published, &last_ok, checked + HOST_GRACE_MS).len(), 1, "still within 72 hours");
        assert!(verified_hosts(&published, &last_ok, checked + HOST_GRACE_MS + 1).is_empty());
    }

    #[test]
    fn relay_checks_per_requester_are_limited_and_windowed() {
        let asks = RelayAsks::default();
        let from = DeviceId([7; 32]);
        let other = DeviceId([8; 32]);
        let start = Instant::now();
        for _ in 0..RELAY_CHECKS_PER_WINDOW {
            assert!(asks.admit(from, start));
        }
        assert!(!asks.admit(from, start), "one more is refused inside the window");
        assert!(asks.admit(other, start), "another requester has its own allowance");
        let later = start + RELAY_CHECK_WINDOW;
        assert!(asks.admit(from, later), "the window passes, and the allowance comes back");
    }

    #[test]
    fn only_public_addresses_are_checked() {
        use std::net::IpAddr;
        for refused in [
            "127.0.0.1", "10.1.2.3", "172.16.0.9", "192.168.1.1", "169.254.7.7", "100.64.0.1",
            "100.127.255.255", "0.0.0.0", "224.0.0.1", "192.0.2.1", "::1", "::", "fc00::1", "fe80::1",
            "2001:db8::1", "::ffff:10.0.0.1", "ff02::1",
        ] {
            let ip: IpAddr = refused.parse().unwrap();
            assert!(!is_public_address(ip), "{refused} should be refused");
        }
        for allowed in ["8.8.8.8", "1.1.1.1", "100.128.0.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            let ip: IpAddr = allowed.parse().unwrap();
            assert!(is_public_address(ip), "{allowed} should be allowed");
        }
    }

    #[test]
    fn a_check_is_refused_unless_the_member_uses_server_relays_and_publishes_the_relay() {
        let published = vec!["https://relay.example.org".to_string()];
        assert_eq!(
            check_refusal(false, &published, "https://relay.example.org"),
            Some(NO_SERVER_RELAYS)
        );
        assert_eq!(
            check_refusal(true, &published, "https://other.example.org"),
            Some(NOT_PUBLISHED)
        );
        assert_eq!(check_refusal(true, &published, "https://relay.example.org"), None);
        assert_eq!(check_refusal(true, &[], "https://relay.example.org"), Some(NOT_PUBLISHED));
    }

    #[tokio::test]
    async fn a_check_only_dials_a_public_https_address() {
        assert!(public_address("https://8.8.8.8").await.is_ok());
        for refused in ["https://127.0.0.1", "https://10.0.0.5", "https://100.64.0.1", "https://[::1]", "https://169.254.1.1"] {
            assert!(public_address(refused).await.is_err(), "{refused} should be refused");
        }
        assert!(public_address("http://8.8.8.8").await.is_err(), "plain http is refused");
    }

    #[test]
    fn the_relay_check_table_never_exceeds_its_cap() {
        let asks = RelayAsks::default();
        let now = Instant::now();
        for n in 0..(MAX_TRACKED_ASKERS as u32 + 200) {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&n.to_le_bytes());
            asks.admit(DeviceId(id), now);
        }
        let len = asks.asks.lock().unwrap().len();
        assert!(len <= MAX_TRACKED_ASKERS, "{len} requesters tracked");
    }
}
