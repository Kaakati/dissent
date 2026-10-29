//! Our own address. Each change is signed, cached, persisted and announced, so a device that
//! restarts on a new address stays reachable without waiting for the DHT. Members also pass on
//! each other's signed addresses, so a joiner can reach members it was never introduced to.

use crate::engine::{now_ms, Engine, Inner};
use crate::error::Result;
use crate::identity_watch;
use crate::rpc::Request;
use crate::store::write_bin;
use crate::wire::{encode_gossip, GossipMsg, SignedAddr, MAX_ADDRS_PER_REPLY};
use dissent_identity::{CommunityId, DeviceId, RootId};
use dissent_net::{PeerAddr, TopicSender};
use futures_lite::{Stream, StreamExt};
use std::collections::BTreeSet;
use std::sync::{Arc, Weak};
use tracing::{debug, warn};

impl Engine {
    /// Starts the task that follows our endpoint address. The first item arrives straight away.
    pub(crate) fn spawn_addr_watch(&self) {
        let addrs = self.node().watch_addr();
        self.spawn_task(watch_addr(Arc::downgrade(&self.inner), addrs));
    }

    /// Signs and stores a new address of ours, then announces it.
    async fn on_own_addr(&self, addr: PeerAddr) {
        let record_addr = addr.clone();
        let signed = {
            let mut guard = self.write();
            // Timestamps only move forward, even across restarts and clock steps.
            let ts = now_ms().max(guard.my_addr_ts.saturating_add(1));
            let signed = match SignedAddr::sign(
                addr.clone(),
                self.inner.identity.cert.clone(),
                ts,
                &self.inner.identity.device,
            ) {
                Ok(signed) => signed,
                Err(e) => {
                    warn!(error = %e, "could not sign our address");
                    return;
                }
            };
            guard.my_addr = Some(addr);
            guard.my_addr_ts = ts;
            guard.my_signed = Some(signed.clone());
            if let Err(e) = write_bin(&guard.paths.own_addr(), &signed) {
                warn!(error = %e, "could not persist our address");
            }
            signed
        };
        // While invisible the new address is kept, and announced once visible again.
        if self.invisible() {
            return;
        }
        self.announce(&signed).await;
        self.refresh_own_record(record_addr).await;
        self.spawn_publish_records();
    }

    /// Sends a signed address to every community topic we are on, and to each device of our
    /// contacts and direct message peers. Delivery is best effort.
    pub(crate) async fn announce(&self, signed: &SignedAddr) {
        let senders: Vec<Arc<TopicSender>> = self
            .read()
            .communities
            .values()
            .filter_map(|c| c.sender.clone())
            .collect();
        match encode_gossip(&GossipMsg::Addr(Box::new(signed.clone()))) {
            Ok(bytes) => {
                for sender in senders {
                    if let Err(e) = sender.broadcast(bytes.clone()).await {
                        debug!(error = %e, "address not broadcast on a community topic");
                    }
                }
            }
            Err(e) => warn!(error = %e, "could not encode our address"),
        }

        let direct = match self.request_bytes(&Request::Addr(Box::new(signed.clone()))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our address");
                return;
            }
        };
        let node = self.node();
        for device in self.announce_targets() {
            let node = node.clone();
            let bytes = direct.clone();
            self.spawn_task(async move {
                if let Err(e) = node.request(device, bytes).await {
                    debug!(device = %device, error = %e, "address not delivered directly");
                }
            });
        }
    }

    /// Sends our signed address on one community's topic, so its members learn it.
    pub(crate) async fn broadcast_own_addr(&self, community: CommunityId) {
        if self.invisible() {
            return;
        }
        let (sender, signed) = {
            let w = self.read();
            (
                w.communities.get(&community).and_then(|c| c.sender.clone()),
                w.my_signed.clone(),
            )
        };
        let (Some(sender), Some(signed)) = (sender, signed) else {
            return;
        };
        match encode_gossip(&GossipMsg::Addr(Box::new(signed))) {
            Ok(bytes) => {
                if let Err(e) = sender.broadcast(bytes).await {
                    debug!(community = %community, error = %e, "address not broadcast");
                }
            }
            Err(e) => warn!(error = %e, "could not encode our address"),
        }
    }

    /// Re-sends our signed address on every community topic.
    pub(crate) async fn rebroadcast_own_addr(&self) {
        let communities: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for community in communities {
            self.broadcast_own_addr(community).await;
        }
    }

    /// Devices of our contacts and of the people we have exchanged direct messages with who are still
    /// in scope: a contact, or a member of a community we are in. A legacy DM peer who is neither, such
    /// as a stranger under the `Everyone` policy or a co-member who left, no longer gets our address.
    pub(crate) fn announce_targets(&self) -> Vec<DeviceId> {
        let w = self.read();
        let mut roots: BTreeSet<RootId> = w
            .dms
            .keys()
            .copied()
            .filter(|root| crate::peer_exchange::in_scope(&w, root))
            .collect();
        roots.extend(w.contacts.iter().map(|c| c.cert.root));
        roots.remove(&w.me);
        let mut devices: BTreeSet<DeviceId> = BTreeSet::new();
        for root in roots {
            devices.extend(w.directory.devices_of(&root));
        }
        devices.remove(&w.device);
        devices.into_iter().collect()
    }

    /// The signed addresses we pass on for `community`: ours first, then the members' we have.
    /// Capped at `MAX_ADDRS_PER_REPLY`. Unsigned addresses are never passed on.
    pub(crate) fn member_addrs(&self, community: CommunityId) -> Vec<SignedAddr> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let devices = w.member_devices(c);
        let mut out: Vec<SignedAddr> = w.my_signed.iter().cloned().collect();
        out.extend(w.addresses.signed_for(&devices));
        out.truncate(MAX_ADDRS_PER_REPLY);
        out
    }

    /// Keeps the signed addresses that belong to members of `community`. The sender of each one
    /// is only a relay, so each is checked on its own: a valid signature, and a device of a
    /// current member.
    pub(crate) fn learn_member_addrs(&self, community: CommunityId, addrs: Vec<SignedAddr>) {
        for sa in addrs {
            let is_member = self
                .read()
                .communities
                .get(&community)
                .is_some_and(|c| c.is_member(&sa.cert.root));
            if !is_member {
                continue;
            }
            if let Err(e) = self.accept_addr(sa) {
                debug!(community = %community, error = %e, "rejected a member address");
            }
        }
    }

    /// Checks a signed address and keeps it if it is newer than the one held for that device.
    pub(crate) fn accept_addr(&self, sa: SignedAddr) -> Result<()> {
        sa.verify()?;
        if sa.addr.id == self.inner.identity.device_id() {
            return Ok(());
        }
        let mut guard = self.write();
        let w = &mut *guard;
        let notice = identity_watch::learn_device(
            &mut w.directory,
            &w.dms,
            &w.conv_book,
            &mut w.identity_changes,
            &w.paths.identity_changes(),
            &sa.cert,
            now_ms(),
        );
        let addr = sa.addr.clone();
        if w.addresses.learn_signed(sa) {
            if let Some(node) = self.inner.node.get() {
                if let Err(e) = node.remember(&addr) {
                    debug!(error = %e, "could not remember announced address");
                }
            }
            if let Err(e) = w.save_addresses() {
                warn!(error = %e, "could not persist peer addresses");
            }
        }
        drop(guard);
        if let Some(ev) = notice {
            self.emit(ev);
        }
        Ok(())
    }
}

/// Follows our endpoint address until the engine is dropped.
async fn watch_addr(weak: Weak<Inner>, mut addrs: impl Stream<Item = PeerAddr> + Unpin) {
    while let Some(addr) = addrs.next().await {
        let Some(inner) = weak.upgrade() else {
            break;
        };
        Engine { inner }.on_own_addr(addr).await;
    }
}
