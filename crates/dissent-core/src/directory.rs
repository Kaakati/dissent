//! What we know about other devices: their certs, and the addresses we can dial them at.

use dissent_identity::{DeviceCert, DeviceId, RootId};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Verified device certs, indexed by device and by the user they belong to.
#[derive(Default)]
pub(crate) struct Directory {
    certs: BTreeMap<DeviceId, DeviceCert>,
    by_root: BTreeMap<RootId, BTreeSet<DeviceId>>,
}

impl Directory {
    /// Records a cert after checking it. Invalid certs are ignored.
    pub fn learn(&mut self, cert: &DeviceCert) {
        if self.certs.contains_key(&cert.device) || cert.verify().is_err() {
            return;
        }
        self.certs.insert(cert.device, cert.clone());
        self.by_root.entry(cert.root).or_default().insert(cert.device);
    }

    /// True when this exact cert was learned before, so its signature is already checked.
    pub fn knows(&self, cert: &DeviceCert) -> bool {
        self.certs.get(&cert.device) == Some(cert)
    }

    pub fn devices_of(&self, root: &RootId) -> Vec<DeviceId> {
        self.by_root
            .get(root)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// One stored address. `ts_ms` is zero for an address that no device has signed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredAddr {
    pub addr: PeerAddr,
    pub ts_ms: u64,
}

/// Last known dialable address per device. A signed announcement wins over anything unsigned.
#[derive(Default)]
pub(crate) struct AddressBook {
    by_device: BTreeMap<DeviceId, StoredAddr>,
}

impl AddressBook {
    pub fn from_list(list: Vec<StoredAddr>) -> Self {
        Self {
            by_device: list.into_iter().map(|s| (s.addr.id, s)).collect(),
        }
    }

    /// Stores an address from an unsigned source, such as an invite or a contact card.
    /// It never replaces an address the device has announced itself. Returns true when this
    /// changed what was stored.
    pub fn learn(&mut self, addr: PeerAddr) -> bool {
        match self.by_device.get(&addr.id) {
            Some(old) if old.ts_ms > 0 || old.addr == addr => false,
            _ => {
                self.by_device.insert(addr.id, StoredAddr { addr, ts_ms: 0 });
                true
            }
        }
    }

    /// Stores an address the device signed at `ts_ms`. Only a newer announcement replaces the
    /// stored one. Returns true when this changed what was stored.
    pub fn learn_signed(&mut self, addr: PeerAddr, ts_ms: u64) -> bool {
        if self.by_device.get(&addr.id).is_some_and(|old| old.ts_ms >= ts_ms) {
            return false;
        }
        self.by_device.insert(addr.id, StoredAddr { addr, ts_ms });
        true
    }

    pub fn all(&self) -> Vec<StoredAddr> {
        self.by_device.values().cloned().collect()
    }

    /// Every stored address, without its timestamp.
    pub fn peers(&self) -> Vec<PeerAddr> {
        self.by_device.values().map(|s| s.addr.clone()).collect()
    }
}
