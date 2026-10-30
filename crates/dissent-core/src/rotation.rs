//! Rotating a community's secret when someone is banned. An owner or admin issues a
//! `SecretRotation` once the banned devices are out of the MLS group. The new secret is sealed
//! under the group, so a removed device cannot open it. Members open it from the DAG. A device that
//! cannot open it, such as a storage member, asks an active member for the current secret over the
//! authenticated connection. Every device that learns a newer secret moves its gossip topic to it.
//! When two admins rotate at once, the replay picks one winner (the lowest event id), and every
//! device converges on it. See `docs/design/06-threat-model.md`, finding 2.
//!
//! A person who leaves is covered the same way, once every staying device answers `RotationSupport`
//! with yes. See `docs/design/05-encrypted-servers.md`, "Leaving and the secret".

use std::collections::{BTreeMap, BTreeSet};
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use dissent_events::{ChannelId, CommunityState, Payload, Role};
use dissent_identity::{CommunityId, DeviceId, RootId};
use tracing::{debug, info, warn};

use crate::engine::{now_ms, Engine};
use crate::rpc::{Request, Response};

/// How long a community keeps its old topic running after a secret rotation.
const TOPIC_DRAIN: Duration = Duration::from_secs(10);

/// Whether an admin may rotate the secret when a member leaves, not only when one is banned. A kill switch.
///
/// Clients up to v0.1.11 reject a `SecretRotation` that covers a person who left, and a member on one would be
/// unable to read new messages after it. So with this on, a leave rotation is still issued only once every staying
/// member device has answered `RotationSupport` with yes (`leave_rotation_ready`). Until then a rotation covers only
/// the banned, as in phase 1. See `docs/design/05-encrypted-servers.md`, "Leaving and the secret".
pub(crate) const ROTATE_ON_LEAVE: bool = true;

/// How long a device's yes to `RotationSupport` counts before the device is asked again.
const SUPPORT_FRESH: Duration = Duration::from_secs(10 * 60);
/// How long one device has to answer `RotationSupport`.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// How often a departure's rotation is retried while the removal commit is on its way, and how many times.
/// After that the maintenance pass takes over.
const ROTATE_STEP: Duration = Duration::from_secs(1);
const ROTATE_TRIES: u32 = 10;

/// The devices that said yes to `RotationSupport`, with when, and the communities whose leave rotation is held
/// back, so the reason is logged when the hold starts and not on every pass. In memory only.
#[derive(Default)]
pub(crate) struct LeaveGate {
    yes: BTreeMap<DeviceId, Instant>,
    held: BTreeSet<CommunityId>,
}

impl LeaveGate {
    fn fresh(&self, device: &DeviceId) -> bool {
        self.yes.get(device).is_some_and(|at| at.elapsed() < SUPPORT_FRESH)
    }
}

/// The members a rotation issued now would cover: the banned ones not yet covered, and, with `on_leave`, the
/// departed ones too.
pub(crate) fn pending_rotation(st: &CommunityState, on_leave: bool) -> BTreeSet<RootId> {
    if on_leave {
        return st.rotation_due();
    }
    st.banned.iter().copied().filter(|r| !st.secret_covered.contains(r)).collect()
}

impl Engine {
    /// Removes a departed member's devices from the group, then issues the rotation they are owed. The removal commit
    /// may come from another member, so the rotation is retried for a short while, not left to the next maintenance
    /// pass.
    pub(crate) async fn rotate_after_departure(&self, community: CommunityId) {
        for _ in 0..ROTATE_TRIES {
            self.mls_reconcile(community).await;
            self.rotate_if_due(community).await;
            if !self.rotation_pending(community) {
                return;
            }
            tokio::time::sleep(ROTATE_STEP).await;
        }
    }

    /// Whether some departure or ban is still waiting for a rotation, as this device sees the state. A departure
    /// counts only while every staying device is known to accept its rotation, so a held-back leave rotation is
    /// left to the maintenance pass instead of being retried here.
    fn rotation_pending(&self, community: CommunityId) -> bool {
        let on_leave = ROTATE_ON_LEAVE && self.leave_support_known(community);
        let w = self.read();
        w.communities
            .get(&community)
            .is_some_and(|c| !pending_rotation(c.state(), on_leave).is_empty())
    }

    /// The devices that must accept a leave rotation before one is issued: every device of a member who stays,
    /// storage members and this root's other devices included. Banned and departed members and revoked devices
    /// are left out, since the rotation is what cuts them off.
    fn staying_devices(&self, community: CommunityId) -> Vec<DeviceId> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let st = c.state();
        st.members
            .iter()
            .filter(|(root, m)| !m.left && !st.banned.contains(root))
            .flat_map(|(root, _)| w.directory.devices_of(root))
            .filter(|d| *d != w.device && !st.revoked_devices.contains_key(d))
            .collect()
    }

    /// Whether every staying device said yes to `RotationSupport` lately, from the answers already held. Never asks.
    fn leave_support_known(&self, community: CommunityId) -> bool {
        let devices = self.staying_devices(community);
        let gate = self.inner.leave_gate.lock().unwrap_or_else(PoisonError::into_inner);
        devices.iter().all(|d| gate.fresh(d))
    }

    /// Asks each staying device without a recent yes whether it accepts a leave rotation, and says whether all do.
    /// A device that does not answer, answers no, or cannot decode the question (every release up to v0.1.11)
    /// counts as no: it would reject the rotation and lose access, so the rotation waits. A later pass asks again.
    async fn leave_rotation_ready(&self, community: CommunityId) -> bool {
        for device in self.staying_devices(community) {
            if self.inner.leave_gate.lock().unwrap_or_else(PoisonError::into_inner).fresh(&device) {
                continue;
            }
            let reply = tokio::time::timeout(ASK_WAIT, self.call(device, &Request::RotationSupport)).await;
            let mut gate = self.inner.leave_gate.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(reply, Ok(Ok(Response::RotationSupport(true)))) {
                gate.yes.insert(device, Instant::now());
                continue;
            }
            if gate.held.insert(community) {
                info!(
                    community = %community,
                    device = %device,
                    "not rotating the secret for a departure: a member device has not said it accepts one"
                );
            }
            return false;
        }
        let mut gate = self.inner.leave_gate.lock().unwrap_or_else(PoisonError::into_inner);
        if gate.held.remove(&community) {
            info!(community = %community, "every member device accepts a leave rotation");
        }
        true
    }

    /// Installs a community secret from rotation event `source`, saves it, and moves the
    /// community's gossip topic to it. Returns whether it replaced ours.
    pub(crate) async fn install_secret(
        &self,
        community: CommunityId,
        generation: u64,
        secret: [u8; 32],
        source: [u8; 32],
    ) -> bool {
        let fx = {
            let mut w = self.write();
            let paths = w.paths.clone();
            let Some(keys) = w.access.get_mut(&community) else {
                return false;
            };
            if !keys.install(generation, secret, source, now_ms()) {
                return false;
            }
            let saved = keys.clone();
            if let Err(e) = crate::access::save(&paths, &community, &saved) {
                warn!(community = %community, error = %e, "could not save the community secret");
            }
            w.install_ring(community, saved.history.clone())
        };
        self.perform_effects(fx);
        self.move_topic(community).await;
        true
    }

    /// Joins the topic of the current secret, and keeps the old one running for a drain period.
    /// Messages already sent on the old topic still reach us, and leaving at once would drop them.
    /// Moves run one at a time: each takes the parts the previous move left and subscribes with the
    /// secret current then, so the last move always lands on the current secret's topic.
    async fn move_topic(&self, community: CommunityId) {
        let _one_at_a_time = self.inner.topic_move_lock.lock().await;
        let bootstrap = self.bootstrap_for(&community);
        let (old_sender, old_task) = {
            let mut w = self.write();
            let Some(c) = w.communities.get_mut(&community) else {
                return;
            };
            (c.sender.take(), c.task.take())
        };
        match self.subscribe_topic(community, bootstrap).await {
            Ok(rx) => self.spawn_receiver(community, rx),
            Err(e) => warn!(community = %community, error = %e, "could not join the new topic"),
        }
        if old_task.is_some() {
            let engine = self.clone();
            self.spawn_task(async move {
                tokio::time::sleep(TOPIC_DRAIN).await;
                engine.stop_parts(old_sender, old_task).await;
            });
        }
    }

    /// Whether this device holds something other than the community's current secret. That is a
    /// newer generation, or the same generation from a rival rotation that lost the tie-break. A
    /// secret from an invite has no known source, so it counts as current.
    pub(crate) fn secret_behind(&self, community: CommunityId) -> bool {
        let w = self.read();
        let Some(keys) = w.access.get(&community) else {
            return false;
        };
        let Some(state) = w.communities.get(&community).map(|c| c.state()) else {
            return false;
        };
        let Some(winner) = state.secret_winner else {
            return false;
        };
        let winner = winner.0;
        state.secret_generation > keys.generation
            || (state.secret_generation == keys.generation
                && keys.source != [0; 32]
                && keys.source != winner)
    }

    /// Brings this device up to the community's current secret: opens the winning rotation if it
    /// can, then asks a member for it if it is still behind.
    pub(crate) fn spawn_secret_refresh(&self, community: CommunityId) {
        let engine = self.clone();
        self.spawn_task(async move { engine.refresh_secret(community).await });
    }

    pub(crate) async fn refresh_secret(&self, community: CommunityId) {
        self.adopt_rotations(community).await;
        self.fetch_secret_if_behind(community).await;
        self.fetch_history_if_missing(community).await;
    }

    /// Asks the members' devices for the older secrets this device lacks. A joiner holds only the
    /// secret from its invite, but the events from before a rotation are sealed under older ones.
    async fn fetch_history_if_missing(&self, community: CommunityId) {
        if !self.history_missing(community) {
            return;
        }
        let devices: Vec<DeviceId> = {
            let w = self.read();
            w.communities
                .get(&community)
                .map(|c| w.member_devices(c))
                .unwrap_or_default()
        };
        self.fetch_history_from(community, &devices).await;
    }

    /// Asks `devices` for the older secrets, one at a time, until none is missing. A joiner cannot
    /// name the members yet, because their join events are sealed too, so it asks the invite peers.
    pub(crate) async fn fetch_history_from(&self, community: CommunityId, devices: &[DeviceId]) {
        for device in devices {
            if !self.history_missing(community) {
                return;
            }
            let reply = self.call(*device, &Request::SecretHistory { community }).await;
            if let Ok(Response::SecretHistory(pairs)) = reply {
                self.remember_secrets(community, &pairs).await;
            }
        }
    }

    /// Whether a generation below the current one is absent from this device's history.
    fn history_missing(&self, community: CommunityId) -> bool {
        let w = self.read();
        let Some(keys) = w.access.get(&community) else {
            return false;
        };
        (0..keys.generation).any(|g| !keys.history.iter().any(|(have, _)| *have == g))
    }

    /// Adds older secrets to this device's history, saves them, and rebuilds the replay with them.
    pub(crate) async fn remember_secrets(&self, community: CommunityId, pairs: &[(u64, [u8; 32])]) {
        let fx = {
            let mut w = self.write();
            let paths = w.paths.clone();
            let Some(keys) = w.access.get_mut(&community) else {
                return;
            };
            let before = keys.history.len();
            for (generation, secret) in pairs {
                keys.remember(*generation, *secret);
            }
            if keys.history.len() == before {
                return;
            }
            let saved = keys.clone();
            if let Err(e) = crate::access::save(&paths, &community, &saved) {
                warn!(community = %community, error = %e, "could not save the community secrets");
            }
            w.install_ring(community, saved.history.clone())
        };
        self.perform_effects(fx);
    }

    /// Opens the winning rotation with the group, and installs its secret if it differs from ours.
    async fn adopt_rotations(&self, community: CommunityId) {
        if !self.secret_behind(community) {
            return;
        }
        let (winner, rotations) = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            let Some(winner) = c.state().secret_winner else {
                return;
            };
            (winner.0, c.rotations_in_order())
        };
        let group = community.0;
        for (generation, source, sealed) in rotations {
            if source != winner {
                continue;
            }
            let opened = self.with_mls_ratchet(|mls| mls.decrypt_application(&group, &sealed));
            let Ok((bytes, _)) = opened else {
                continue;
            };
            let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) else {
                continue;
            };
            self.install_secret(community, generation, secret, source).await;
        }
    }

    /// Asks the members' devices for the current secret, until one answers with one we lack.
    async fn fetch_secret_if_behind(&self, community: CommunityId) {
        if !self.secret_behind(community) {
            return;
        }
        let devices: Vec<DeviceId> = {
            let w = self.read();
            w.communities
                .get(&community)
                .map(|c| w.member_devices(c))
                .unwrap_or_default()
        };
        for device in devices {
            let reply = self.call(device, &Request::Secret { community }).await;
            let Ok(Response::Secret(Some((generation, secret, source)))) = reply else {
                continue;
            };
            if self.install_secret(community, generation, secret, source).await {
                return;
            }
        }
    }

    /// Whether `from` is a device of an active member of `community`.
    fn is_active_device(&self, from: DeviceId, community: CommunityId) -> bool {
        let root = self.read().directory.root_of(&from);
        root.is_some_and(|r| self.commit_rules(community).is_active(&r))
    }

    /// Serves the current secret to an active member's device. Anyone else is refused.
    pub(crate) fn serve_secret(&self, from: DeviceId, community: CommunityId) -> Response {
        let active = self.is_active_device(from, community);
        match self.access_keys(community) {
            Some(keys) if active => {
                Response::Secret(Some((keys.generation, keys.secret, keys.source)))
            }
            _ => Response::Error("not an active member".into()),
        }
    }

    /// Serves every secret this device holds to an active member's device. Anyone else is refused,
    /// and so is a banned device, which is no longer active.
    pub(crate) fn serve_secret_history(&self, from: DeviceId, community: CommunityId) -> Response {
        let active = self.is_active_device(from, community);
        match self.access_keys(community) {
            Some(keys) if active => Response::SecretHistory(keys.history),
            _ => Response::Error("not an active member".into()),
        }
    }

    /// Issues a rotation for the bans and departures it does not yet answer, once the removed devices are out of
    /// the group. Only the owner and admins issue one, and storage members never do. A departed member is
    /// covered like a banned one, so one rotation covers every departure since the last.
    pub(crate) async fn rotate_if_due(&self, community: CommunityId) {
        let me = self.inner.me;
        let (due, bans, generation, allowed) = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            let st = c.state();
            let allowed = st.owner == me
                || st.roles.get(&me).is_some_and(|r| r.contains(&Role::Admin));
            (pending_rotation(st, true), pending_rotation(st, false), st.secret_generation + 1, allowed)
        };
        if due.is_empty() || !allowed || self.is_storage_here(community) {
            return;
        }
        // A departure is covered only once every staying device accepts that. Until then the rotation covers the
        // bans alone, which every client accepts, and the departures wait for a later pass.
        let leavers = due.len() > bans.len();
        let pending: Vec<RootId> = if leavers && ROTATE_ON_LEAVE && self.leave_rotation_ready(community).await {
            due.into_iter().collect()
        } else {
            bans.into_iter().collect()
        };
        if pending.is_empty() {
            return;
        }
        let banned_devices: Vec<DeviceId> = {
            let w = self.read();
            pending.iter().flat_map(|r| w.directory.devices_of(r)).collect()
        };
        let members = self
            .with_mls_read(|mls| mls.members(&community.0).ok())
            .unwrap_or_default();
        if members.iter().any(|cert| banned_devices.contains(&cert.device)) {
            // The removal commit has not applied here yet. A later pass issues the rotation.
            return;
        }
        let secret = crate::access::random_secret();
        let group = community.0;
        let sealed = match self.mls_encrypt(&group, &secret) {
            Ok(sealed) => sealed,
            Err(e) => {
                debug!(community = %community, error = %e, "could not seal the new secret");
                return;
            }
        };
        let payload = Payload::SecretRotation {
            generation,
            covers: pending,
            sealed,
        };
        match self
            .author_publish(community, ChannelId::CONTROL, payload)
            .await
        {
            Ok(se) => {
                // The replay may still pick a rival with a lower id. Adoption corrects that later.
                self.install_secret(community, generation, secret, se.event.id.0)
                    .await;
            }
            Err(e) => debug!(community = %community, error = %e, "rotation not accepted"),
        }
    }
}
