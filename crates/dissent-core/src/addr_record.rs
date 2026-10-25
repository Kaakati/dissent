//! Signed, versioned, expiring address records for peer exchange.
//!
//! A device signs its own record with its device key. The record names the device's relay and
//! direct addresses, carries a sequence number that only moves up, and expires after a day. The
//! cert ties the device to its root. Anyone who relays a record cannot change it, so a relay
//! cannot forge an address.
//!
//! This module holds the record type, the bounded cache that keeps verified records, and the
//! limit on how often one peer may push. Who may see which record is decided in `peer_exchange`.

use std::collections::{BTreeMap, HashMap};

use crate::engine::MAX_CLOCK_SKEW_MS;
use crate::error::{CoreError, Result};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};

/// Domain prefix for the signature over a record.
const RECORD_DOMAIN: &[u8] = b"dissent/addr-record/v1";
/// Records live this long. Devices re-sign them every ten minutes, well inside that.
pub(crate) const RECORD_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// Most records the cache holds. Least recently used records go first.
pub(crate) const CACHE_CAP: usize = 4096;
/// Most devices one root may have in the cache. Its least recently used device goes first.
pub(crate) const PER_ROOT_CAP: usize = 4;
/// Least time between two pushes the cache accepts from one peer.
pub(crate) const PUSH_GAP_MS: u64 = 10_000;
/// Most records one message carries.
pub(crate) const MAX_RECORDS_PER_MESSAGE: usize = 64;

/// A device's dialable address, signed by the device key at a sequence number, until `expires_ms`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AddrRecord {
    /// Certifies the device for its root.
    pub cert: DeviceCert,
    /// The device id, its relay and its direct addresses.
    pub addr: PeerAddr,
    /// Each new record of a device carries a larger value. A record with a lower or equal value is stale.
    pub seq: u64,
    /// Milliseconds since the Unix epoch after which the record is not used.
    pub expires_ms: u64,
    pub sig: Signature,
}

impl AddrRecord {
    /// Signs `addr` with `key`. `cert` must certify `key`.
    pub fn sign(
        addr: PeerAddr,
        cert: DeviceCert,
        seq: u64,
        expires_ms: u64,
        key: &DeviceKey,
    ) -> Result<Self> {
        let sig = key.sign(&signing_bytes(&addr, seq, expires_ms)?);
        Ok(Self {
            cert,
            addr,
            seq,
            expires_ms,
            sig,
        })
    }

    pub fn device(&self) -> DeviceId {
        self.addr.id
    }

    pub fn root(&self) -> RootId {
        self.cert.root
    }

    /// Checks the cert and that it names this device, the signature, and that the record is
    /// current at `now_ms` with a lifetime no longer than `RECORD_TTL_MS`.
    pub fn verify(&self, now_ms: u64) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.addr.id {
            return Err(CoreError::Invalid("record is not for the certified device".into()));
        }
        if self.expires_ms <= now_ms {
            return Err(CoreError::Invalid("record has expired".into()));
        }
        // A signer whose clock runs ahead of ours sees the same lifetime as longer, so allow for skew.
        if self.expires_ms.saturating_sub(now_ms) > RECORD_TTL_MS + MAX_CLOCK_SKEW_MS {
            return Err(CoreError::Invalid("record lives longer than allowed".into()));
        }
        self.addr
            .id
            .verify(&signing_bytes(&self.addr, self.seq, self.expires_ms)?, &self.sig)?;
        Ok(())
    }
}

fn signing_bytes(addr: &PeerAddr, seq: u64, expires_ms: u64) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(addr, seq, expires_ms))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(RECORD_DOMAIN.len() + body.len());
    out.extend_from_slice(RECORD_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

/// What the cache did with a record it was offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Learned {
    /// A device the cache did not hold before.
    New,
    /// A newer record for a device the cache held.
    Replaced,
}

struct Held {
    record: AddrRecord,
    /// Ticks of `RecordCache::clock` when the record was last learned or read.
    used: u64,
}

/// Verified records of other devices, bounded in size and per root. A device's record is kept
/// until it expires or is evicted to make room.
#[derive(Default)]
pub(crate) struct RecordCache {
    records: BTreeMap<DeviceId, Held>,
    clock: u64,
}

impl RecordCache {
    /// Verifies `record` and keeps it if it is newer than the one held for its device. Evicts
    /// the least recently used records to keep the per-root and total caps. A record that fails
    /// to verify, or is not newer, is refused.
    pub fn learn(&mut self, record: AddrRecord, now_ms: u64) -> Result<Learned> {
        record.verify(now_ms)?;
        let device = record.device();
        let root = record.root();
        if self
            .records
            .get(&device)
            .is_some_and(|held| held.record.seq >= record.seq)
        {
            return Err(CoreError::Invalid("record is not newer than the one held".into()));
        }
        self.clock += 1;
        let learned = if self.records.contains_key(&device) {
            Learned::Replaced
        } else {
            Learned::New
        };
        self.records.insert(
            device,
            Held {
                record,
                used: self.clock,
            },
        );
        self.enforce_caps(root);
        Ok(learned)
    }

    /// The record of `device`, if held and not expired. Marks it recently used.
    pub fn get(&mut self, device: &DeviceId, now_ms: u64) -> Option<AddrRecord> {
        self.expire(now_ms);
        self.clock += 1;
        let clock = self.clock;
        let held = self.records.get_mut(device)?;
        held.used = clock;
        Some(held.record.clone())
    }

    /// Drops every record that has expired by `now_ms`.
    pub fn expire(&mut self, now_ms: u64) {
        self.records.retain(|_, held| held.record.expires_ms > now_ms);
    }

    /// `(device, seq)` for every held record that `visible` accepts. Records of other devices of
    /// the same root are listed as well, so a device sees its own digest.
    pub fn digest(&self, now_ms: u64, visible: impl Fn(&AddrRecord) -> bool) -> Vec<(DeviceId, u64)> {
        self.records
            .values()
            .filter(|held| held.record.expires_ms > now_ms && visible(&held.record))
            .map(|held| (held.record.device(), held.record.seq))
            .collect()
    }

    /// The held records that `visible` accepts and that are newer than the ones in `theirs`.
    /// At most `MAX_RECORDS_PER_MESSAGE`, least recently used last.
    pub fn newer_than(
        &self,
        theirs: &[(DeviceId, u64)],
        now_ms: u64,
        visible: impl Fn(&AddrRecord) -> bool,
    ) -> Vec<AddrRecord> {
        let known: HashMap<DeviceId, u64> = theirs.iter().copied().collect();
        let mut out: Vec<&Held> = self
            .records
            .values()
            .filter(|held| held.record.expires_ms > now_ms && visible(&held.record))
            .filter(|held| known.get(&held.record.device()).is_none_or(|seq| *seq < held.record.seq))
            .collect();
        out.sort_by_key(|held| std::cmp::Reverse(held.used));
        out.into_iter()
            .take(MAX_RECORDS_PER_MESSAGE)
            .map(|held| held.record.clone())
            .collect()
    }

    /// Number of held records, expired ones included until the next `expire`.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn enforce_caps(&mut self, root: RootId) {
        loop {
            let same_root: Vec<(u64, DeviceId)> = self
                .records
                .iter()
                .filter(|(_, held)| held.record.root() == root)
                .map(|(device, held)| (held.used, *device))
                .collect();
            if same_root.len() <= PER_ROOT_CAP {
                break;
            }
            let victim = same_root.iter().min().map(|(_, device)| *device);
            if let Some(victim) = victim {
                self.records.remove(&victim);
            }
        }
        while self.records.len() > CACHE_CAP {
            let victim = self
                .records
                .iter()
                .min_by_key(|(_, held)| held.used)
                .map(|(device, _)| *device);
            if let Some(victim) = victim {
                self.records.remove(&victim);
            }
        }
    }
}

/// Lets one peer push records at most once per `PUSH_GAP_MS`. Refused pushes do not restart the wait.
#[derive(Default)]
pub(crate) struct PushLimiter {
    last: HashMap<DeviceId, u64>,
}

impl PushLimiter {
    /// Whether `peer` may push at `now_ms`. An admitted push starts its wait.
    pub fn admit(&mut self, peer: DeviceId, now_ms: u64) -> bool {
        if let Some(last) = self.last.get(&peer) {
            if now_ms.saturating_sub(*last) < PUSH_GAP_MS {
                return false;
            }
        }
        if self.last.len() >= CACHE_CAP {
            self.last
                .retain(|_, last| now_ms.saturating_sub(*last) < PUSH_GAP_MS);
        }
        self.last.insert(peer, now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;
    use std::net::SocketAddr;

    const NOW: u64 = 1_700_000_000_000;

    /// A root with one device, certified. Returns the device key and its cert.
    fn device_of(root: &RootKey) -> (DeviceKey, DeviceCert) {
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), NOW);
        (device, cert)
    }

    fn addr_for(device: &DeviceKey, port: u16) -> PeerAddr {
        PeerAddr {
            id: device.id(),
            relay: None,
            direct: vec![SocketAddr::from(([192, 0, 2, 1], port))],
        }
    }

    fn record(device: &DeviceKey, cert: &DeviceCert, seq: u64, expires_ms: u64) -> AddrRecord {
        AddrRecord::sign(addr_for(device, 4433), cert.clone(), seq, expires_ms, device).unwrap()
    }

    fn fresh_root() -> RootKey {
        RootKey::generate()
    }

    #[test]
    fn a_signed_record_verifies_and_round_trips() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let rec = record(&device, &cert, 1, NOW + 1000);
        rec.verify(NOW).expect("verifies");
        let bytes = postcard::to_allocvec(&rec).unwrap();
        let back: AddrRecord = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, rec);
        back.verify(NOW).expect("still verifies after a round trip");
    }

    #[test]
    fn a_tampered_address_is_refused() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let mut rec = record(&device, &cert, 1, NOW + 1000);
        rec.addr.direct = vec![SocketAddr::from(([198, 51, 100, 9], 4433))];
        assert!(rec.verify(NOW).is_err());
    }

    #[test]
    fn a_record_signed_by_another_device_is_refused() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let (other, _) = device_of(&root);
        // The cert names `device`, but the signature is by `other`.
        let forged = AddrRecord::sign(addr_for(&device, 4433), cert, 1, NOW + 1000, &other).unwrap();
        assert!(forged.verify(NOW).is_err());
    }

    #[test]
    fn a_cert_for_another_device_is_refused() {
        let root = fresh_root();
        let (device, _) = device_of(&root);
        let (_, other_cert) = device_of(&root);
        let rec = AddrRecord::sign(addr_for(&device, 4433), other_cert, 1, NOW + 1000, &device).unwrap();
        assert!(rec.verify(NOW).is_err(), "cert must certify the record's device");
    }

    #[test]
    fn a_cert_from_another_root_is_refused() {
        let root = fresh_root();
        let (device, mut cert) = device_of(&root);
        // Re-point the cert at a different root. Its root signature no longer holds.
        cert.root = fresh_root().id();
        let rec = AddrRecord::sign(addr_for(&device, 4433), cert, 1, NOW + 1000, &device).unwrap();
        assert!(rec.verify(NOW).is_err());
    }

    #[test]
    fn an_expired_record_is_refused_and_dropped() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let rec = record(&device, &cert, 1, NOW + 1000);
        assert!(rec.verify(NOW + 1000).is_err(), "expires_ms is the first moment it is stale");
        let mut cache = RecordCache::default();
        cache.learn(rec.clone(), NOW).unwrap();
        assert!(cache.get(&device.id(), NOW + 2000).is_none());
        assert!(cache.is_empty(), "expired records are removed on read");
    }

    #[test]
    fn a_record_claiming_too_long_a_life_is_refused() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let limit = RECORD_TTL_MS + MAX_CLOCK_SKEW_MS;
        let rec = record(&device, &cert, 1, NOW + limit);
        rec.verify(NOW).expect("a lifetime within the skew allowance verifies");
        let too_long = record(&device, &cert, 1, NOW + limit + 1);
        assert!(too_long.verify(NOW).is_err());
    }

    #[test]
    fn a_higher_seq_wins_and_a_stale_or_equal_one_is_refused() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let mut cache = RecordCache::default();
        let old = AddrRecord::sign(addr_for(&device, 4000), cert.clone(), 5, NOW + 1000, &device).unwrap();
        let new = AddrRecord::sign(addr_for(&device, 4001), cert.clone(), 6, NOW + 1000, &device).unwrap();
        assert_eq!(cache.learn(old.clone(), NOW).unwrap(), Learned::New);
        assert_eq!(cache.learn(new.clone(), NOW).unwrap(), Learned::Replaced);
        assert!(cache.learn(old, NOW).is_err(), "a lower seq is stale");
        assert!(cache.learn(new.clone(), NOW).is_err(), "an equal seq is stale");
        assert_eq!(cache.get(&device.id(), NOW).unwrap().addr.direct, new.addr.direct);
    }

    #[test]
    fn a_record_that_fails_verification_is_not_cached() {
        let root = fresh_root();
        let (device, cert) = device_of(&root);
        let mut rec = record(&device, &cert, 1, NOW + 1000);
        rec.seq = 2;
        let mut cache = RecordCache::default();
        assert!(cache.learn(rec, NOW).is_err());
        assert!(cache.is_empty());
    }

    #[test]
    fn digest_diff_sends_only_the_newer_records() {
        let root = fresh_root();
        let (a, a_cert) = device_of(&root);
        let (b, b_cert) = device_of(&root);
        let mut cache = RecordCache::default();
        cache.learn(record(&a, &a_cert, 3, NOW + 1000), NOW).unwrap();
        cache.learn(record(&b, &b_cert, 7, NOW + 1000), NOW).unwrap();

        // The peer already holds b at seq 7 and a at seq 2, so only a's newer record is sent.
        let theirs = vec![(b.id(), 7), (a.id(), 2)];
        let sent = cache.newer_than(&theirs, NOW, |_| true);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].device(), a.id());
        assert_eq!(sent[0].seq, 3);

        // A peer holding nothing gets both.
        assert_eq!(cache.newer_than(&[], NOW, |_| true).len(), 2);
        // The digest lists what we hold, as (device, seq).
        let mut digest = cache.digest(NOW, |_| true);
        digest.sort();
        let mut expected = vec![(a.id(), 3), (b.id(), 7)];
        expected.sort();
        assert_eq!(digest, expected);
    }

    #[test]
    fn visibility_filters_what_is_shared() {
        let root = fresh_root();
        let (a, a_cert) = device_of(&root);
        let other = fresh_root();
        let (b, b_cert) = device_of(&other);
        let mut cache = RecordCache::default();
        cache.learn(record(&a, &a_cert, 1, NOW + 1000), NOW).unwrap();
        cache.learn(record(&b, &b_cert, 1, NOW + 1000), NOW).unwrap();
        let only_root = |r: &AddrRecord| r.root() == root.id();
        let sent = cache.newer_than(&[], NOW, only_root);
        assert_eq!(sent.len(), 1, "records the filter rejects are never sent");
        assert_eq!(sent[0].device(), a.id());
        assert_eq!(cache.digest(NOW, only_root), vec![(a.id(), 1)]);
    }

    #[test]
    fn a_root_keeps_at_most_four_devices_and_evicts_the_least_recently_used() {
        let root = fresh_root();
        let mut cache = RecordCache::default();
        let mut devices = Vec::new();
        for _ in 0..(PER_ROOT_CAP + 1) {
            let (device, cert) = device_of(&root);
            cache.learn(record(&device, &cert, 1, NOW + 1000), NOW).unwrap();
            devices.push(device);
        }
        assert_eq!(cache.len(), PER_ROOT_CAP);
        // The first device learned was the least recently used, so it is the one evicted.
        assert!(cache.get(&devices[0].id(), NOW).is_none());
        assert!(cache.get(&devices[PER_ROOT_CAP].id(), NOW).is_some());
    }

    #[test]
    fn reading_a_record_protects_it_from_eviction() {
        let root = fresh_root();
        let mut cache = RecordCache::default();
        let mut devices = Vec::new();
        for _ in 0..PER_ROOT_CAP {
            let (device, cert) = device_of(&root);
            cache.learn(record(&device, &cert, 1, NOW + 1000), NOW).unwrap();
            devices.push(device);
        }
        // Touch the oldest, so the second-oldest becomes the victim.
        assert!(cache.get(&devices[0].id(), NOW).is_some());
        let (extra, extra_cert) = device_of(&root);
        cache.learn(record(&extra, &extra_cert, 1, NOW + 1000), NOW).unwrap();
        assert!(cache.get(&devices[0].id(), NOW).is_some());
        assert!(cache.get(&devices[1].id(), NOW).is_none());
    }

    #[test]
    fn the_total_cache_is_capped_and_evicts_globally_least_recently_used() {
        let mut cache = RecordCache::default();
        // One device per root keeps the per-root cap out of the way.
        let first_root = fresh_root();
        let (first, first_cert) = device_of(&first_root);
        cache.learn(record(&first, &first_cert, 1, NOW + 1000), NOW).unwrap();
        for _ in 0..CACHE_CAP {
            let root = fresh_root();
            let (device, cert) = device_of(&root);
            cache.learn(record(&device, &cert, 1, NOW + 1000), NOW).unwrap();
        }
        assert_eq!(cache.len(), CACHE_CAP);
        assert!(cache.get(&first.id(), NOW).is_none(), "the oldest record was evicted");
    }

    #[test]
    fn a_peer_may_push_once_per_gap() {
        let mut limiter = PushLimiter::default();
        let peer = DeviceId([1; 32]);
        assert!(limiter.admit(peer, NOW));
        assert!(!limiter.admit(peer, NOW + 9_999), "inside the gap");
        assert!(limiter.admit(peer, NOW + PUSH_GAP_MS), "after the gap");
        assert!(!limiter.admit(peer, NOW + PUSH_GAP_MS + 1), "refusals do not restart the wait");
        assert!(limiter.admit(DeviceId([2; 32]), NOW + 1), "other peers are independent");
    }
}
