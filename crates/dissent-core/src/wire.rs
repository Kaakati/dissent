//! Gossip envelopes and signed device addresses.

use crate::error::{CoreError, Result};
use dissent_events::SignedEvent;
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};

/// Domain prefix for the signature over an address announcement.
const ADDR_DOMAIN: &[u8] = b"dissent/addr/v1";
/// Domain prefix for the signature over a commit announcement.
const COMMIT_DOMAIN: &[u8] = b"dissent/commit/v1";
/// Domain prefix for the signature over an epoch check.
const EPOCH_CHECK_DOMAIN: &[u8] = b"dissent/epoch-check/v1";

/// Everything that travels on a community topic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GossipMsg {
    Event(Box<SignedEvent>),
    Addr(Box<SignedAddr>),
    /// An MLS commit was applied. The bytes are fetched over RPC, so a large commit never meets
    /// the gossip frame limit.
    Commit(Box<SignedCommit>),
    /// A presence beacon from a member. See `presence.rs`.
    Presence(Box<crate::presence::SignedPresence>),
    /// A typing signal from a member. See `typing.rs`.
    Typing(Box<crate::typing::SignedTyping>),
    /// A member's check of the epoch its group is at, so devices on two branches of a forked group
    /// notice. See `forks.rs`. Added in v0.1.26, last, so every earlier variant keeps its index; a
    /// released client fails to decode it and drops only this message.
    EpochCheck(Box<SignedEpochCheck>),
}

/// A device's check of the MLS epoch it is at: a value every member of that epoch derives alike, and
/// two branches of a forked group derive differently. Signed by the device key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedEpochCheck {
    pub community: CommunityId,
    pub epoch: u64,
    /// An MLS exporter secret of the epoch, see `forks::EPOCH_CHECK_LABEL`. It means nothing to a
    /// device outside the group.
    pub check: [u8; 16],
    pub device: DeviceId,
    pub ts_ms: u64,
    pub sig: Signature,
}

impl SignedEpochCheck {
    pub fn sign(community: CommunityId, epoch: u64, check: [u8; 16], ts_ms: u64, key: &DeviceKey) -> Result<Self> {
        let device = key.id();
        let sig = key.sign(&epoch_check_signing_bytes(community, epoch, &check, device, ts_ms)?);
        Ok(Self {
            community,
            epoch,
            check,
            device,
            ts_ms,
            sig,
        })
    }

    pub fn verify(&self) -> Result<()> {
        let msg = epoch_check_signing_bytes(self.community, self.epoch, &self.check, self.device, self.ts_ms)?;
        self.device.verify(&msg, &self.sig)?;
        Ok(())
    }
}

fn epoch_check_signing_bytes(
    community: CommunityId,
    epoch: u64,
    check: &[u8; 16],
    device: DeviceId,
    ts_ms: u64,
) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(community, epoch, check, device, ts_ms))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(EPOCH_CHECK_DOMAIN.len() + body.len());
    out.extend_from_slice(EPOCH_CHECK_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Announces the MLS commit that entered `epoch`. Signed by the committer's device key, so a
/// relay cannot substitute another commit. The fetched bytes must match `hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedCommit {
    pub community: CommunityId,
    /// The epoch the group enters once the commit applies.
    pub epoch: u64,
    /// blake3 of the commit bytes.
    pub hash: [u8; 32],
    pub committer: DeviceId,
    pub sig: Signature,
}

impl SignedCommit {
    /// Announces `commit`, made by `key`, which belongs to `committer`.
    pub fn sign(
        community: CommunityId,
        epoch: u64,
        commit: &[u8],
        committer: DeviceId,
        key: &DeviceKey,
    ) -> Result<Self> {
        let hash = *blake3::hash(commit).as_bytes();
        let sig = key.sign(&commit_signing_bytes(community, epoch, &hash, committer)?);
        Ok(Self {
            community,
            epoch,
            hash,
            committer,
            sig,
        })
    }

    pub fn verify(&self) -> Result<()> {
        let msg = commit_signing_bytes(self.community, self.epoch, &self.hash, self.committer)?;
        self.committer.verify(&msg, &self.sig)?;
        Ok(())
    }

    /// Whether `commit` is the commit this announcement names.
    pub fn matches(&self, commit: &[u8]) -> bool {
        blake3::hash(commit).as_bytes() == &self.hash
    }
}

fn commit_signing_bytes(
    community: CommunityId,
    epoch: u64,
    hash: &[u8; 32],
    committer: DeviceId,
) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(community, epoch, hash, committer))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(COMMIT_DOMAIN.len() + body.len());
    out.extend_from_slice(COMMIT_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

pub(crate) fn encode_gossip(msg: &GossipMsg) -> Result<Vec<u8>> {
    postcard::to_allocvec(msg).map_err(|e| CoreError::Encoding(e.to_string()))
}

pub(crate) fn decode_gossip(bytes: &[u8]) -> Result<GossipMsg> {
    postcard::from_bytes(bytes).map_err(|e| CoreError::Encoding(e.to_string()))
}

/// The gossip envelope as released clients up to v0.1.25 decode it, before `EpochCheck`. A test
/// engine acting as a released client decodes with it (`test_support::act_as_released_gossip`).
pub(crate) mod released {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) enum GossipMsg {
        Event(Box<dissent_events::SignedEvent>),
        Addr(Box<super::SignedAddr>),
        Commit(Box<super::SignedCommit>),
        Presence(Box<crate::presence::SignedPresence>),
        Typing(Box<crate::typing::SignedTyping>),
    }

    pub(crate) fn decode(bytes: &[u8]) -> crate::error::Result<GossipMsg> {
        postcard::from_bytes(bytes).map_err(|e| crate::error::CoreError::Encoding(e.to_string()))
    }
}

/// A device's dialable address, signed by the device key at `ts_ms`. Anyone may relay it, so
/// receivers check the signature rather than trusting the sender.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedAddr {
    pub addr: PeerAddr,
    pub cert: DeviceCert,
    pub ts_ms: u64,
    pub sig: Signature,
}

impl SignedAddr {
    /// Signs `addr` with the device key. `cert` must certify that key.
    pub fn sign(addr: PeerAddr, cert: DeviceCert, ts_ms: u64, key: &DeviceKey) -> Result<Self> {
        let sig = key.sign(&signing_bytes(&addr, ts_ms)?);
        Ok(Self {
            addr,
            cert,
            ts_ms,
            sig,
        })
    }

    /// Checks the cert, that it certifies the address's device, and the device's signature.
    pub fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.addr.id {
            return Err(CoreError::Invalid("address is not for the certified device".into()));
        }
        self.addr
            .id
            .verify(&signing_bytes(&self.addr, self.ts_ms)?, &self.sig)?;
        Ok(())
    }
}

fn signing_bytes(addr: &PeerAddr, ts_ms: u64) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(addr, ts_ms))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(ADDR_DOMAIN.len() + body.len());
    out.extend_from_slice(ADDR_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Most signed addresses one reply carries.
pub(crate) const MAX_ADDRS_PER_REPLY: usize = 64;

#[cfg(test)]
mod epoch_check_tests {
    use super::*;

    fn check(key: &DeviceKey) -> SignedEpochCheck {
        SignedEpochCheck::sign(CommunityId([7; 32]), 12, [3; 16], 1_000, key).expect("sign")
    }

    #[test]
    fn an_epoch_check_verifies_and_any_change_breaks_it() {
        let key = DeviceKey::generate();
        let good = check(&key);
        good.verify().expect("verifies");
        let mut epoch = good.clone();
        epoch.epoch += 1;
        let mut value = good.clone();
        value.check[0] ^= 1;
        let mut device = good.clone();
        device.device = DeviceKey::generate().id();
        let mut ts = good.clone();
        ts.ts_ms += 1;
        let mut community = good.clone();
        community.community = CommunityId([8; 32]);
        for bad in [epoch, value, device, ts, community] {
            assert!(bad.verify().is_err(), "{bad:?}");
        }
    }

    /// The signature covers the domain prefix, so a signature over the same fields made for another purpose
    /// does not pass.
    #[test]
    fn the_epoch_check_signature_is_domain_separated() {
        assert_eq!(EPOCH_CHECK_DOMAIN, b"dissent/epoch-check/v1");
        let key = DeviceKey::generate();
        let good = check(&key);
        let body = postcard::to_allocvec(&(good.community, good.epoch, &good.check, good.device, good.ts_ms)).expect("encode");
        let undomained = SignedEpochCheck { sig: key.sign(&body), ..good };
        assert!(undomained.verify().is_err());
    }

    /// Postcard writes an enum's variant index first. `EpochCheck` is the sixth variant, and stays so: a
    /// variant added before it would shift every client's decoding.
    #[test]
    fn the_epoch_check_variant_index_is_pinned() {
        let key = DeviceKey::generate();
        let bytes = encode_gossip(&GossipMsg::EpochCheck(Box::new(check(&key)))).expect("encode");
        assert_eq!(bytes[0], 5);
        assert_eq!(decode_gossip(&bytes).expect("decode"), GossipMsg::EpochCheck(Box::new(check(&key))));
    }

    /// A released client cannot decode an epoch check, and decodes every earlier variant as this build writes
    /// it, so the new variant changes nothing else on the wire.
    #[test]
    fn a_released_client_refuses_only_the_epoch_check() {
        let key = DeviceKey::generate();
        let bytes = encode_gossip(&GossipMsg::EpochCheck(Box::new(check(&key)))).expect("encode");
        assert!(released::decode(&bytes).is_err());
        let commit = SignedCommit::sign(CommunityId([7; 32]), 3, b"commit", key.id(), &key).expect("sign");
        let bytes = encode_gossip(&GossipMsg::Commit(Box::new(commit.clone()))).expect("encode");
        assert_eq!(released::decode(&bytes).expect("decodes"), released::GossipMsg::Commit(Box::new(commit)));
        let identity = dissent_identity::Identity::create(1);
        let addr = PeerAddr { id: identity.device_id(), relay: None, direct: Vec::new() };
        let signed = SignedAddr::sign(addr, identity.cert.clone(), 5, &identity.device).expect("sign");
        let bytes = encode_gossip(&GossipMsg::Addr(Box::new(signed.clone()))).expect("encode");
        assert_eq!(released::decode(&bytes).expect("decodes"), released::GossipMsg::Addr(Box::new(signed)));
        let beacon = crate::presence::SignedPresence::sign(identity.cert.clone(), crate::presence::Status::Online, 5, &identity.device)
            .expect("sign");
        let bytes = encode_gossip(&GossipMsg::Presence(Box::new(beacon.clone()))).expect("encode");
        assert_eq!(released::decode(&bytes).expect("decodes"), released::GossipMsg::Presence(Box::new(beacon)));
    }
}
