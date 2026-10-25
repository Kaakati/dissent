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

/// What a sealed payload is. Targets are in clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SealedKind {
    Message,
    Reply { target: EventId },
    Edit { target: EventId },
    /// A reaction. The emoji is in the sealed bytes, so only members see which one.
    React { target: EventId },
    /// Removes the author's reaction. The emoji is in the sealed bytes.
    Unreact { target: EventId },
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
    /// A channel for voice calls. Checked and stored like `ChannelCreate`, and listed as voice.
    VoiceChannelCreate {
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
    /// Message content sealed under the community's MLS group. The kind and any target stay in
    /// clear, so permission checks need no key. Only the text is ciphertext.
    Sealed {
        /// MLS epoch the ciphertext was made in.
        epoch: u64,
        kind: SealedKind,
        ciphertext: Vec<u8>,
    },
    /// Reply to a live message in the same channel.
    Reply {
        target: EventId,
        text: String,
    },
    /// Add a reaction. `emoji` is validated by `crate::emoji::normalize_emoji`.
    React {
        target: EventId,
        emoji: String,
    },
    /// Remove a reaction. A no-op if the author has not reacted with `emoji`.
    Unreact {
        target: EventId,
        emoji: String,
    },
    /// Sets the relay servers members use for this community. Admins and the owner only. The
    /// latest by HLC wins. See `relay::valid_relays` for the limits.
    SetRelays { relays: Vec<String> },
    /// Joins as a storage member: it keeps and serves the community's ciphertext, mirrors blobs and
    /// serves as an inbox. It is never added to the MLS group, so it cannot read messages, and it
    /// cannot post. Appended, so the encoding of earlier variants is unchanged.
    JoinAsStorage { display_name: String },
    /// Rotates the community secret after bans. `generation` is the previous one plus one, and
    /// `covers` are the banned roots this rotation answers. `sealed` is the new 32-byte secret,
    /// encrypted under the group after the removal commits, so a removed device cannot open it.
    SecretRotation {
        generation: u64,
        covers: Vec<RootId>,
        sealed: Vec<u8>,
    },
    /// A control payload sealed under the community secret of `generation`. The kind and every
    /// field are inside `ciphertext`, so only a member holding that generation's secret can replay
    /// it. Appended, so the encoding of earlier variants is unchanged. See `control`.
    SealedControl {
        generation: u64,
        ciphertext: Vec<u8>,
    },
    /// Changes a channel's settings. A field left `None` stays as it is. Owner and admin only.
    /// `slowmode_s` and the voice fields are checked against the channel's kind. Appended, see
    /// `control`.
    ChannelUpdate {
        channel: ChannelId,
        name: Option<String>,
        slowmode_s: Option<u32>,
        visibility: Option<ChannelVisibility>,
        bitrate_kbps: Option<u32>,
        user_limit: Option<u32>,
        /// A relay from the community's `SetRelays`, or empty for automatic.
        relay: Option<String>,
    },
    /// Deletes a channel and its messages. Owner and admin only. Appended, see `control`.
    ChannelDelete { channel: ChannelId },
    /// Creates an invite. Its link carries `invite_id`, and it admits no one after `expires_ms`
    /// when that is set. Appended, see `control`.
    InviteCreate {
        invite_id: [u8; 16],
        expires_ms: Option<u64>,
    },
    /// Stops new joins through an invite. Members already in the community stay. Owner and admin
    /// only. Appended, see `control`.
    InviteRevoke { invite_id: [u8; 16] },
    /// Joins through an invite made with `InviteCreate`. Refused once the invite is revoked or
    /// expired, so every member refuses it the same way. Appended, see `control`.
    JoinVia {
        display_name: String,
        invite_id: [u8; 16],
    },
    /// Sets or clears the author's profile picture. Sealed under the community secret, so only
    /// members see the picture's key. Only the author can change their own picture, so there is
    /// no permission to check beyond the author. Appended, see `control` and `picture`.
    ProfileUpdate { avatar: Option<crate::picture::Picture> },
    /// Sets or clears the community's icon. Owner and admin only. Sealed, like `ProfileUpdate`.
    /// Appended, see `control` and `picture`.
    CommunityIconSet { avatar: Option<crate::picture::Picture> },
    /// A control payload sealed under the encryption key of MLS epoch `epoch`, exported from the
    /// community's MLS group. Only devices that were members in that epoch hold the key, so storage
    /// members and everyone removed since cannot open it. `tag` is a keyed hash over the header and
    /// ciphertext, under the epoch's write key. A storage member checks it without being able to
    /// decrypt, and drops events without a valid tag. Appended, see `control`.
    MemberSealedControl {
        epoch: u64,
        ciphertext: Vec<u8>,
        tag: [u8; 32],
    },
    /// Turns member-only control on or off. Owner only. While on, members author control events in
    /// the `MemberSealedControl` form. Appended, see `control`.
    MemberOnlyControl { on: bool },
    /// Which past control content a new member is given keys for. Owner and admin only. Appended,
    /// see `control`.
    HistoryVisibility { setting: HistorySetting },
}

/// How much control history a member who joins later can read. See `Payload::HistoryVisibility`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistorySetting {
    /// Every epoch's keys go to a new member. The default, as before.
    #[default]
    All,
    /// A new member gets keys from the epoch it joins in, and nothing older.
    SinceJoin,
}

/// Who may read a channel without a warning. Default is for everyone who can view the channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelVisibility {
    #[default]
    Default,
    /// Messages and attachments are blurred until someone clicks to reveal them.
    Spoiler,
    /// Viewers confirm their age once before they see the channel.
    AgeRestricted,
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
