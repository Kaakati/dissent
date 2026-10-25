//! A profile picture or server icon, as an event refers to it. The image is an encrypted blob in
//! `dissent-files`: the event carries the key, so only members that can open the event can fetch
//! and read the picture. See `docs/design/08-files.md`.

use serde::{Deserialize, Serialize};

/// Largest picture accepted, in plaintext bytes. Replay rejects a larger one, and the engine
/// refuses to store one.
pub const MAX_PICTURE_BYTES: u64 = 256 * 1024;

/// Where a picture's encrypted blob is, and how to open it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    /// BLAKE3 hash of the ciphertext blob. Fetching verifies every chunk against it.
    pub hash: [u8; 32],
    /// The file key the blob was encrypted under.
    pub key: [u8; 32],
    /// The nonce prefix the blob was encrypted with.
    pub nonce_prefix: [u8; 8],
    /// Plaintext length in bytes. At most [`MAX_PICTURE_BYTES`].
    pub size: u64,
}

impl Picture {
    /// Whether the picture is non-empty and within the size cap.
    pub fn is_valid(&self) -> bool {
        (1..=MAX_PICTURE_BYTES).contains(&self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::seal;
    use crate::state::replay_with;
    use crate::testutil::{event, User};
    use crate::{ChannelId, Dag, Event, EventBody, EventId, Hlc, Payload, Rejection, Role};
    use dissent_identity::CommunityId;

    const SECRET: [u8; 32] = [5; 32];

    fn at(wall_ms: u64) -> Hlc {
        Hlc {
            wall_ms,
            counter: 0,
        }
    }

    fn pic(tag: u8) -> Picture {
        Picture {
            hash: [tag; 32],
            key: [tag.wrapping_add(100); 32],
            nonce_prefix: [tag; 8],
            size: 1000,
        }
    }

    /// Appends a control event by `u`, citing the last one, so the events form one causal chain.
    fn chain(evs: &mut Vec<Event>, u: &User, payload: Payload) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let ev = event(u, parents, at(evs.len() as u64 + 1), payload);
        let id = ev.id;
        evs.push(ev);
        id
    }

    fn dag_of(evs: Vec<Event>) -> Dag {
        let mut dag = Dag::new();
        for ev in evs {
            dag.insert(ev).unwrap();
        }
        dag
    }

    #[test]
    fn the_newest_picture_of_a_member_wins() {
        let owner = User::new();
        let bob = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        chain(&mut evs, &bob, Payload::Join { display_name: "bob".into() });
        chain(&mut evs, &bob, Payload::ProfileUpdate { avatar: Some(pic(1)) });
        chain(&mut evs, &bob, Payload::ProfileUpdate { avatar: Some(pic(2)) });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.avatars[&bob.id()].1, Some(pic(2)));
    }

    #[test]
    fn only_the_owner_and_admins_set_the_icon() {
        let owner = User::new();
        let admin = User::new();
        let member = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        chain(&mut evs, &admin, Payload::Join { display_name: "ada".into() });
        chain(&mut evs, &member, Payload::Join { display_name: "bob".into() });
        chain(
            &mut evs,
            &owner,
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        let refused = chain(&mut evs, &member, Payload::CommunityIconSet { avatar: Some(pic(3)) });
        chain(&mut evs, &admin, Payload::CommunityIconSet { avatar: Some(pic(4)) });
        chain(&mut evs, &owner, Payload::CommunityIconSet { avatar: None });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.contains(&(refused, Rejection::NoPermission)));
        // The owner's later removal is the newest icon event.
        assert_eq!(st.icon.map(|(_, p)| p), Some(None));
    }

    #[test]
    fn an_icon_from_an_admin_is_kept_until_someone_newer_changes_it() {
        let owner = User::new();
        let admin = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        chain(&mut evs, &admin, Payload::Join { display_name: "ada".into() });
        chain(
            &mut evs,
            &owner,
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        chain(&mut evs, &admin, Payload::CommunityIconSet { avatar: Some(pic(5)) });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.is_empty());
        assert_eq!(st.icon.map(|(_, p)| p), Some(Some(pic(5))));
    }

    #[test]
    fn a_picture_that_is_empty_or_over_the_cap_is_malformed() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let too_big = Picture {
            size: MAX_PICTURE_BYTES + 1,
            ..pic(6)
        };
        let empty = Picture { size: 0, ..pic(7) };
        let big = chain(&mut evs, &owner, Payload::ProfileUpdate { avatar: Some(too_big) });
        let none = chain(&mut evs, &owner, Payload::ProfileUpdate { avatar: Some(empty) });
        let icon = chain(&mut evs, &owner, Payload::CommunityIconSet { avatar: Some(too_big) });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        for id in [big, none, icon] {
            assert!(st.rejected.contains(&(id, Rejection::Malformed)), "{id:?} should be malformed");
        }
        assert!(st.avatars.is_empty());
        assert!(st.icon.is_none());
    }

    #[test]
    fn a_sealed_picture_shows_neither_hash_nor_key_without_the_secret() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let picture = pic(9);
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel: ChannelId::CONTROL,
            author: owner.id(),
            device: owner.device.id(),
            parents: vec![evs.last().unwrap().id],
            hlc: at(10),
            payload: Payload::ProfileUpdate {
                avatar: Some(picture),
            },
        };
        let sealed = seal(&SECRET, 0, &body).unwrap();
        let ev = Event::sign(EventBody { payload: sealed, ..body }, &owner.device);
        let bytes = postcard::to_allocvec(&ev).unwrap();
        assert!(!bytes.windows(32).any(|w| w == picture.key), "the key is in the clear");
        assert!(!bytes.windows(32).any(|w| w == picture.hash), "the hash is in the clear");
        let id = ev.id;
        evs.push(ev);
        let dag = dag_of(evs);

        let outsider = replay_with(owner.id(), &dag, &[]);
        assert!(outsider.rejected.contains(&(id, Rejection::Locked)));
        assert!(outsider.avatars.is_empty());

        let member = replay_with(owner.id(), &dag, &[(0, SECRET)]);
        assert_eq!(member.avatars[&owner.id()].1, Some(picture));
    }
}
