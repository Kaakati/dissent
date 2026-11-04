//! Adding joiners to a community's MLS group, and removing devices of banned or departed members.
//! Any member may do either, within the rules in the parent module. A device waits its turn first,
//! so the highest-ranked online member usually acts, and a second attempt finds nothing left to do.

use std::collections::BTreeSet;

use dissent_identity::{CommunityId, DeviceId};
use dissent_net::PeerAddr;

use crate::engine::Engine;
use crate::membership::COMMIT_STEP;
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::sealed::mls_err;

/// How long a send waits for a banned device to leave the group, in polls: past the longest turn (`COMMIT_DELAY_CAP`).
const SEALABLE_TRIES: u32 = 200;
const SEALABLE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

impl Engine {
    /// Answers a joiner's `MlsJoin`. Adds the joiner's device to the group and returns the
    /// welcome. Refuses unless the joiner is a current member.
    pub(crate) async fn handle_mls_join(
        &self,
        conn_from: DeviceId,
        community: CommunityId,
        from: PeerAddr,
    ) -> Response {
        self.learn_peer(conn_from, from);
        if !self.is_current_member_device(conn_from, community) {
            return Response::Error("not a member of this community".into());
        }
        self.wait_turn(community).await;
        if self.in_group(conn_from, community) {
            return Response::Error("already in the group".into());
        }
        let key_package = match self
            .call(conn_from, &Request::KeyPackages { community })
            .await
        {
            Ok(Response::KeyPackages(mut packages)) if !packages.is_empty() => packages.remove(0),
            _ => return Response::Error("no key package from the joiner".into()),
        };
        let commit = match self.with_mls(|mls| mls.add_members(&community.0, &[key_package])) {
            Ok(commit) => commit,
            Err(e) => return Response::Error(format!("could not add: {e}")),
        };
        self.note_epoch(community, &[conn_from]);
        // The joiner gets the history keys it may have now, so it need not wait for its own fetch.
        self.push_history_keys(community, conn_from);
        self.publish_commit(community, commit.commit).await;
        self.refresh_history_ready(community).await;
        // Check the new member's devices for history messages before the welcome goes out, so the joiner's first
        // message from us is in the new form, with no gap while the check runs.
        Response::Welcome(commit.welcome)
    }

    /// Whether `device` belongs to a current, non-banned member of `community`.
    fn is_current_member_device(&self, device: DeviceId, community: CommunityId) -> bool {
        let rules = self.commit_rules(community);
        let root = self.read().directory.root_of(&device);
        root.is_some_and(|root| rules.is_person(&root))
    }

    /// Whether `device` already has a leaf in the group.
    fn in_group(&self, device: DeviceId, community: CommunityId) -> bool {
        self.with_mls_read(|mls| {
            mls.members(&community.0)
                .map(|certs| certs.iter().any(|c| c.device == device))
                .unwrap_or(false)
        })
    }

    /// Sleeps until this device's turn to act on a membership change.
    async fn wait_turn(&self, community: CommunityId) {
        let delay = self.commit_delay(community);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    /// Joins the group from a welcome, if every member it lists is a current member in our view.
    pub(crate) fn accept_welcome(&self, community: CommunityId, welcome: &[u8]) -> Result<()> {
        let rules = self.commit_rules(community);
        self.with_mls(|mls| {
            mls.join_from_welcome_with_policy(welcome, |info| {
                let ok = rules.allows_welcome(info);
                if !ok {
                    tracing::warn!(
                        community = %community,
                        "refused a welcome that lists a device outside the community"
                    );
                }
                ok
            })
        })
        .map(|_| ())
        .map_err(mls_err)?;
        self.note_epoch(community, &[]);
        Ok(())
    }

    /// Asks `peer` to add this device to the community's group, then joins from the welcome.
    pub(crate) async fn request_mls_join(
        &self,
        community: CommunityId,
        peer: DeviceId,
        from: PeerAddr,
    ) -> Result<()> {
        match self
            .call(peer, &Request::MlsJoin { community, from })
            .await?
        {
            Response::Welcome(welcome) => self.accept_welcome(community, &welcome),
            Response::Error(reason) => Err(CoreError::Invalid(format!("join refused: {reason}"))),
            other => Err(CoreError::Invalid(format!("unexpected reply: {other:?}"))),
        }
    }

    /// Joins the community's group through the first of `devices` that adds us. Returns whether
    /// one did. Used when joining, and by maintenance for a member that still has no group.
    pub(crate) async fn mls_join_via(&self, community: CommunityId, devices: &[DeviceId]) -> bool {
        let Some(from) = self.read().my_addr.clone() else {
            return false;
        };
        for device in devices {
            match self
                .request_mls_join(community, *device, from.clone())
                .await
            {
                Ok(()) => {
                    self.unseal_pending(community);
                    // Keys and the name arrive in the background, so the join does not wait on members.
                    let engine = self.clone();
                    self.spawn_task(async move {
                        engine.fetch_epoch_history_until_answered(community).await;
                        engine.fetch_history_keys_until_answered(community).await;
                        engine.restore_join_name(community).await;
                    });
                    return true;
                }
                Err(e) => {
                    tracing::debug!(community = %community, peer = %device, error = %e, "MLS join not granted")
                }
            }
        }
        false
    }

    /// Joins the group of every community where this device is a member but has no group yet.
    pub(crate) async fn mls_join_missing(&self, community: CommunityId) {
        if self.is_storage_here(community) {
            return;
        }
        if self.mls_epoch(community).is_some() {
            return;
        }
        let devices: Vec<DeviceId> = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            if !c.is_member(&self.inner.me) {
                return;
            }
            w.member_devices(c)
        };
        self.mls_join_via(community, &devices).await;
    }

    /// Removes the devices of banned or departed members from the group, once this device's turn
    /// comes. A device that is already gone when the turn comes is left alone.
    pub(crate) async fn mls_reconcile(&self, community: CommunityId) {
        if self.is_storage_here(community) {
            return;
        }
        if self.departed_devices(community).is_empty() {
            return;
        }
        self.wait_turn(community).await;
        // A commit that added someone since banned is usually followed at once by the removal its author or another
        // device made. Whoever took the add waits one step for that removal, the owner too, rather than make a second
        // removal at the same epoch, which would fork the group.
        let since = self.inner.banned_adds.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&community).copied();
        if let Some(wait) = since.and_then(|at| COMMIT_STEP.checked_sub(at.elapsed())) {
            tokio::time::sleep(wait).await;
        }
        let departed = self.departed_devices(community);
        if departed.is_empty() {
            return;
        }
        match self.with_mls(|mls| mls.remove_members(&community.0, &departed)) {
            Ok(commit) => {
                self.note_epoch(community, &[]);
                self.publish_commit(community, commit).await
            }
            Err(e) => {
                tracing::warn!(community = %community, error = %e, "could not remove departed devices")
            }
        }
    }

    /// Whether a banned person's device is in the group now. It holds this epoch's keys, so nothing new is sealed under
    /// them until it is removed.
    pub(crate) fn banned_device_in_group(&self, community: CommunityId) -> bool {
        let rules = self.commit_rules(community);
        // The guard keeps this device from sealing to someone banned. If the one banned is this device's own person, there
        // is nothing to keep from them, and what it sends is refused by replay as before, with that reason and at once.
        if rules.is_banned_person(&self.inner.me) {
            return false;
        }
        let members = self.with_mls_read(|mls| mls.members(&community.0).ok()).unwrap_or_default();
        let w = self.read();
        members
            .iter()
            .any(|cert| rules.is_banned_person(&cert.root) || w.directory.root_of(&cert.device).is_some_and(|root| rules.is_banned_person(&root)))
    }

    /// Holds a send while a banned device is in the group, and removes it in this device's turn if nobody else has, so a
    /// message sent right after a ban goes out under the next epoch. If it is still there after that, `seal` refuses, the
    /// send fails with that reason, and the composer gets its text back.
    pub(crate) async fn wait_until_sealable(&self, community: CommunityId) {
        self.wait_for_pending_adds(community).await;
        if !self.banned_device_in_group(community) {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move { engine.mls_reconcile(community).await });
        for _ in 0..SEALABLE_TRIES {
            if !self.banned_device_in_group(community) {
                return;
            }
            tokio::time::sleep(SEALABLE_POLL).await;
        }
    }

    /// Devices of current members that this device lists but its MLS group does not hold yet: their add commit has not
    /// reached it. Banned, departed, revoked and storage members are left out, and so is a device a send already waited
    /// for. The group's devices are read once per epoch.
    pub(crate) fn pending_adds(&self, community: CommunityId) -> BTreeSet<DeviceId> {
        let Some(epoch) = self.mls_epoch(community) else {
            return BTreeSet::new();
        };
        let in_group = {
            let mut cache = self.inner.mls_devices.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match cache.get(&community) {
                Some((at, devices)) if *at == epoch => devices.clone(),
                _ => {
                    let devices: BTreeSet<DeviceId> = self
                        .with_mls_read(|mls| mls.members(&community.0).ok())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|cert| cert.device)
                        .collect();
                    cache.insert(community, (epoch, devices.clone()));
                    devices
                }
            }
        };
        let waited = self
            .inner
            .pending_adds_waited
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&community)
            .cloned()
            .unwrap_or_default();
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return BTreeSet::new();
        };
        let st = c.state();
        st.members
            .iter()
            .filter(|(root, m)| !m.left && !m.storage && !st.banned.contains(root))
            .flat_map(|(root, _)| w.directory.devices_of(root))
            .filter(|d| *d != w.device && !st.revoked_devices.contains_key(d) && !in_group.contains(d) && !waited.contains(d))
            .collect()
    }

    /// Holds a send while a member this device already lists is not in its MLS group yet, for at most
    /// `Timings::pending_add_wait`, and asks neighbours for the commits meanwhile. Sealed before the add, the message
    /// would be under an epoch the new member never had, and it could never read it. Each device holds one send at most:
    /// one whose add never comes (it has not asked to join yet, or nobody added it) does not slow every send.
    pub(crate) async fn wait_for_pending_adds(&self, community: CommunityId) {
        let pending = self.pending_adds(community);
        if pending.is_empty() {
            return;
        }
        self.inner
            .pending_adds_waited
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(community)
            .or_default()
            .extend(pending.iter().copied());
        let deadline = std::time::Instant::now() + self.inner.timings.pending_add_wait;
        let neighbours: Vec<DeviceId> = self
            .inner
            .neighbours
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&community)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();
        loop {
            for peer in &neighbours {
                self.mls_commit_check(community, *peer).await;
                if self.pending_in_group(community, &pending) {
                    return;
                }
            }
            if self.pending_in_group(community, &pending) || std::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(SEALABLE_POLL).await;
        }
    }

    /// Whether every device of `pending` is in the community's MLS group now.
    fn pending_in_group(&self, community: CommunityId, pending: &BTreeSet<DeviceId>) -> bool {
        let devices: BTreeSet<DeviceId> = self
            .with_mls_read(|mls| mls.members(&community.0).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|cert| cert.device)
            .collect();
        pending.iter().all(|d| devices.contains(d))
    }

    /// After taking a commit that added someone since banned: the group has usually moved on, so this device asks for
    /// the commits that followed at once. If nobody has removed the device, `mls_maintenance` does in this device's turn.
    pub(crate) fn follow_a_banned_add(&self, community: CommunityId) {
        self.inner.banned_adds.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(community, std::time::Instant::now());
        let engine = self.clone();
        self.spawn_task(async move {
            let neighbours: Vec<DeviceId> = engine
                .inner
                .neighbours
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&community)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default();
            for peer in neighbours {
                engine.mls_commit_check(community, peer).await;
                if !engine.banned_device_in_group(community) {
                    return;
                }
            }
        });
    }

    /// Devices in the group that belong to banned or departed members.
    fn departed_devices(&self, community: CommunityId) -> Vec<DeviceId> {
        let rules = self.commit_rules(community);
        let members = self
            .with_mls_read(|mls| mls.members(&community.0).ok())
            .unwrap_or_default();
        let w = self.read();
        members
            .iter()
            .map(|cert| cert.device)
            .filter(|device| {
                w.directory
                    .root_of(device)
                    .is_some_and(|root| rules.is_departed(&root))
            })
            .collect()
    }
}
