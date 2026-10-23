//! [`CommunityRecord`] and its encoding as a signed Pkarr packet.
//!
//! Encoding: `postcard(record)` is split into chunks of at most 253 bytes. Each
//! chunk becomes one TXT character-string under the name `_dissent`, framed as
//! `[index, total, data...]`. Chunks are raw bytes, not base32, because base32
//! expands data by 8/5 and eight 90-char peers would overflow the 1000-byte packet.

use dissent_identity::{CommunityId, CommunityKey};
use pkarr::dns::rdata::{RData, TXT};
use pkarr::dns::{CharacterString, Name};
use pkarr::errors::SignedPacketBuildError;
use pkarr::SignedPacket;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

use crate::{DiscoveryError, Result};

/// Maximum peer addresses in one record.
pub const MAX_PEERS: usize = 8;
const RECORD_NAME: &str = "_dissent";
const TTL_SECS: u32 = 3600;
const CHUNK_HEADER: usize = 2;
const CHUNK_DATA: usize = 255 - CHUNK_HEADER;

/// What a community publishes under its own key on the Mainline DHT.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct CommunityRecord {
    pub name: String,
    /// Opaque `PeerAddr` strings from `dissent-net` (base32). At most [`MAX_PEERS`].
    pub peers: Vec<String>,
    /// Optional custom relay URLs.
    pub relays: Vec<Url>,
}

fn enc(e: impl std::fmt::Display) -> DiscoveryError {
    DiscoveryError::Encoding(e.to_string())
}

impl CommunityRecord {
    /// Encodes and signs the record with the community key.
    pub fn to_signed_packet(&self, key: &CommunityKey) -> Result<SignedPacket> {
        if self.peers.len() > MAX_PEERS {
            return Err(enc(format!(
                "{} peers exceeds maximum {MAX_PEERS}",
                self.peers.len()
            )));
        }
        let payload = postcard::to_allocvec(self).map_err(enc)?;
        let chunks: Vec<&[u8]> = payload.chunks(CHUNK_DATA).collect();
        let total = u8::try_from(chunks.len())
            .map_err(|_| DiscoveryError::RecordTooLarge(payload.len()))?;

        let mut builder = SignedPacket::builder();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut frame = Vec::with_capacity(CHUNK_HEADER + chunk.len());
            frame.push(i as u8);
            frame.push(total);
            frame.extend_from_slice(chunk);
            let name = Name::new(RECORD_NAME).map_err(enc)?;
            let text = TXT::new().with_char_string(CharacterString::new(&frame).map_err(enc)?);
            builder = builder.txt(name, text, TTL_SECS);
        }
        let keypair = pkarr::Keypair::from_secret_key(&key.0.seed());
        builder.sign(&keypair).map_err(|e| match e {
            SignedPacketBuildError::PacketTooLarge(n) => DiscoveryError::RecordTooLarge(n),
            other => DiscoveryError::Pkarr(other.to_string()),
        })
    }

    /// Decodes a record from a packet, without checking which key signed it.
    pub fn from_signed_packet(p: &SignedPacket) -> Result<Self> {
        let mut chunks: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        let mut total: Option<u8> = None;
        for rr in p.resource_records(RECORD_NAME) {
            let RData::TXT(txt) = &rr.rdata else {
                continue;
            };
            // iter_raw splits each string at its first '=', so rejoin it to recover the exact bytes.
            for (key, value) in txt.iter_raw() {
                let mut frame = key.to_vec();
                if let Some(v) = value {
                    frame.push(b'=');
                    frame.extend_from_slice(v);
                }
                let [index, count, data @ ..] = frame.as_slice() else {
                    return Err(enc("chunk shorter than header"));
                };
                if *total.get_or_insert(*count) != *count || index >= count {
                    return Err(enc("inconsistent chunk framing"));
                }
                if chunks.insert(*index, data.to_vec()).is_some() {
                    return Err(enc("duplicate chunk"));
                }
            }
        }
        let total = total.ok_or_else(|| enc("packet has no _dissent records"))?;
        let mut payload = Vec::new();
        for i in 0..total {
            let part = chunks
                .remove(&i)
                .ok_or_else(|| enc(format!("missing chunk {i}/{total}")))?;
            payload.extend_from_slice(&part);
        }
        postcard::from_bytes(&payload).map_err(enc)
    }

    /// Decodes a record and checks the packet was signed by `id`.
    pub fn from_signed_packet_for(p: &SignedPacket, id: &CommunityId) -> Result<Self> {
        if p.public_key().to_bytes() != *id.as_bytes() {
            return Err(enc("packet is signed by a different community"));
        }
        Self::from_signed_packet(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(peers: usize, peer_len: usize) -> CommunityRecord {
        CommunityRecord {
            name: "Rustaceans".into(),
            peers: (0..peers)
                .map(|i| format!("{i:0>width$}", width = peer_len))
                .collect(),
            relays: vec!["https://relay.example.org/".parse().unwrap()],
        }
    }

    #[test]
    fn roundtrip_with_eight_long_peers() {
        let key = CommunityKey::generate();
        let rec = sample(MAX_PEERS, 90);
        let packet = rec
            .to_signed_packet(&key)
            .expect("8 x 90-char peers must fit");
        assert_eq!(CommunityRecord::from_signed_packet(&packet).unwrap(), rec);
        assert_eq!(
            CommunityRecord::from_signed_packet_for(&packet, &key.id()).unwrap(),
            rec
        );
    }

    #[test]
    fn roundtrip_empty_record() {
        let key = CommunityKey::generate();
        let rec = CommunityRecord::default();
        let packet = rec.to_signed_packet(&key).unwrap();
        assert_eq!(CommunityRecord::from_signed_packet(&packet).unwrap(), rec);
    }

    #[test]
    fn oversized_record_is_rejected() {
        let key = CommunityKey::generate();
        let err = sample(MAX_PEERS, 2000).to_signed_packet(&key).unwrap_err();
        assert!(matches!(err, DiscoveryError::RecordTooLarge(_)), "{err:?}");
    }

    #[test]
    fn too_many_peers_is_rejected() {
        let key = CommunityKey::generate();
        let err = sample(MAX_PEERS + 1, 10)
            .to_signed_packet(&key)
            .unwrap_err();
        assert!(matches!(err, DiscoveryError::Encoding(_)), "{err:?}");
    }

    #[test]
    fn packet_from_other_community_is_rejected() {
        let a = CommunityKey::generate();
        let b = CommunityKey::generate();
        let packet = sample(2, 20).to_signed_packet(&a).unwrap();
        let err = CommunityRecord::from_signed_packet_for(&packet, &b.id()).unwrap_err();
        assert!(matches!(err, DiscoveryError::Encoding(_)), "{err:?}");
    }
}
