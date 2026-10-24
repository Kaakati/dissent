//! [`CommunityRecord`] and its encoding as a signed Pkarr packet, sealed for invite holders.
//!
//! The record is encrypted under a key derived from the community's current secret, and the packet
//! is signed by a key derived from that secret too. A stranger who knows only the community id
//! cannot find the packet, and could not read it if they did. The community key signs the record
//! inside the ciphertext, so a holder of the secret cannot forge a record for the community.
//!
//! Encoding: `XChaCha20-Poly1305(postcard((record, community signature)))`, prefixed by its 24-byte
//! nonce and bound to the community id. The bytes are split into chunks of at most 253 bytes. Each
//! chunk becomes one TXT character-string under the name `_dissent`, framed as
//! `[index, total, data...]`. Chunks are raw bytes, not base32, because base32 expands data by 8/5
//! and eight 90-char peers would overflow the 1000-byte packet.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use dissent_identity::{CommunityId, CommunityKey, SecretKey, Signature};
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
const NONCE_LEN: usize = 24;
const KEY_CONTEXT: &str = "dissent/community-record-key/v1";
const DHT_CONTEXT: &str = "dissent/community-dht-key/v1";
const SIG_DOMAIN: &[u8] = b"dissent/community-record-sig/v1";

/// What a community publishes on the Mainline DHT, for holders of its invite.
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

/// The secret and community id together, the input to every derivation here.
fn material(secret: &[u8; 32], id: &CommunityId) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(secret);
    out[32..].copy_from_slice(id.as_bytes());
    out
}

/// The Pkarr keypair that signs the packet for `id` under `secret`. Its public key is the DHT key.
pub fn dht_keypair(secret: &[u8; 32], id: &CommunityId) -> pkarr::Keypair {
    let seed = blake3::derive_key(DHT_CONTEXT, &material(secret, id));
    pkarr::Keypair::from_secret_key(&seed)
}

/// The DHT key for `id` under `secret`, as the public key a resolver asks for.
pub fn dht_public_key(secret: &[u8; 32], id: &CommunityId) -> pkarr::PublicKey {
    dht_keypair(secret, id).public_key()
}

fn seal(secret: &[u8; 32], id: &CommunityId, plain: &[u8]) -> Result<Vec<u8>> {
    let key = blake3::derive_key(KEY_CONTEXT, &material(secret, id));
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let random = SecretKey::generate().seed();
    let nonce = &random[..NONCE_LEN];
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plain,
                aad: id.as_bytes(),
            },
        )
        .map_err(|_| enc("could not seal the record"))?;
    let mut out = nonce.to_vec();
    out.extend(ciphertext);
    Ok(out)
}

fn open(secret: &[u8; 32], id: &CommunityId, sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < NONCE_LEN {
        return Err(enc("sealed record is too short"));
    }
    let key = blake3::derive_key(KEY_CONTEXT, &material(secret, id));
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: id.as_bytes(),
            },
        )
        .map_err(|_| enc("record does not open with this secret"))
}

fn signed_bytes(rec: &[u8]) -> Vec<u8> {
    let mut out = SIG_DOMAIN.to_vec();
    out.extend_from_slice(rec);
    out
}

impl CommunityRecord {
    /// Seals and signs the record for holders of `secret`. The packet is signed by the DHT key
    /// derived from the secret.
    pub fn to_private_packet(&self, key: &CommunityKey, secret: &[u8; 32]) -> Result<SignedPacket> {
        if self.peers.len() > MAX_PEERS {
            return Err(enc(format!(
                "{} peers exceeds maximum {MAX_PEERS}",
                self.peers.len()
            )));
        }
        let rec_bytes = postcard::to_allocvec(self).map_err(enc)?;
        let sig: Signature = key.sign(&signed_bytes(&rec_bytes));
        let plain = postcard::to_allocvec(&(self, sig)).map_err(enc)?;
        let id = key.id();
        let sealed = seal(secret, &id, &plain)?;

        let chunks: Vec<&[u8]> = sealed.chunks(CHUNK_DATA).collect();
        let total = u8::try_from(chunks.len())
            .map_err(|_| DiscoveryError::RecordTooLarge(sealed.len()))?;
        // All chunks share one TXT record. Each chunk is its own character-string, and a record
        // header costs more than the framing it would save.
        let mut text = TXT::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut frame = Vec::with_capacity(CHUNK_HEADER + chunk.len());
            frame.push(i as u8);
            frame.push(total);
            frame.extend_from_slice(chunk);
            text = text.with_char_string(CharacterString::new(&frame).map_err(enc)?.into_owned());
        }
        let name = Name::new(RECORD_NAME).map_err(enc)?;
        let builder = SignedPacket::builder().txt(name, text, TTL_SECS);
        builder
            .sign(&dht_keypair(secret, &id))
            .map_err(|e| match e {
                SignedPacketBuildError::PacketTooLarge(n) => DiscoveryError::RecordTooLarge(n),
                other => DiscoveryError::Pkarr(other.to_string()),
            })
    }

    /// Opens a packet with `secret` and checks the record is signed by the community `id`.
    pub fn from_private_packet(p: &SignedPacket, id: &CommunityId, secret: &[u8; 32]) -> Result<Self> {
        let sealed = reassemble(p)?;
        let plain = open(secret, id, &sealed)?;
        let (rec, sig): (CommunityRecord, Signature) = postcard::from_bytes(&plain).map_err(enc)?;
        let rec_bytes = postcard::to_allocvec(&rec).map_err(enc)?;
        id.verify(&signed_bytes(&rec_bytes), &sig)
            .map_err(|_| enc("record is not signed by the community"))?;
        Ok(rec)
    }
}

/// Joins the TXT chunks of a packet back into the sealed bytes.
fn reassemble(p: &SignedPacket) -> Result<Vec<u8>> {
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
    Ok(payload)
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

    const SECRET: [u8; 32] = [7; 32];

    #[test]
    fn roundtrip_with_eight_long_peers() {
        let key = CommunityKey::generate();
        let rec = sample(MAX_PEERS, 90);
        let packet = rec
            .to_private_packet(&key, &SECRET)
            .expect("8 x 90-char peers must fit");
        assert_eq!(
            CommunityRecord::from_private_packet(&packet, &key.id(), &SECRET).unwrap(),
            rec
        );
    }

    #[test]
    fn roundtrip_empty_record() {
        let key = CommunityKey::generate();
        let rec = CommunityRecord::default();
        let packet = rec.to_private_packet(&key, &SECRET).unwrap();
        assert_eq!(
            CommunityRecord::from_private_packet(&packet, &key.id(), &SECRET).unwrap(),
            rec
        );
    }

    #[test]
    fn the_name_is_not_in_clear() {
        let key = CommunityKey::generate();
        let packet = sample(2, 20).to_private_packet(&key, &SECRET).unwrap();
        let wire = format!("{packet:?}");
        assert!(!wire.contains("Rustaceans"), "the community name is sealed");
    }

    #[test]
    fn a_wrong_secret_cannot_open_the_record() {
        let key = CommunityKey::generate();
        let packet = sample(2, 20).to_private_packet(&key, &SECRET).unwrap();
        let err = CommunityRecord::from_private_packet(&packet, &key.id(), &[9; 32]).unwrap_err();
        assert!(matches!(err, DiscoveryError::Encoding(_)), "{err:?}");
    }

    #[test]
    fn a_record_for_another_community_is_rejected() {
        let a = CommunityKey::generate();
        let b = CommunityKey::generate();
        let packet = sample(2, 20).to_private_packet(&a, &SECRET).unwrap();
        assert!(CommunityRecord::from_private_packet(&packet, &b.id(), &SECRET).is_err());
    }

    #[test]
    fn a_record_signed_by_a_secret_holder_is_rejected() {
        // A holder of the secret can publish under the derived key, but cannot sign for the
        // community. The record must fail the community signature check.
        let community = CommunityKey::generate();
        let impostor = CommunityKey::generate();
        let packet = sample(2, 20).to_private_packet(&impostor, &SECRET).unwrap();
        assert!(CommunityRecord::from_private_packet(&packet, &community.id(), &SECRET).is_err());
    }

    #[test]
    fn a_rotated_secret_opens_the_new_record_and_not_the_old_one() {
        let key = CommunityKey::generate();
        let rec = sample(2, 20);
        let rotated = [9u8; 32];
        let packet = rec.to_private_packet(&key, &rotated).unwrap();
        assert_eq!(
            CommunityRecord::from_private_packet(&packet, &key.id(), &rotated).unwrap(),
            rec
        );
        assert!(CommunityRecord::from_private_packet(&packet, &key.id(), &SECRET).is_err());
    }

    #[test]
    fn oversized_record_is_rejected() {
        let key = CommunityKey::generate();
        let err = sample(MAX_PEERS, 2000)
            .to_private_packet(&key, &SECRET)
            .unwrap_err();
        assert!(matches!(err, DiscoveryError::RecordTooLarge(_)), "{err:?}");
    }

    #[test]
    fn too_many_peers_is_rejected() {
        let key = CommunityKey::generate();
        let err = sample(MAX_PEERS + 1, 10)
            .to_private_packet(&key, &SECRET)
            .unwrap_err();
        assert!(matches!(err, DiscoveryError::Encoding(_)), "{err:?}");
    }
}
