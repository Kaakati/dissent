//! Rotating a community's secret when someone is banned. An owner or admin issues a
//! `SecretRotation` once the banned devices are out of the MLS group. The new secret is sealed
//! under the group, so a removed device cannot open it. Members open it from the DAG. A device that
//! cannot open it, such as a storage member, asks an active member for the current secret over the
//! authenticated connection. Every device that learns a newer secret moves its gossip topic to it.
//! When two admins rotate at once, the replay picks one winner (the lowest event id), and every
//! device converges on it. See `docs/design/06-threat-model.md`, finding 2.

use dissent_events::{ChannelId, Payload, Role};
use dissent_identity::{CommunityId, DeviceId, RootId};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::rpc::{Request, Response};

/// How long a community keeps its old topic running after a secret rotation.
const TOPIC_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

impl Engine {
    /// Installs a community secret from rotation event `source`, saves it, and moves the
    /// community's gossip topic to it. Returns whether it replaced ours.
    pub(crate) async fn install_secret(
        &self,
        community: CommunityId,
        generation: u64,
        secret: [u8; 32],
        source: [u8; 32],
    ) -> bool {
        let installed = {
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
            true
        };
        if installed {
            self.move_topic(community).await;
        }
        installed
    }

    /// Joins the topic of the current secret, and keeps the old one running for a drain period.
    /// Messages already sent on the old topic still reach us, and leaving at once would drop them.
    async fn move_topic(&self, community: CommunityId) {
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

    /// Serves the current secret to an active member's device. Anyone else is refused.
    pub(crate) fn serve_secret(&self, from: DeviceId, community: CommunityId) -> Response {
        let active = {
            let root = self.read().directory.root_of(&from);
            root.is_some_and(|r| self.commit_rules(community).is_active(&r))
        };
        match self.access_keys(community) {
            Some(keys) if active => {
                Response::Secret(Some((keys.generation, keys.secret, keys.source)))
            }
            _ => Response::Error("not an active member".into()),
        }
    }

    /// Issues a rotation for bans it does not yet answer, once the banned devices are out of the
    /// group. Only the owner and admins issue one, and storage members never do.
    pub(crate) async fn rotate_if_due(&self, community: CommunityId) {
        let me = self.inner.me;
        let (pending, generation, allowed) = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            let st = c.state();
            let allowed = st.owner == me
                || st.roles.get(&me).is_some_and(|r| r.contains(&Role::Admin));
            let pending: Vec<RootId> = st
                .banned
                .iter()
                .filter(|r| !st.secret_covered.contains(*r))
                .copied()
                .collect();
            (pending, st.secret_generation + 1, allowed)
        };
        if pending.is_empty() || !allowed || self.is_storage_here(community) {
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
        let sealed = match self.with_mls_ratchet(|mls| mls.encrypt(&group, &secret)) {
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
