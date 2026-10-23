//! Our own address. Each change is signed, cached, persisted and announced, so a device that
//! restarts on a new address stays reachable without waiting for the DHT.

use crate::engine::{now_ms, Engine, Inner};
use crate::error::Result;
use crate::rpc::{encode, Request};
use crate::store::write_bin;
use crate::wire::{encode_gossip, GossipMsg, SignedAddr};
use dissent_identity::{DeviceId, RootId};
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
            if let Err(e) = write_bin(&guard.paths.own_addr(), &signed) {
                warn!(error = %e, "could not persist our address");
            }
            signed
        };
        self.announce(&signed).await;
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
        match encode_gossip(&GossipMsg::Addr(signed.clone())) {
            Ok(bytes) => {
                for sender in senders {
                    if let Err(e) = sender.broadcast(bytes.clone()).await {
                        debug!(error = %e, "address not broadcast on a community topic");
                    }
                }
            }
            Err(e) => warn!(error = %e, "could not encode our address"),
        }

        let direct = match encode(&Request::Addr(Box::new(signed.clone()))) {
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

    /// Devices of our contacts and of everyone we have exchanged direct messages with.
    fn announce_targets(&self) -> Vec<DeviceId> {
        let w = self.read();
        let mut roots: BTreeSet<RootId> = w.dms.keys().copied().collect();
        roots.extend(w.contacts.iter().map(|c| c.cert.root));
        roots.remove(&w.me);
        let mut devices: BTreeSet<DeviceId> = BTreeSet::new();
        for root in roots {
            devices.extend(w.directory.devices_of(&root));
        }
        devices.remove(&w.device);
        devices.into_iter().collect()
    }

    /// Checks a signed address and keeps it if it is newer than the one held for that device.
    pub(crate) fn accept_addr(&self, sa: SignedAddr) -> Result<()> {
        sa.verify()?;
        if sa.addr.id == self.inner.identity.device_id() {
            return Ok(());
        }
        let mut guard = self.write();
        let w = &mut *guard;
        w.directory.learn(&sa.cert);
        if w.addresses.learn_signed(sa.addr.clone(), sa.ts_ms) {
            if let Some(node) = self.inner.node.get() {
                if let Err(e) = node.remember(&sa.addr) {
                    debug!(error = %e, "could not remember announced address");
                }
            }
            if let Err(e) = w.save_addresses() {
                warn!(error = %e, "could not persist peer addresses");
            }
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
