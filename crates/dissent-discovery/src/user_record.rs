//! [`UserRecord`]: the devices of one user, published at the user's own address.
//!
//! A user's address is its root key ([`RootId`]), and a Pkarr packet is signed by its key. So the
//! record lives at the DHT key that is the address: only the root key can publish under it, and
//! anyone who holds the address can resolve it. The record is public on purpose. It is how a
//! person is found by address, so it reveals that the address uses Dissent and which device ids
//! hold it (see the threat model, design 06).
//!
//! Encoding: `"dissent/user-record/v1" || postcard(WireRecord)`, split into TXT frames under
//! `_dissent` the same way as a community record. Each device is `(device, issued_at_ms, sig)`.
//! Its root is the record's own address, so each cert is rebuilt and verified against it. An empty
//! record withdraws the address; a later packet replaces an earlier one on the DHT.

use dissent_identity::{DeviceCert, DeviceId, RootId, RootKey, Signature};
use pkarr::{PublicKey, SignedPacket};
use serde::{Deserialize, Serialize};

use crate::record::{chunked_packet, reassemble};
use crate::{DiscoveryError, Result};

/// Most device certs in one user record.
pub const MAX_DEVICES: usize = 4;
/// Most peer addresses in one user record. Kept small so the packet stays under 1000 bytes.
pub const MAX_USER_PEERS: usize = 2;
const PAYLOAD_DOMAIN: &[u8] = b"dissent/user-record/v1";

/// The devices of a user and where to dial them. Every cert is signed by the record's root.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UserRecord {
    pub devices: Vec<DeviceCert>,
    /// Opaque `PeerAddr` strings from `dissent-net`. At most [`MAX_USER_PEERS`].
    pub peers: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct WireDevice {
    device: DeviceId,
    issued_at_ms: u64,
    sig: Signature,
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    devices: Vec<WireDevice>,
    peers: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> DiscoveryError {
    DiscoveryError::InvalidRecord(msg.into())
}

impl UserRecord {
    /// True when the record lists no devices, which means the address is not discoverable.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    fn check_limits(&self) -> Result<()> {
        if self.devices.len() > MAX_DEVICES {
            return Err(invalid(format!("{} devices exceeds maximum {MAX_DEVICES}", self.devices.len())));
        }
        if self.peers.len() > MAX_USER_PEERS {
            return Err(invalid(format!("{} peers exceeds maximum {MAX_USER_PEERS}", self.peers.len())));
        }
        Ok(())
    }

    /// Signs the record with the root key and packs it into a packet for the DHT.
    pub fn to_packet(&self, root: &RootKey) -> Result<SignedPacket> {
        self.check_limits()?;
        let id = root.id();
        let mut devices = Vec::with_capacity(self.devices.len());
        for cert in &self.devices {
            if cert.root != id {
                return Err(invalid("a device cert is for another root"));
            }
            cert.verify().map_err(|_| invalid("a device cert does not verify"))?;
            devices.push(WireDevice {
                device: cert.device,
                issued_at_ms: cert.issued_at_ms,
                sig: cert.sig,
            });
        }
        let wire = WireRecord {
            devices,
            peers: self.peers.clone(),
        };
        let mut payload = PAYLOAD_DOMAIN.to_vec();
        payload.extend(postcard::to_allocvec(&wire).map_err(|e| DiscoveryError::Encoding(e.to_string()))?);
        chunked_packet(&payload, &pkarr::Keypair::from_secret_key(&root.0.seed()))
    }

    /// Checks that `packet` is signed by the address `id`, decodes it, and verifies every device
    /// cert against `id`. A forged or tampered cert fails the whole record.
    pub fn from_packet(packet: &SignedPacket, id: &RootId) -> Result<Self> {
        if packet.public_key().to_bytes() != *id.as_bytes() {
            return Err(invalid("the record is not signed by this address"));
        }
        let payload = reassemble(packet)?;
        let body = payload
            .strip_prefix(PAYLOAD_DOMAIN)
            .ok_or_else(|| invalid("the record has the wrong type"))?;
        let wire: WireRecord = postcard::from_bytes(body).map_err(|e| DiscoveryError::Encoding(e.to_string()))?;
        let devices: Vec<DeviceCert> = wire
            .devices
            .into_iter()
            .map(|d| DeviceCert {
                root: *id,
                device: d.device,
                issued_at_ms: d.issued_at_ms,
                sig: d.sig,
            })
            .collect();
        for cert in &devices {
            cert.verify().map_err(|_| invalid("a device cert does not verify"))?;
        }
        let rec = Self {
            devices,
            peers: wire.peers,
        };
        rec.check_limits()?;
        Ok(rec)
    }
}

/// The public key a resolver asks the DHT for, for the address `id`.
pub fn dht_key(id: &RootId) -> Result<PublicKey> {
    PublicKey::try_from(id.as_bytes()).map_err(|e| DiscoveryError::Pkarr(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::{DeviceKey, Identity};

    fn record_for(root: &RootKey, devices: usize) -> UserRecord {
        UserRecord {
            devices: (0..devices)
                .map(|i| root.certify(DeviceKey::generate().id(), 100 + i as u64))
                .collect(),
            peers: vec!["peer-a".into()],
        }
    }

    /// Builds a packet from raw wire fields, signed by `root`. Used to forge records.
    fn raw_packet(root: &RootKey, wire: &WireRecord) -> SignedPacket {
        let mut payload = PAYLOAD_DOMAIN.to_vec();
        payload.extend(postcard::to_allocvec(wire).unwrap());
        chunked_packet(&payload, &pkarr::Keypair::from_secret_key(&root.0.seed())).unwrap()
    }

    fn wire_of(rec: &UserRecord) -> WireRecord {
        WireRecord {
            devices: rec
                .devices
                .iter()
                .map(|c| WireDevice {
                    device: c.device,
                    issued_at_ms: c.issued_at_ms,
                    sig: c.sig,
                })
                .collect(),
            peers: rec.peers.clone(),
        }
    }

    #[test]
    fn record_round_trips_through_a_packet() {
        let root = RootKey::generate();
        let rec = record_for(&root, 2);
        let packet = rec.to_packet(&root).unwrap();
        assert_eq!(UserRecord::from_packet(&packet, &root.id()).unwrap(), rec);
    }

    #[test]
    fn empty_record_round_trips_and_is_empty() {
        let root = RootKey::generate();
        let rec = UserRecord::default();
        let packet = rec.to_packet(&root).unwrap();
        let back = UserRecord::from_packet(&packet, &root.id()).unwrap();
        assert!(back.is_empty());
        assert_eq!(back, rec);
    }

    #[test]
    fn a_record_for_another_address_is_refused() {
        let root = RootKey::generate();
        let packet = record_for(&root, 1).to_packet(&root).unwrap();
        let other = RootKey::generate().id();
        assert!(UserRecord::from_packet(&packet, &other).is_err());
    }

    #[test]
    fn a_packet_signed_by_a_stranger_is_refused() {
        // Someone who knows the address and the record layout, but not the root key.
        let root = RootKey::generate();
        let stranger = RootKey::generate();
        let rec = record_for(&root, 1);
        let packet = raw_packet(&stranger, &wire_of(&rec));
        assert!(UserRecord::from_packet(&packet, &root.id()).is_err());
    }

    #[test]
    fn a_forged_device_cert_is_refused() {
        let root = RootKey::generate();
        let rec = record_for(&root, 1);
        let mut wire = wire_of(&rec);
        wire.devices[0].issued_at_ms += 1;
        let packet = raw_packet(&root, &wire);
        let err = UserRecord::from_packet(&packet, &root.id()).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidRecord(_)), "{err:?}");
    }

    #[test]
    fn a_cert_from_another_root_is_refused() {
        // The root signs the packet, but a device cert inside was made by a different root.
        let root = RootKey::generate();
        let other = RootKey::generate();
        let foreign = other.certify(DeviceKey::generate().id(), 5);
        let rec = UserRecord {
            devices: vec![foreign],
            peers: vec![],
        };
        let packet = raw_packet(&root, &wire_of(&rec));
        assert!(UserRecord::from_packet(&packet, &root.id()).is_err());
        // Building such a record is refused too.
        assert!(rec.to_packet(&root).is_err());
    }

    #[test]
    fn building_a_record_with_a_bad_cert_is_refused() {
        let root = RootKey::generate();
        let mut rec = record_for(&root, 1);
        rec.devices[0].issued_at_ms += 1;
        assert!(rec.to_packet(&root).is_err());
    }

    #[test]
    fn too_many_devices_or_peers_is_refused() {
        let root = RootKey::generate();
        assert!(record_for(&root, MAX_DEVICES + 1).to_packet(&root).is_err());
        let mut rec = record_for(&root, 1);
        rec.peers = vec!["p".into(); MAX_USER_PEERS + 1];
        assert!(rec.to_packet(&root).is_err());
    }

    #[test]
    fn a_device_of_a_fresh_identity_round_trips() {
        let id = Identity::create(1);
        let root = id.root.clone().unwrap();
        let rec = UserRecord {
            devices: vec![id.cert.clone()],
            peers: vec![],
        };
        let packet = rec.to_packet(&root).unwrap();
        assert_eq!(UserRecord::from_packet(&packet, &root.id()).unwrap(), rec);
    }
}
