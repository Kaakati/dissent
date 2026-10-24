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
use serde::Serialize;

use crate::{ChannelId, Error, Event, EventBody, EventId, Hlc, Payload, Result};

const KEY_DOMAIN: &str = "dissent/control-seal/v1";
const AAD_DOMAIN: &[u8] = b"dissent/control-aad/v1";
const NONCE_LEN: usize = 24;

/// A community secret with its generation. A ring of these opens every control event its holder
/// may read.
pub type SealKey = (u64, [u8; 32]);

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
        generation,
    };
    let mut out = AAD_DOMAIN.to_vec();
    out.extend(postcard::to_allocvec(&header).map_err(|e| Error::Encoding(e.to_string()))?);
    Ok(out)
}

fn cipher(secret: &[u8; 32]) -> XChaCha20Poly1305 {
    let key = blake3::derive_key(KEY_DOMAIN, secret);
    XChaCha20Poly1305::new(Key::from_slice(&key))
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
    if !matches!(ev.body.payload, Payload::SealedControl { .. }) {
        return Ok(Cow::Borrowed(ev));
    }
    let payload = open(ring, &ev.body)?;
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
    use crate::{Dag, Rejection};

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
