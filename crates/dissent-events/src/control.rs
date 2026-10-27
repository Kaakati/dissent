//! Sealed control payloads. A control event's payload is encrypted under the community secret of
//! the generation it was made in. The event's id and signature cover the ciphertext, so relays and
//! DHT nodes verify authorship without a key. The header (community, channel, author, device,
//! parents, hlc and generation) is bound as associated data, so the ciphertext cannot be moved to
//! another event. Replay opens the payload with a ring of `(generation, secret)` keys before it
//! applies the event. See `docs/design/06-threat-model.md`, finding 2.

use std::borrow::Cow;

use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use dissent_identity::{CommunityId, DeviceId, RootId};
use serde::{Deserialize, Serialize};

use crate::{ChannelId, Error, Event, EventBody, EventId, Hlc, Payload, Result, SealedKind};

const KEY_DOMAIN: &str = "dissent/control-seal/v1";
const AAD_DOMAIN: &[u8] = b"dissent/control-aad/v1";
const MEMBER_AAD_DOMAIN: &[u8] = b"dissent/member-control-aad/v1";
const MEMBER_ENC_DOMAIN: &str = "dissent/member-control-enc/v1";
const MEMBER_WRITE_DOMAIN: &str = "dissent/member-control-write/v1";
const TAG_DOMAIN: &[u8] = b"dissent/member-control-tag/v1";
const NONCE_LEN: usize = 24;

/// The generation a carried payload is filed under. No community secret has this generation, so a
/// client that predates a carrier kind finds no key and shows the event as locked, rather than
/// failing to decode it. Current clients unwrap the kind inside. See `CARRIED_MEMBER`.
pub const CARRIER_GENERATION: u64 = u64::MAX;

/// Carrier kind of a member-sealed control payload. The body is a postcard `MemberBody`.
const CARRIED_MEMBER: u8 = 1;

/// Carrier kind of a private-channel message. The body is a postcard `PrivateBody`. Replay opens no key
/// for it, since the kind and generation are clear.
const CARRIED_PRIVATE: u8 = 3;

/// Carrier kind of a private channel's access event. The body is a postcard `Payload`, which is the
/// community-secret seal of `ChannelCreatePrivate` or `ChannelAccess`: the same generation and seal as other
/// control. Only a holder of that generation's secret can open it, so a banned member, after a rotation,
/// cannot read a private channel's name or readers. See `carry_sealed`.
const CARRIED_SEALED: u8 = 4;

/// Wraps a community-secret seal of a private channel's access event in the carrier, so an older client shows
/// it as locked rather than failing the batch it is in. `sealed` must be a payload from `seal`.
pub fn carry_sealed(sealed: &Payload) -> Result<Payload> {
    let Payload::SealedControl { generation, .. } = sealed else {
        return Err(Error::Encoding("only a sealed control payload is carried".into()));
    };
    if *generation == CARRIER_GENERATION {
        return Err(Error::Encoding("a carrier cannot carry a carrier".into()));
    }
    let body = postcard::to_allocvec(sealed).map_err(|e| Error::Encoding(e.to_string()))?;
    let carried = postcard::to_allocvec(&Carried {
        kind: CARRIED_SEALED,
        body,
    })
    .map_err(|e| Error::Encoding(e.to_string()))?;
    Ok(Payload::SealedControl {
        generation: CARRIER_GENERATION,
        ciphertext: carried,
    })
}

/// Opens a carried private-channel access event with the ring, as the body it was sealed for. Only
/// `ChannelCreatePrivate` and `ChannelAccess` are accepted, so the carrier cannot carry another kind.
fn open_sealed_carried(body: &EventBody, ciphertext: &[u8], ring: &[SealKey]) -> std::result::Result<Payload, OpenError> {
    let carried = carried(ciphertext).ok_or(OpenError::Forged)?;
    if carried.kind != CARRIED_SEALED {
        return Err(OpenError::Forged);
    }
    let inner: Payload = postcard::from_bytes(&carried.body).map_err(|_| OpenError::Forged)?;
    let Payload::SealedControl { generation, .. } = &inner else {
        return Err(OpenError::Forged);
    };
    if *generation == CARRIER_GENERATION {
        return Err(OpenError::Forged);
    }
    let mut sealed = body.clone();
    sealed.payload = inner;
    let payload = open(ring, &sealed)?;
    match payload {
        Payload::ChannelCreatePrivate { .. } | Payload::ChannelAccess { .. } => Ok(payload),
        _ => Err(OpenError::Forged),
    }
}

/// A private-channel message's clear fields and its sealed text.
#[derive(Serialize, Deserialize)]
struct PrivateBody {
    kind: SealedKind,
    generation: u64,
    ciphertext: Vec<u8>,
}

/// The carried kind and body of a carrier payload, or `None` when the bytes are not a carrier.
fn carried(ciphertext: &[u8]) -> Option<Carried> {
    postcard::from_bytes(ciphertext).ok()
}

/// Wraps a private-channel message in the carrier, so an older client shows it as locked. The caller
/// seals `ciphertext` under the channel key of `generation`. The result goes in the body before it is signed.
pub fn carry_private(kind: SealedKind, generation: u64, ciphertext: Vec<u8>) -> Result<Payload> {
    let body = postcard::to_allocvec(&PrivateBody {
        kind,
        generation,
        ciphertext,
    })
    .map_err(|e| Error::Encoding(e.to_string()))?;
    let carried = postcard::to_allocvec(&Carried {
        kind: CARRIED_PRIVATE,
        body,
    })
    .map_err(|e| Error::Encoding(e.to_string()))?;
    Ok(Payload::SealedControl {
        generation: CARRIER_GENERATION,
        ciphertext: carried,
    })
}

const PRIVATE_AAD_DOMAIN: &[u8] = b"dissent/private-text-aad/v1";

/// The associated data a private message's text is sealed with. It binds the community, the channel,
/// the key generation and the author, so the text cannot be moved to another channel or generation.
pub fn private_aad(community: &CommunityId, channel: &ChannelId, generation: u64, author: &RootId) -> Vec<u8> {
    let mut out = PRIVATE_AAD_DOMAIN.to_vec();
    out.extend_from_slice(community.as_bytes());
    out.extend_from_slice(&channel.0);
    out.extend_from_slice(&generation.to_be_bytes());
    out.extend_from_slice(author.as_bytes());
    out
}

/// Seals a private message's plaintext under the channel key of its generation: a fresh nonce, then the
/// AEAD ciphertext. `aad` comes from `private_aad`.
pub fn seal_private_text(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; NONCE_LEN] = rand::random();
    let body = XChaCha20Poly1305::new(key.into())
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: plain,
                aad,
            },
        )
        .map_err(|_| Error::Encoding("could not seal the private message".into()))?;
    Ok([nonce.as_slice(), body.as_slice()].concat())
}

/// Opens a private message's ciphertext with the channel key. `None` when the key is wrong or the
/// ciphertext was changed.
pub fn open_private_text(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, body) = data.split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: body,
                aad,
            },
        )
        .ok()
}

/// Opens a carried private-channel message to its replayed form. No key is needed.
fn open_private(ciphertext: &[u8]) -> std::result::Result<Payload, OpenError> {
    let carried = carried(ciphertext).ok_or(OpenError::Forged)?;
    if carried.kind != CARRIED_PRIVATE {
        return Err(OpenError::Forged);
    }
    let body: PrivateBody = postcard::from_bytes(&carried.body).map_err(|_| OpenError::Forged)?;
    Ok(Payload::PrivateSealed {
        kind: body.kind,
        generation: body.generation,
        ciphertext: body.ciphertext,
    })
}

/// A payload of a kind older clients cannot decode, wrapped so they can still decode the event.
#[derive(Serialize, Deserialize)]
struct Carried {
    kind: u8,
    body: Vec<u8>,
}

/// A member-sealed payload's fields, as carried.
#[derive(Serialize, Deserialize)]
struct MemberBody {
    epoch: u64,
    ciphertext: Vec<u8>,
    tag: [u8; 32],
}

/// The parts of a member-sealed control payload: its epoch, ciphertext and tag. Recognises the
/// carrier form and the form v0.1.6 wrote, so both replay.
pub fn member_parts(payload: &Payload) -> Option<(u64, Vec<u8>, [u8; 32])> {
    match payload {
        Payload::MemberSealedControl {
            epoch,
            ciphertext,
            tag,
        } => Some((*epoch, ciphertext.clone(), *tag)),
        Payload::SealedControl {
            generation: CARRIER_GENERATION,
            ciphertext,
        } => {
            let carried: Carried = postcard::from_bytes(ciphertext).ok()?;
            if carried.kind != CARRIED_MEMBER {
                return None;
            }
            let body: MemberBody = postcard::from_bytes(&carried.body).ok()?;
            Some((body.epoch, body.ciphertext, body.tag))
        }
        _ => None,
    }
}

/// Whether a payload is a member-sealed control payload, in either form.
pub fn is_member_sealed(payload: &Payload) -> bool {
    member_parts(payload).is_some()
}

/// The member-sealed payload for the given fields, in the carrier form that is written now.
pub fn member_sealed_payload(epoch: u64, ciphertext: Vec<u8>, tag: [u8; 32]) -> Result<Payload> {
    let body = postcard::to_allocvec(&MemberBody {
        epoch,
        ciphertext,
        tag,
    })
    .map_err(|e| Error::Encoding(e.to_string()))?;
    let carried = postcard::to_allocvec(&Carried {
        kind: CARRIED_MEMBER,
        body,
    })
    .map_err(|e| Error::Encoding(e.to_string()))?;
    Ok(Payload::SealedControl {
        generation: CARRIER_GENERATION,
        ciphertext: carried,
    })
}

/// The MLS exporter label that control keys are drawn from. The keys of epoch `e` are derived from
/// `export(EXPORT_LABEL, e)` of the community group in that epoch.
pub const EXPORT_LABEL: &str = "dissent/control/v1";

/// A community secret with its generation. A ring of these opens every control event its holder
/// may read.
pub type SealKey = (u64, [u8; 32]);

/// The encryption key of one MLS epoch, with the epoch. A ring of these opens member-sealed control
/// events. See `EpochKeys`.
pub type EpochKey = (u64, [u8; 32]);

/// The keys of one MLS epoch, both derived from the epoch's exporter secret. `enc` opens the
/// member-sealed payloads of that epoch. `write` only computes and checks tags, so it cannot open
/// anything. A storage member holds `write`, and never `enc`.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EpochKeys {
    pub enc: [u8; 32],
    pub write: [u8; 32],
}

impl EpochKeys {
    /// The keys of an epoch from its exporter output. Any length works, the output is hashed.
    pub fn from_export(export: &[u8]) -> Self {
        Self {
            enc: blake3::derive_key(MEMBER_ENC_DOMAIN, export),
            write: blake3::derive_key(MEMBER_WRITE_DOMAIN, export),
        }
    }
}

/// Why a sealed control event could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The ring holds no key of the event's generation. The holder may not have fetched it yet.
    NoKey,
    /// A key of that generation exists, but the payload does not open under it, or what it opens to
    /// is not a control payload.
    Forged,
}

/// Whether a control payload is sealed when it is authored. Message content is sealed by MLS, and
/// reaction and delete targets stay in clear. Secret rotations carry keys, so they stay readable
/// without one. Only the membership, role, name and channel payloads are sealed here.
pub fn is_sealable(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::ChannelCreate { .. }
            | Payload::VoiceChannelCreate { .. }
            | Payload::RoleGrant { .. }
            | Payload::RoleRevoke { .. }
            | Payload::Ban { .. }
            | Payload::Unban { .. }
            | Payload::HandleClaim { .. }
            | Payload::Join { .. }
            | Payload::JoinAsStorage { .. }
            | Payload::Leave
            | Payload::KeyRotation(_)
            | Payload::DeviceRevoke(_)
            | Payload::SetRelays { .. }
            | Payload::ChannelUpdate { .. }
            | Payload::ChannelDelete { .. }
            | Payload::InviteCreate { .. }
            | Payload::InviteRevoke { .. }
            | Payload::JoinVia { .. }
            | Payload::ProfileUpdate { .. }
            | Payload::CommunityIconSet { .. }
            | Payload::MemberOnlyControl { .. }
            | Payload::HistoryVisibility { .. }
            | Payload::CommunityRename { .. }
            | Payload::CommunityBannerSet { .. }
            | Payload::CommunityProfileSet { .. }
            | Payload::ProfileBannerSet { .. }
            | Payload::ChannelAccess { .. }
            | Payload::ChannelCreatePrivate { .. }
    )
}

/// Whether a control payload is sealed under the epoch key while member-only control is on. Invite
/// events stay under the community secret, since a joiner reads them before it has a group, and
/// the invite a `JoinVia` names must be known to check the join. A rename stays there too: the
/// name is shown to every invite holder anyway, and the invite preview hands the owner's rename to
/// a joiner as proof of the name, which it opens with the invite secret. Everything else in
/// `is_sealable` goes under the epoch key.
pub fn is_member_sealable(payload: &Payload) -> bool {
    is_sealable(payload)
        && !matches!(
            payload,
            Payload::InviteCreate { .. } | Payload::InviteRevoke { .. } | Payload::CommunityRename { .. }
        )
}

/// The header a payload is sealed against. Parents are sorted and deduplicated, as `Event::sign`
/// stores them, so the same header is built on both sides.
#[derive(Serialize)]
struct Header<'a> {
    community: CommunityId,
    channel: ChannelId,
    author: RootId,
    device: DeviceId,
    parents: &'a [EventId],
    hlc: Hlc,
    generation: u64,
}

fn aad(body: &EventBody, generation: u64) -> Result<Vec<u8>> {
    aad_under(AAD_DOMAIN, body, generation)
}

/// The associated data for a payload sealed under `domain`, with `number` as its generation or epoch.
fn aad_under(domain: &[u8], body: &EventBody, number: u64) -> Result<Vec<u8>> {
    let mut parents = body.parents.clone();
    parents.sort_unstable();
    parents.dedup();
    let header = Header {
        community: body.community,
        channel: body.channel,
        author: body.author,
        device: body.device,
        parents: &parents,
        hlc: body.hlc,
        generation: number,
    };
    let mut out = domain.to_vec();
    out.extend(postcard::to_allocvec(&header).map_err(|e| Error::Encoding(e.to_string()))?);
    Ok(out)
}

fn cipher(secret: &[u8; 32]) -> XChaCha20Poly1305 {
    let key = blake3::derive_key(KEY_DOMAIN, secret);
    XChaCha20Poly1305::new(Key::from_slice(&key))
}

fn member_cipher(enc: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(Key::from_slice(enc))
}

/// The write tag over the associated data and the ciphertext, each length-prefixed.
fn member_tag(write: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> [u8; 32] {
    let mut input = TAG_DOMAIN.to_vec();
    input.extend((aad.len() as u64).to_le_bytes());
    input.extend(aad);
    input.extend((ciphertext.len() as u64).to_le_bytes());
    input.extend(ciphertext);
    *blake3::keyed_hash(write, &input).as_bytes()
}

/// Seals `body.payload`, which is plaintext here, under `secret` of `generation`. The caller
/// stores the returned payload in the body before it signs the event.
pub fn seal(secret: &[u8; 32], generation: u64, body: &EventBody) -> Result<Payload> {
    if !is_sealable(&body.payload) {
        return Err(Error::Encoding(
            "this payload is not sealed as control".into(),
        ));
    }
    let plain = postcard::to_allocvec(&body.payload).map_err(|e| Error::Encoding(e.to_string()))?;
    let nonce: [u8; NONCE_LEN] = rand::random();
    let sealed = cipher(secret)
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: &plain,
                aad: &aad(body, generation)?,
            },
        )
        .map_err(|_| Error::Encoding("could not seal the control payload".into()))?;
    let mut ciphertext = nonce.to_vec();
    ciphertext.extend(sealed);
    Ok(Payload::SealedControl {
        generation,
        ciphertext,
    })
}

/// Opens the sealed payload of `body` with the keys in `ring`. Only keys of the payload's own
/// generation are tried.
pub fn open(ring: &[SealKey], body: &EventBody) -> std::result::Result<Payload, OpenError> {
    let Payload::SealedControl {
        generation,
        ciphertext,
    } = &body.payload
    else {
        return Err(OpenError::Forged);
    };
    if ciphertext.len() < NONCE_LEN {
        return Err(OpenError::Forged);
    }
    let aad = aad(body, *generation).map_err(|_| OpenError::Forged)?;
    let mut keys = ring.iter().filter(|(g, _)| g == generation).peekable();
    if keys.peek().is_none() {
        return Err(OpenError::NoKey);
    }
    let (nonce, sealed) = ciphertext.split_at(NONCE_LEN);
    for (_, secret) in keys {
        let Ok(plain) = cipher(secret).decrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: sealed,
                aad: &aad,
            },
        ) else {
            continue;
        };
        let payload: Payload = postcard::from_bytes(&plain).map_err(|_| OpenError::Forged)?;
        return if is_sealable(&payload) {
            Ok(payload)
        } else {
            Err(OpenError::Forged)
        };
    }
    Err(OpenError::Forged)
}

/// The event as replay and the engine read it. A sealed control event comes back with its
/// plaintext payload. Every other event is borrowed as it is. The id is still the sealed event's
/// id, so the result must only be read, never stored or hashed.
pub fn open_event<'a>(
    ev: &'a Event,
    ring: &[SealKey],
) -> std::result::Result<Cow<'a, Event>, OpenError> {
    open_event_keyed(ev, ring, &[])
}

/// Seals `body.payload` under the encryption key of `epoch`, and tags it with the epoch's write key.
/// The caller stores the returned payload in the body before it signs the event.
pub fn seal_member(keys: &EpochKeys, epoch: u64, body: &EventBody) -> Result<Payload> {
    if !is_sealable(&body.payload) {
        return Err(Error::Encoding(
            "this payload is not sealed as control".into(),
        ));
    }
    let plain = postcard::to_allocvec(&body.payload).map_err(|e| Error::Encoding(e.to_string()))?;
    let nonce: [u8; NONCE_LEN] = rand::random();
    let aad = aad_under(MEMBER_AAD_DOMAIN, body, epoch)?;
    let sealed = member_cipher(&keys.enc)
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: &plain,
                aad: &aad,
            },
        )
        .map_err(|_| Error::Encoding("could not seal the control payload".into()))?;
    let mut ciphertext = nonce.to_vec();
    ciphertext.extend(sealed);
    let tag = member_tag(&keys.write, &aad, &ciphertext);
    member_sealed_payload(epoch, ciphertext, tag)
}

/// Opens a member-sealed payload with the encryption keys in `ring`. Only the key of the payload's
/// own epoch is tried.
pub fn open_member(ring: &[EpochKey], body: &EventBody) -> std::result::Result<Payload, OpenError> {
    let Some((epoch, ciphertext, _)) = member_parts(&body.payload) else {
        return Err(OpenError::Forged);
    };
    if ciphertext.len() < NONCE_LEN {
        return Err(OpenError::Forged);
    }
    let Some((_, enc)) = ring.iter().find(|(e, _)| *e == epoch) else {
        return Err(OpenError::NoKey);
    };
    let aad = aad_under(MEMBER_AAD_DOMAIN, body, epoch).map_err(|_| OpenError::Forged)?;
    let (nonce, sealed) = ciphertext.split_at(NONCE_LEN);
    let plain = member_cipher(enc)
        .decrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: sealed,
                aad: &aad,
            },
        )
        .map_err(|_| OpenError::Forged)?;
    let payload: Payload = postcard::from_bytes(&plain).map_err(|_| OpenError::Forged)?;
    if is_sealable(&payload) {
        Ok(payload)
    } else {
        Err(OpenError::Forged)
    }
}

/// Checks the write tag of a member-sealed event with the write keys in `ring`, without opening it.
/// A storage member uses this to keep events that are well formed for a community, and drop the
/// rest. A tag made by anyone holding the write key passes, so this rejects junk, not forgeries
/// by a storage member.
pub fn verify_member_tag(ring: &[EpochKey], body: &EventBody) -> std::result::Result<(), OpenError> {
    let Some((epoch, ciphertext, tag)) = member_parts(&body.payload) else {
        return Err(OpenError::Forged);
    };
    let Some((_, write)) = ring.iter().find(|(e, _)| *e == epoch) else {
        return Err(OpenError::NoKey);
    };
    let aad = aad_under(MEMBER_AAD_DOMAIN, body, epoch).map_err(|_| OpenError::Forged)?;
    if blake3::Hash::from(tag) == blake3::Hash::from(member_tag(write, &aad, &ciphertext)) {
        Ok(())
    } else {
        Err(OpenError::Forged)
    }
}

/// [`open_event`] for both kinds of sealed control payload. `ring` holds the community secret
/// generations, and `epochs` the encryption key of each MLS epoch.
pub fn open_event_keyed<'a>(
    ev: &'a Event,
    ring: &[SealKey],
    epochs: &[EpochKey],
) -> std::result::Result<Cow<'a, Event>, OpenError> {
    let payload = match &ev.body.payload {
        Payload::SealedControl {
            generation: CARRIER_GENERATION,
            ciphertext,
        } => match carried(ciphertext) {
            Some(Carried { kind, .. }) if kind == CARRIED_MEMBER => open_member(epochs, &ev.body)?,
            Some(Carried { kind, .. }) if kind == CARRIED_PRIVATE => open_private(ciphertext)?,
            Some(Carried { kind, .. }) if kind == CARRIED_SEALED => open_sealed_carried(&ev.body, ciphertext, ring)?,
            // A kind this build does not know is refused as forged, not locked, so it does not look
            // like a key that is missing.
            _ => return Err(OpenError::Forged),
        },
        Payload::MemberSealedControl { .. } => open_member(epochs, &ev.body)?,
        Payload::SealedControl { .. } => open(ring, &ev.body)?,
        _ => return Ok(Cow::Borrowed(ev)),
    };
    let mut plain = ev.clone();
    plain.body.payload = payload;
    Ok(Cow::Owned(plain))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replayer::Replayer;
    use crate::state::replay_with;
    use crate::testutil::{event, User};
    use crate::{Dag, HistorySetting, Rejection, Role};

    const S0: [u8; 32] = [7; 32];
    const S1: [u8; 32] = [8; 32];
    const OTHER: [u8; 32] = [9; 32];

    fn at(wall_ms: u64) -> Hlc {
        Hlc {
            wall_ms,
            counter: 0,
        }
    }

    /// An event on the control channel by `u`, its payload sealed under `secret` of `generation`.
    fn sealed(
        u: &User,
        secret: &[u8; 32],
        generation: u64,
        parents: Vec<EventId>,
        wall_ms: u64,
        payload: Payload,
    ) -> Event {
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel: ChannelId::CONTROL,
            author: u.id(),
            device: u.device.id(),
            parents,
            hlc: at(wall_ms),
            payload,
        };
        let payload = seal(secret, generation, &body).unwrap();
        Event::sign(EventBody { payload, ..body }, &u.device)
    }

    fn join(name: &str) -> Payload {
        Payload::Join {
            display_name: name.into(),
        }
    }

    #[test]
    fn member_with_the_key_opens_the_payload() {
        let u = User::new();
        let ev = sealed(&u, &S0, 0, vec![], 1, join("alice"));
        assert!(matches!(
            ev.body.payload,
            Payload::SealedControl { generation: 0, .. }
        ));
        let plain = open_event(&ev, &[(0, S0)]).unwrap();
        assert_eq!(plain.body.payload, join("alice"));
        assert_eq!(plain.id, ev.id);
    }

    #[test]
    fn outsider_without_the_key_sees_no_plaintext() {
        let u = User::new();
        let ev = sealed(&u, &S0, 0, vec![], 1, join("alice-secret-name"));
        let bytes = postcard::to_allocvec(&ev).unwrap();
        assert!(!bytes
            .windows(b"alice-secret-name".len())
            .any(|w| w == b"alice-secret-name"));
        assert_eq!(open_event(&ev, &[]).unwrap_err(), OpenError::NoKey);
        assert_eq!(open_event(&ev, &[(1, S0)]).unwrap_err(), OpenError::NoKey);
        assert_eq!(
            open_event(&ev, &[(0, OTHER)]).unwrap_err(),
            OpenError::Forged
        );
    }

    #[test]
    fn header_is_bound_as_associated_data() {
        let u = User::new();
        let ev = sealed(&u, &S0, 0, vec![], 1, join("alice"));
        // The same ciphertext under a different hlc is not the same event.
        let mut moved = ev.clone();
        moved.body.hlc = at(2);
        assert_eq!(
            open_event(&moved, &[(0, S0)]).unwrap_err(),
            OpenError::Forged
        );
        // Nor under another generation claimed in clear.
        let mut relabelled = ev.clone();
        relabelled.body.payload = match ev.body.payload.clone() {
            Payload::SealedControl { ciphertext, .. } => Payload::SealedControl {
                generation: 1,
                ciphertext,
            },
            other => other,
        };
        assert_eq!(
            open_event(&relabelled, &[(1, S1)]).unwrap_err(),
            OpenError::Forged
        );
    }

    #[test]
    fn tampered_ciphertext_is_refused_at_insert_and_fails_to_open() {
        let u = User::new();
        let ev = sealed(&u, &S0, 0, vec![], 1, join("alice"));
        // A relay flips a byte. The id no longer matches the body, so the DAG refuses it.
        let mut relay_edit = ev.clone();
        if let Payload::SealedControl { ciphertext, .. } = &mut relay_edit.body.payload {
            let last = ciphertext.len() - 1;
            ciphertext[last] ^= 1;
        }
        assert!(matches!(
            Dag::new().insert(relay_edit.clone()),
            Err(Error::IdMismatch)
        ));
        // The author re-signs the altered body. The signature holds, but the AEAD tag does not, so
        // the payload stays closed and replay rejects the event as forged.
        let resigned = Event::sign(relay_edit.body.clone(), &u.device);
        assert!(resigned.verify(&u.cert).is_ok());
        assert_eq!(
            open_event(&resigned, &[(0, S0)]).unwrap_err(),
            OpenError::Forged
        );
        let mut dag = Dag::new();
        dag.insert(resigned.clone()).unwrap();
        let st = replay_with(u.id(), &dag, &[(0, S0)]);
        assert_eq!(st.rejected, vec![(resigned.id, Rejection::Forged)]);
    }

    #[test]
    fn a_sealed_payload_cannot_hide_another_sealed_payload() {
        let u = User::new();
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel: ChannelId::CONTROL,
            author: u.id(),
            device: u.device.id(),
            parents: vec![],
            hlc: at(1),
            payload: Payload::Leave,
        };
        let inner = seal(&S0, 0, &body).unwrap();
        // Sealing refuses a payload that is already sealed.
        let nested = EventBody {
            payload: inner.clone(),
            ..body.clone()
        };
        assert!(seal(&S0, 0, &nested).is_err());
        // Nor does an encryption of a sealed payload open as one.
        let forged = Event::sign(
            EventBody {
                payload: forge(&S0, 0, &body, &inner),
                ..body
            },
            &u.device,
        );
        assert_eq!(
            open_event(&forged, &[(0, S0)]).unwrap_err(),
            OpenError::Forged
        );
    }

    /// Seals the postcard bytes of a payload directly, which `seal` refuses to do for sealed ones.
    fn forge(secret: &[u8; 32], generation: u64, body: &EventBody, inner: &Payload) -> Payload {
        let plain = postcard::to_allocvec(inner).unwrap();
        let nonce = [1u8; NONCE_LEN];
        let sealed = cipher(secret)
            .encrypt(
                XNonce::from_slice(&nonce),
                AeadPayload {
                    msg: &plain,
                    aad: &aad(body, generation).unwrap(),
                },
            )
            .unwrap();
        let mut ciphertext = nonce.to_vec();
        ciphertext.extend(sealed);
        Payload::SealedControl {
            generation,
            ciphertext,
        }
    }

    #[test]
    fn plain_control_events_pass_through_unchanged() {
        let u = User::new();
        let ev = event(&u, vec![], at(1), Payload::Leave);
        let view = open_event(&ev, &[]).unwrap();
        assert!(matches!(view, Cow::Borrowed(_)));
        assert_eq!(view.body.payload, Payload::Leave);
    }

    /// The owner's chain: a sealed `Join` under generation 0, then a sealed `ChannelCreate` under
    /// generation 1, which a rotation after a ban made current.
    fn owner_chain(owner: &User) -> Dag {
        let join_ev = sealed(owner, &S0, 0, vec![], 1, join("boss"));
        let chan = sealed(
            owner,
            &S1,
            1,
            vec![join_ev.id],
            2,
            Payload::ChannelCreate {
                name: "secret-room".into(),
            },
        );
        let mut dag = Dag::new();
        dag.insert(join_ev).unwrap();
        dag.insert(chan).unwrap();
        dag
    }

    #[test]
    fn outsider_replay_names_no_members_or_channels() {
        let owner = User::new();
        let dag = owner_chain(&owner);
        let st = replay_with(owner.id(), &dag, &[]);
        assert!(st.channels.is_empty());
        assert!(st.members.is_empty());
        assert_eq!(st.rejected.len(), 2);
        assert!(st.rejected.iter().all(|(_, why)| *why == Rejection::Locked));
    }

    #[test]
    fn member_replay_reads_names_and_channels() {
        let owner = User::new();
        let dag = owner_chain(&owner);
        let st = replay_with(owner.id(), &dag, &[(0, S0), (1, S1)]);
        assert!(st.rejected.is_empty());
        assert_eq!(
            st.channels.values().collect::<Vec<_>>(),
            vec!["secret-room"]
        );
        assert_eq!(st.members[&owner.id()].display_name, "boss");
    }

    #[test]
    fn member_who_joined_after_a_rotation_reads_older_events() {
        let owner = User::new();
        let dag = owner_chain(&owner);
        let join_id = dag.causal_order()[0].id;
        // A joiner holding the current secret only: the generation 0 Join stays locked.
        let st = replay_with(owner.id(), &dag, &[(1, S1)]);
        assert_eq!(st.rejected, vec![(join_id, Rejection::Locked)]);
        assert_eq!(
            st.channels.values().collect::<Vec<_>>(),
            vec!["secret-room"]
        );
        // With the retained generation 0 secret, the older event opens too.
        let st = replay_with(owner.id(), &dag, &[(1, S1), (0, S0)]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.members[&owner.id()].display_name, "boss");
    }

    #[test]
    fn banned_member_cannot_read_events_after_the_ban() {
        let owner = User::new();
        let claim = sealed(
            &owner,
            &S1,
            1,
            vec![],
            5,
            Payload::HandleClaim {
                handle: "after-ban".into(),
            },
        );
        // A banned member kept generation 0 only, so the rotation to generation 1 is out of reach.
        assert_eq!(
            open_event(&claim, &[(0, S0)]).unwrap_err(),
            OpenError::NoKey
        );
        let mut dag = Dag::new();
        dag.insert(claim).unwrap();
        let locked = replay_with(owner.id(), &dag, &[(0, S0)]);
        assert!(locked.handles.is_empty());
        let open = replay_with(owner.id(), &dag, &[(1, S1)]);
        assert!(open.handles.contains_key("after-ban"));
    }

    #[test]
    fn replayer_rebuilds_when_the_key_arrives() {
        let owner = User::new();
        let dag = owner_chain(&owner);
        let mut rep = Replayer::new(owner.id());
        let ids: Vec<EventId> = dag.causal_order().iter().map(|e| e.id).collect();
        rep.on_insert(&dag, &ids);
        assert_eq!(rep.state().rejected.len(), 2);
        let ring = vec![(0, S0), (1, S1)];
        rep.set_ring(&dag, ring.clone());
        let fresh = replay_with(owner.id(), &dag, &ring);
        assert_eq!(rep.state().rejected, fresh.rejected);
        assert_eq!(rep.state().channels, fresh.channels);
        assert_eq!(rep.state().members[&owner.id()].display_name, "boss");
    }

    #[test]
    fn old_plaintext_control_events_still_replay() {
        let owner = User::new();
        let join_ev = event(&owner, vec![], at(1), join("boss"));
        let chan = event(
            &owner,
            vec![join_ev.id],
            at(2),
            Payload::ChannelCreate {
                name: "general".into(),
            },
        );
        let mut dag = Dag::new();
        dag.insert(join_ev).unwrap();
        dag.insert(chan).unwrap();
        // With no keys at all, a plaintext DAG replays in full.
        let st = replay_with(owner.id(), &dag, &[]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.channels.values().collect::<Vec<_>>(), vec!["general"]);
        assert_eq!(st.members[&owner.id()].display_name, "boss");
    }

    fn epoch_keys(export: u8) -> EpochKeys {
        EpochKeys::from_export(&[export; 32])
    }

    /// An event by `u` on the control channel, its payload sealed for `epoch` under `keys`.
    fn member_sealed(
        u: &User,
        keys: &EpochKeys,
        epoch: u64,
        parents: Vec<EventId>,
        wall_ms: u64,
        payload: Payload,
    ) -> Event {
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel: ChannelId::CONTROL,
            author: u.id(),
            device: u.device.id(),
            parents,
            hlc: at(wall_ms),
            payload,
        };
        let payload = seal_member(keys, epoch, &body).unwrap();
        Event::sign(EventBody { payload, ..body }, &u.device)
    }

    #[test]
    fn member_of_the_epoch_opens_the_payload() {
        let u = User::new();
        let keys = epoch_keys(1);
        let ev = member_sealed(&u, &keys, 4, vec![], 1, join("alice"));
        // Written in the carrier form: a generation no secret has, so a client without the carrier
        // reads it as locked rather than failing to decode it.
        assert!(matches!(
            ev.body.payload,
            Payload::SealedControl {
                generation: CARRIER_GENERATION,
                ..
            }
        ));
        assert_eq!(member_parts(&ev.body.payload).map(|p| p.0), Some(4));
        let plain = open_event_keyed(&ev, &[], &[(4, keys.enc)]).unwrap();
        assert_eq!(plain.body.payload, join("alice"));
        assert_eq!(plain.id, ev.id);
    }

    #[test]
    fn member_payload_hides_the_name_and_needs_its_epoch() {
        let u = User::new();
        let keys = epoch_keys(2);
        let ev = member_sealed(&u, &keys, 0, vec![], 1, join("hidden-name"));
        let bytes = postcard::to_allocvec(&ev).unwrap();
        assert!(!bytes.windows(b"hidden-name".len()).any(|w| w == b"hidden-name"));
        assert_eq!(open_event(&ev, &[]).unwrap_err(), OpenError::NoKey);
        // A key of another epoch, or another epoch's key, does not open it.
        assert_eq!(
            open_event_keyed(&ev, &[], &[(1, keys.enc)]).unwrap_err(),
            OpenError::NoKey
        );
        assert_eq!(
            open_event_keyed(&ev, &[], &[(0, epoch_keys(3).enc)]).unwrap_err(),
            OpenError::Forged
        );
    }

    #[test]
    fn write_key_tags_but_cannot_open() {
        let u = User::new();
        let keys = epoch_keys(4);
        let ev = member_sealed(&u, &keys, 0, vec![], 1, join("alice"));
        // A storage member holds the write key only. It checks the tag, and it cannot decrypt.
        assert!(verify_member_tag(&[(0, keys.write)], &ev.body).is_ok());
        assert_eq!(
            open_event_keyed(&ev, &[], &[(0, keys.write)]).unwrap_err(),
            OpenError::Forged
        );
        assert_eq!(
            verify_member_tag(&[(0, keys.enc)], &ev.body).unwrap_err(),
            OpenError::Forged
        );
        assert_eq!(
            verify_member_tag(&[], &ev.body).unwrap_err(),
            OpenError::NoKey
        );
    }

    #[test]
    fn a_forged_tag_is_refused_and_a_moved_payload_fails_the_tag() {
        let u = User::new();
        let keys = epoch_keys(5);
        let ev = member_sealed(&u, &keys, 0, vec![], 1, join("alice"));
        // Any change to the ciphertext breaks the tag, even when the author re-signs.
        let mut edited = ev.body.clone();
        let (epoch, mut ciphertext, tag) = member_parts(&edited.payload).unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 1;
        edited.payload = member_sealed_payload(epoch, ciphertext, tag).unwrap();
        let resigned = Event::sign(edited, &u.device);
        assert_eq!(
            verify_member_tag(&[(0, keys.write)], &resigned.body).unwrap_err(),
            OpenError::Forged
        );
        // A tag of zeros is junk.
        let mut junk = ev.body.clone();
        let (epoch, ciphertext, _) = member_parts(&junk.payload).unwrap();
        junk.payload = member_sealed_payload(epoch, ciphertext, [0; 32]).unwrap();
        assert_eq!(
            verify_member_tag(&[(0, keys.write)], &junk).unwrap_err(),
            OpenError::Forged
        );
        // Changing the header breaks the tag too, since the header is bound.
        let mut moved = ev.body.clone();
        moved.hlc = at(2);
        assert_eq!(
            verify_member_tag(&[(0, keys.write)], &moved).unwrap_err(),
            OpenError::Forged
        );
    }

    #[test]
    fn a_member_sealed_payload_cannot_be_read_under_the_generation_path() {
        let u = User::new();
        let keys = epoch_keys(6);
        let ev = member_sealed(&u, &keys, 0, vec![], 1, join("alice"));
        // The generation ring cannot stand in for an epoch: a relabelled payload stays closed.
        let mut relabelled = ev.clone();
        relabelled.body.payload = Payload::SealedControl {
            generation: 0,
            ciphertext: vec![0; 40],
        };
        assert_eq!(
            open_event_keyed(&relabelled, &[(0, keys.enc)], &[(0, keys.enc)]).unwrap_err(),
            OpenError::Forged
        );
    }

    #[test]
    fn outsider_replay_of_member_sealed_control_names_nothing() {
        let owner = User::new();
        let keys = epoch_keys(7);
        let join_ev = member_sealed(&owner, &keys, 0, vec![], 1, join("boss"));
        let chan = member_sealed(
            &owner,
            &keys,
            0,
            vec![join_ev.id],
            2,
            Payload::ChannelCreate {
                name: "secret-room".into(),
            },
        );
        let mut dag = Dag::new();
        dag.insert(join_ev).unwrap();
        dag.insert(chan).unwrap();
        // No epoch key: the DAG alone shows no member, role or channel.
        let st = crate::state::replay_keyed(owner.id(), &dag, &[], &[]);
        assert!(st.members.is_empty());
        assert!(st.channels.is_empty());
        assert!(st.rejected.iter().all(|(_, why)| *why == Rejection::Locked));
        // The epoch key shows them.
        let st = crate::state::replay_keyed(owner.id(), &dag, &[], &[(0, keys.enc)]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.members[&owner.id()].display_name, "boss");
    }

    #[test]
    fn a_departed_member_cannot_open_the_next_epoch() {
        // The leaver kept epoch 0's key. The removal commit moves the group to epoch 1, whose key
        // it never receives, so an event made in epoch 1 stays closed to it.
        let owner = User::new();
        let leaver = User::new();
        let old = epoch_keys(8);
        let new = epoch_keys(9);
        let leave = member_sealed(&leaver, &old, 0, vec![], 1, Payload::Leave);
        let after = member_sealed(
            &owner,
            &new,
            1,
            vec![leave.id],
            2,
            Payload::ChannelCreate {
                name: "after-leave".into(),
            },
        );
        assert_eq!(
            open_event_keyed(&after, &[], &[(0, old.enc)]).unwrap_err(),
            OpenError::NoKey
        );
        let mut dag = Dag::new();
        dag.insert(leave).unwrap();
        dag.insert(after).unwrap();
        let st = crate::state::replay_keyed(owner.id(), &dag, &[], &[(0, old.enc)]);
        assert!(st.channels.is_empty());
        assert_eq!(st.rejected.len(), 1);
    }

    #[test]
    fn member_only_switch_is_owner_only_and_latest_wins() {
        let owner = User::new();
        let admin = User::new();
        let grant = event(
            &owner,
            vec![],
            at(1),
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        let by_admin = event(
            &admin,
            vec![grant.id],
            at(2),
            Payload::MemberOnlyControl { on: true },
        );
        let on = event(
            &owner,
            vec![by_admin.id],
            at(3),
            Payload::MemberOnlyControl { on: true },
        );
        let off = event(
            &owner,
            vec![on.id],
            at(4),
            Payload::MemberOnlyControl { on: false },
        );
        let mut dag = Dag::new();
        for ev in [grant, by_admin.clone(), on, off] {
            dag.insert(ev).unwrap();
        }
        let st = replay_with(owner.id(), &dag, &[]);
        assert!(!st.member_only, "the owner's later switch-off wins");
        assert_eq!(st.rejected.len(), 1);
        assert_eq!(st.rejected[0], (by_admin.id, Rejection::NoPermission));
    }

    #[test]
    fn history_defaults_to_all_and_admins_may_set_it() {
        let owner = User::new();
        let mut dag = Dag::new();
        let set = event(
            &owner,
            vec![],
            at(1),
            Payload::HistoryVisibility {
                setting: HistorySetting::SinceJoin,
            },
        );
        dag.insert(set).unwrap();
        assert_eq!(replay_with(owner.id(), &dag, &[]).history, HistorySetting::SinceJoin);
        let empty = Dag::new();
        assert_eq!(replay_with(owner.id(), &empty, &[]).history, HistorySetting::All);
        // A plain user may not set it.
        let user = User::new();
        let mut dag = Dag::new();
        dag.insert(event(
            &user,
            vec![],
            at(1),
            Payload::HistoryVisibility {
                setting: HistorySetting::SinceJoin,
            },
        ))
        .unwrap();
        let st = replay_with(owner.id(), &dag, &[]);
        assert_eq!(st.history, HistorySetting::All);
        assert_eq!(st.rejected.len(), 1);
    }

    #[test]
    fn invite_events_stay_under_the_community_secret() {
        use crate::is_member_sealable;
        assert!(!is_member_sealable(&Payload::InviteCreate {
            invite_id: [0; 16],
            expires_ms: None,
        }));
        assert!(!is_member_sealable(&Payload::InviteRevoke { invite_id: [0; 16] }));
        assert!(is_member_sealable(&Payload::ChannelCreate { name: "x".into() }));
    }

    #[test]
    fn private_text_opens_only_with_its_key_and_its_aad() {
        let key = [5u8; 32];
        let (community, channel, author) = (CommunityId([1; 32]), ChannelId([2; 32]), RootId([3; 32]));
        let aad = private_aad(&community, &channel, 0, &author);
        let sealed = seal_private_text(&key, &aad, b"secret words").unwrap();
        assert_eq!(open_private_text(&key, &aad, &sealed).unwrap(), b"secret words");
        assert!(open_private_text(&[6u8; 32], &aad, &sealed).is_none(), "another key does not open it");
        let moved = private_aad(&community, &channel, 1, &author);
        assert!(open_private_text(&key, &moved, &sealed).is_none(), "another generation does not open it");
        let elsewhere = private_aad(&community, &ChannelId([9; 32]), 0, &author);
        assert!(open_private_text(&key, &elsewhere, &sealed).is_none(), "another channel does not open it");
    }

    #[test]
    fn the_v0_1_6_member_form_still_opens_beside_the_carrier() {
        let u = User::new();
        let keys = epoch_keys(10);
        let carried = member_sealed(&u, &keys, 3, vec![], 1, join("legacy"));
        // Rebuild the same fields in the form v0.1.6 wrote, and sign it as an event.
        let (epoch, ciphertext, tag) = member_parts(&carried.body.payload).unwrap();
        let legacy = Event::sign(
            EventBody {
                payload: Payload::MemberSealedControl {
                    epoch,
                    ciphertext,
                    tag,
                },
                ..carried.body.clone()
            },
            &u.device,
        );
        assert!(legacy.verify(&u.cert).is_ok());
        let plain = open_event_keyed(&legacy, &[], &[(3, keys.enc)]).unwrap();
        assert_eq!(plain.body.payload, join("legacy"));
        assert!(verify_member_tag(&[(3, keys.write)], &legacy.body).is_ok());
    }

    #[test]
    fn a_carried_event_is_locked_to_a_client_without_the_carrier() {
        // A client that predates the carrier has no key for generation u64::MAX, so it reads the
        // event as locked, not as a decode failure.
        let u = User::new();
        let keys = epoch_keys(11);
        let ev = member_sealed(&u, &keys, 0, vec![], 1, join("hidden"));
        assert!(ev.verify(&u.cert).is_ok());
        let round = postcard::from_bytes::<Event>(&postcard::to_allocvec(&ev).unwrap()).unwrap();
        assert_eq!(round.id, ev.id);
        assert!(round.verify(&u.cert).is_ok());
        // The generation ring cannot open it, since the carrier is not a generation.
        assert_eq!(
            open_event(&round, &[(CARRIER_GENERATION, keys.enc)]).unwrap_err(),
            OpenError::NoKey
        );
        assert_eq!(
            open_event_keyed(&round, &[], &[(0, epoch_keys(12).enc)]).unwrap_err(),
            OpenError::Forged
        );
        assert!(open_event_keyed(&round, &[], &[(0, keys.enc)]).is_ok());
    }

    #[test]
    fn a_private_message_opens_without_a_key_and_an_unknown_carried_kind_is_forged() {
        let u = User::new();
        let body = |payload: Payload| EventBody {
            community: CommunityId([0; 32]),
            channel: ChannelId::CONTROL,
            author: u.id(),
            device: u.device.id(),
            parents: vec![],
            hlc: at(1),
            payload,
        };
        // A private message is readable as its replayed form with no key: its kind and generation are clear.
        let private = carry_private(SealedKind::Message, 2, vec![9, 9, 9]).unwrap();
        let ev = Event::sign(body(private), &u.device);
        let plain = open_event_keyed(&ev, &[], &[]).unwrap();
        assert_eq!(
            plain.body.payload,
            Payload::PrivateSealed {
                kind: SealedKind::Message,
                generation: 2,
                ciphertext: vec![9, 9, 9],
            }
        );
        // A carrier of a kind this build does not know is refused as forged, not locked.
        let unknown = Payload::SealedControl {
            generation: CARRIER_GENERATION,
            ciphertext: postcard::to_allocvec(&Carried {
                kind: 9,
                body: vec![],
            })
            .unwrap(),
        };
        let ev = Event::sign(body(unknown), &u.device);
        assert_eq!(open_event_keyed(&ev, &[], &[]).unwrap_err(), OpenError::Forged);
    }

    #[test]
    fn member_only_and_history_payloads_are_sealable() {
        assert!(is_sealable(&Payload::MemberOnlyControl { on: true }));
        assert!(is_sealable(&Payload::HistoryVisibility {
            setting: HistorySetting::SinceJoin,
        }));
        assert!(!is_sealable(&Payload::MemberSealedControl {
            epoch: 0,
            ciphertext: vec![],
            tag: [0; 32],
        }));
    }

    #[test]
    fn a_rename_is_sealed_under_the_secret_and_replays() {
        let owner = User::new();
        let join_ev = sealed(&owner, &S0, 0, vec![], 1, join("boss"));
        let rename = sealed(
            &owner,
            &S0,
            0,
            vec![join_ev.id],
            2,
            Payload::CommunityRename {
                name: "Hidden Name".into(),
            },
        );
        // Sealed under the generation, as every control payload is. A released client holding the
        // secret cannot decode the new kind inside, and rejects that one event as forged.
        assert!(matches!(rename.body.payload, Payload::SealedControl { generation: 0, .. }));
        let bytes = postcard::to_allocvec(&rename).unwrap();
        assert!(!bytes.windows(b"Hidden Name".len()).any(|w| w == b"Hidden Name"));
        assert_eq!(open_event(&rename, &[]).unwrap_err(), OpenError::NoKey);
        let mut dag = Dag::new();
        dag.insert(join_ev).unwrap();
        dag.insert(rename).unwrap();
        let st = replay_with(owner.id(), &dag, &[(0, S0)]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.name.map(|(_, n)| n).as_deref(), Some("Hidden Name"));
    }

    #[test]
    fn payload_variant_indices_are_unchanged() {
        // postcard writes the variant index first. Leave was the 12th variant, and the sealed
        // variant is appended after SecretRotation, so no stored event changes meaning.
        assert_eq!(postcard::to_allocvec(&Payload::Leave).unwrap()[0], 11);
        let sealed_index = postcard::to_allocvec(&Payload::SealedControl {
            generation: 0,
            ciphertext: vec![],
        })
        .unwrap()[0];
        assert_eq!(sealed_index, 21);
    }
}
