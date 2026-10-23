//! Gossip envelopes and signed device addresses.

use crate::error::{CoreError, Result};
use dissent_events::SignedEvent;
use dissent_identity::{DeviceCert, DeviceKey, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};

/// Domain prefix for the signature over an address announcement.
const ADDR_DOMAIN: &[u8] = b"dissent/addr/v1";

/// Everything that travels on a community topic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GossipMsg {
    Event(SignedEvent),
    Addr(SignedAddr),
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
