//! Signed events. An event's id is the hash of its body, and its signature covers the id.

use crate::{Error, Hlc, Result};
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};

const ID_DOMAIN: &[u8] = b"dissent/event/v1";
const SIG_DOMAIN: &[u8] = b"dissent/event-sig/v1";

macro_rules! digest_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub [u8; 32]);

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&data_encoding::BASE32_NOPAD.encode(&self.0).to_lowercase())
            }
        }
        impl ::std::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{}({})", stringify!($name), &self.to_string()[..8])
            }
        }
        impl ::std::str::FromStr for $name {
            type Err = $crate::Error;
            fn from_str(s: &str) -> $crate::Result<Self> {
                let bytes = data_encoding::BASE32_NOPAD
                    .decode(s.trim().to_ascii_uppercase().as_bytes())
                    .map_err(|e| $crate::Error::Encoding(e.to_string()))?;
                let arr: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| $crate::Error::Encoding("expected 32 bytes".into()))?;
                Ok(Self(arr))
            }
        }
    };
}

pub(crate) use digest_id;

digest_id!(
    EventId,
    "Hash of an event body. The event's address in the DAG."
);
digest_id!(
    ChannelId,
    "Channel address. `CONTROL` holds community-wide events. Any other id is the id of the `ChannelCreate` event that made the channel."
);

impl ChannelId {
    /// Home of community-wide events: roles, bans, handles and channel creation.
    pub const CONTROL: Self = Self([0; 32]);
}

impl EventId {
    /// Hash of the postcard encoding of `body`, under the event domain.
    pub fn of(body: &EventBody) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(ID_DOMAIN);
        h.update(&postcard::to_allocvec(body).expect("EventBody always encodes"));
        Self(*h.finalize().as_bytes())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    Admin,
    Moderator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    ChannelCreate {
        name: String,
    },
    Message {
        text: String,
    },
    Edit {
        target: EventId,
        text: String,
    },
    Delete {
        target: EventId,
    },
    RoleGrant {
        user: RootId,
        role: Role,
    },
    RoleRevoke {
        user: RootId,
        role: Role,
    },
    Ban {
        user: RootId,
    },
    Unban {
        user: RootId,
    },
    HandleClaim {
        handle: String,
    },
    /// Join or update display name. Re-sending updates the name.
    Join {
        display_name: String,
    },
    Leave,
    /// Move the community membership to a new root. Must be authored by the new root.
    KeyRotation(dissent_identity::KeyRotation),
    /// Revoke one device of the author's root. Takes effect for later events from that device.
    DeviceRevoke(dissent_identity::DeviceRevocation),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventBody {
    pub community: CommunityId,
    pub channel: ChannelId,
    pub author: RootId,
    pub device: DeviceId,
    /// Sorted and deduplicated.
    pub parents: Vec<EventId>,
    pub hlc: Hlc,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub body: EventBody,
    pub id: EventId,
    pub sig: Signature,
}

impl Event {
    /// Sort and dedup parents, compute the id, and sign it with `device`.
    /// `body.device` must equal `device.id()`; otherwise [`Event::verify`] fails.
    pub fn sign(mut body: EventBody, device: &DeviceKey) -> Self {
        body.parents.sort_unstable();
        body.parents.dedup();
        debug_assert_eq!(body.device, device.id());
        let id = EventId::of(&body);
        let sig = device.sign(&sig_msg(&id));
        Self { body, id, sig }
    }

    /// Check the id, parent canonical form, the device certificate, and the signature.
    pub fn verify(&self, cert: &DeviceCert) -> Result<()> {
        if EventId::of(&self.body) != self.id {
            return Err(Error::IdMismatch);
        }
        if self.body.parents.windows(2).any(|w| w[0] >= w[1]) {
            return Err(Error::NonCanonical("parents must be sorted and unique"));
        }
        cert.verify()?;
        if cert.device != self.body.device {
            return Err(Error::DeviceMismatch);
        }
        if cert.root != self.body.author {
            return Err(Error::AuthorMismatch);
        }
        self.body.device.verify(&sig_msg(&self.id), &self.sig)?;
        Ok(())
    }
}

fn sig_msg(id: &EventId) -> Vec<u8> {
    [SIG_DOMAIN, id.0.as_slice()].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{event, User};

    #[test]
    fn sign_verify_and_canonical_parents() {
        let u = User::new();
        let a = EventId([1; 32]);
        let b = EventId([2; 32]);
        let ev = event(
            &u,
            vec![b, a, b],
            Hlc {
                wall_ms: 1,
                counter: 0,
            },
            msg("hi"),
        );
        assert_eq!(ev.body.parents, vec![a, b]);
        ev.verify(&u.cert).unwrap();
    }

    #[test]
    fn tampering_is_detected() {
        let u = User::new();
        let ev = event(&u, vec![], Hlc::default(), msg("hi"));

        let mut body_changed = ev.clone();
        body_changed.body.payload = msg("bye");
        assert!(matches!(
            body_changed.verify(&u.cert),
            Err(Error::IdMismatch)
        ));

        let mut sig_flipped = ev.clone();
        sig_flipped.sig.0[0] ^= 1;
        assert!(matches!(
            sig_flipped.verify(&u.cert),
            Err(Error::Identity(_))
        ));

        // Same device, certified by a different root: author mismatch.
        let other = User::new();
        let rogue_cert = other.root.certify(u.device.id(), 0);
        assert!(matches!(ev.verify(&rogue_cert), Err(Error::AuthorMismatch)));

        // Valid cert for some other device: device mismatch.
        assert!(matches!(ev.verify(&other.cert), Err(Error::DeviceMismatch)));

        let mut wrong_device = ev.clone();
        wrong_device.body.device = other.device.id();
        wrong_device.id = EventId::of(&wrong_device.body);
        assert!(matches!(
            wrong_device.verify(&u.cert),
            Err(Error::DeviceMismatch)
        ));
    }

    #[test]
    fn id_stable_across_postcard_roundtrip() {
        let u = User::new();
        let ev = event(
            &u,
            vec![EventId([9; 32])],
            Hlc {
                wall_ms: 5,
                counter: 2,
            },
            msg("x"),
        );
        let bytes = postcard::to_allocvec(&ev).unwrap();
        let back: Event = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, ev);
        assert_eq!(EventId::of(&back.body), back.id);
        back.verify(&u.cert).unwrap();
    }

    #[test]
    fn ids_display_and_parse() {
        let id = EventId([7; 32]);
        let s = id.to_string();
        assert_eq!(s.len(), 52);
        assert_eq!(s.parse::<EventId>().unwrap(), id);
        assert_eq!(
            ChannelId::CONTROL.to_string().parse::<ChannelId>().unwrap(),
            ChannelId::CONTROL
        );
        assert!("short".parse::<EventId>().is_err());
    }

    fn msg(t: &str) -> Payload {
        Payload::Message { text: t.into() }
    }
}
