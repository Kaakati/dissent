//! Adding joiners to a community's MLS group, and removing devices of banned or departed members.
//! Any member may do either, within the rules in the parent module. A device waits its turn first,
//! so the highest-ranked online member usually acts, and a second attempt finds nothing left to do.

use dissent_identity::{CommunityId, DeviceId};
use dissent_net::PeerAddr;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::sealed::mls_err;

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
        self.publish_commit(community, commit.commit).await;
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
        .map_err(mls_err)
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
        let departed = self.departed_devices(community);
        if departed.is_empty() {
            return;
        }
        match self.with_mls(|mls| mls.remove_members(&community.0, &departed)) {
            Ok(commit) => self.publish_commit(community, commit).await,
            Err(e) => {
                tracing::warn!(community = %community, error = %e, "could not remove departed devices")
            }
        }
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
