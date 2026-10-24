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
