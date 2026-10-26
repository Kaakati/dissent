//! A profile picture or server icon, as an event refers to it. The image is an encrypted blob in
//! `dissent-files`: the event carries the key, so only members that can open the event can fetch
//! and read the picture. See `docs/design/08-files.md`.

use serde::{Deserialize, Serialize};

/// Largest picture accepted, in plaintext bytes. Replay rejects a larger one, and the engine
/// refuses to store one.
pub const MAX_PICTURE_BYTES: u64 = 256 * 1024;
/// Largest community banner accepted, in plaintext bytes. A banner is a 16:9 picture of at most 960
/// by 540 pixels, larger than an icon, so it has its own cap.
pub const MAX_BANNER_BYTES: u64 = 512 * 1024;
/// How many preset banner gradients there are. A preset is named by its index, below this.
pub const BANNER_PRESETS: u8 = 10;
/// Largest animated banner accepted, in plaintext bytes: a looping GIF, at most 960 by 540.
pub const MAX_ANIMATED_BANNER_BYTES: u64 = 2 * 1024 * 1024;

/// A community banner: one of the preset gradients, which the UI draws, or an uploaded picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Banner {
    Preset(u8),
    Image(Picture),
    /// A looping GIF, and a still poster frame shown under reduced motion. The GIF has its own,
    /// larger cap. Appended, so a client from before it rejects the event and shows no banner.
    Animated { image: Picture, poster: Picture },
}

impl Banner {
    /// Whether a preset is one of the `BANNER_PRESETS`, and a picture is within the banner cap.
    pub fn is_valid(&self) -> bool {
        match self {
            Banner::Preset(id) => *id < BANNER_PRESETS,
            Banner::Image(picture) => picture.is_valid_banner(),
            Banner::Animated { image, poster } => {
                (1..=MAX_ANIMATED_BANNER_BYTES).contains(&image.size) && poster.is_valid_banner()
            }
        }
    }

    /// The picture shown, for a banner that has one: the image, or the animated GIF.
    pub fn picture(&self) -> Option<Picture> {
        match self {
            Banner::Preset(_) => None,
            Banner::Image(picture) => Some(*picture),
            Banner::Animated { image, .. } => Some(*image),
        }
    }

    /// The still frame of an animated banner, shown under reduced motion.
    pub fn poster(&self) -> Option<Picture> {
        match self {
            Banner::Animated { poster, .. } => Some(*poster),
            _ => None,
        }
    }

    /// Every picture a device must fetch to show the banner.
    pub fn pictures(&self) -> Vec<Picture> {
        self.picture().into_iter().chain(self.poster()).collect()
    }
}

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

    /// Whether the picture is non-empty and within the banner cap.
    pub fn is_valid_banner(&self) -> bool {
        (1..=MAX_BANNER_BYTES).contains(&self.size)
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

    fn rename(name: &str) -> Payload {
        Payload::CommunityRename { name: name.into() }
    }

    #[test]
    fn only_the_owner_renames_and_the_latest_rename_wins() {
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
        let by_member = chain(&mut evs, &member, rename("Bob's place"));
        let by_admin = chain(&mut evs, &admin, rename("Ada's place"));
        chain(&mut evs, &owner, rename("First"));
        chain(&mut evs, &owner, rename("Second"));
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.contains(&(by_member, Rejection::NoPermission)));
        assert!(st.rejected.contains(&(by_admin, Rejection::NoPermission)));
        assert_eq!(st.name.map(|(_, n)| n).as_deref(), Some("Second"));
    }

    #[test]
    fn concurrent_renames_converge_on_the_higher_hlc() {
        let owner = User::new();
        let join = event(&owner, vec![], at(1), Payload::Join { display_name: "boss".into() });
        // Two renames on sibling branches. The later HLC wins whichever order they arrive in.
        let early = event(&owner, vec![join.id], at(5), rename("Early"));
        let late = event(&owner, vec![join.id], at(9), rename("Late"));
        for order in [[early.clone(), late.clone()], [late.clone(), early.clone()]] {
            let mut evs = vec![join.clone()];
            evs.extend(order);
            let st = replay_with(owner.id(), &dag_of(evs), &[]);
            assert!(st.rejected.is_empty());
            assert_eq!(st.name.map(|(_, n)| n).as_deref(), Some("Late"));
        }
    }

    #[test]
    fn a_rename_must_be_trimmed_non_empty_and_short() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let long = "x".repeat(crate::state::MAX_COMMUNITY_NAME_CHARS + 1);
        let bad: Vec<EventId> = ["", "   ", " padded", "padded ", long.as_str()]
            .into_iter()
            .map(|n| chain(&mut evs, &owner, rename(n)))
            .collect();
        let max = "y".repeat(crate::state::MAX_COMMUNITY_NAME_CHARS);
        chain(&mut evs, &owner, rename(&max));
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        for id in bad {
            assert!(st.rejected.contains(&(id, Rejection::Malformed)), "{id:?} should be malformed");
        }
        assert_eq!(st.name.map(|(_, n)| n), Some(max));
    }

    #[test]
    fn a_rename_off_the_control_channel_is_malformed() {
        let owner = User::new();
        let join = event(&owner, vec![], at(1), Payload::Join { display_name: "boss".into() });
        let mut off = event(&owner, vec![join.id], at(2), rename("Elsewhere"));
        off.body.channel = ChannelId([7; 32]);
        let off = Event::sign(off.body, &owner.device);
        let st = replay_with(owner.id(), &dag_of(vec![join, off.clone()]), &[]);
        assert!(st.rejected.contains(&(off.id, Rejection::Malformed)));
        assert!(st.name.is_none());
    }

    #[test]
    fn rename_and_banner_are_appended_so_stored_payloads_keep_their_meaning() {
        // postcard writes the variant index first. The new payloads come after every older one, so
        // an older stored event decodes as it always did.
        let index = |p: &Payload| postcard::to_allocvec(p).unwrap()[0];
        let icon = Payload::CommunityIconSet { avatar: Some(pic(1)) };
        let history = Payload::HistoryVisibility {
            setting: crate::HistorySetting::SinceJoin,
        };
        assert_eq!(index(&Payload::Leave), 11);
        assert!(index(&rename("x")) > index(&history));
        assert!(index(&Payload::CommunityBannerSet { banner: None }) > index(&history));
        let bytes = postcard::to_allocvec(&icon).unwrap();
        assert_eq!(postcard::from_bytes::<Payload>(&bytes).unwrap(), icon);
        // Both are sealed as control. A rename stays under the community secret, so an invite
        // holder can check it; the banner goes under the epoch key with member-only control.
        assert!(crate::is_sealable(&rename("x")) && crate::is_sealable(&Payload::CommunityBannerSet { banner: None }));
        assert!(!crate::is_member_sealable(&rename("x")));
        assert!(crate::is_member_sealable(&Payload::CommunityBannerSet { banner: None }));
        assert!(index(&profile("", &[])) > index(&history));
        assert!(crate::is_sealable(&profile("", &[])) && crate::is_member_sealable(&profile("", &[])));
    }

    #[test]
    fn owners_and_admins_set_the_banner_under_its_own_cap() {
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
        // A banner may be larger than an icon, but not larger than its own cap.
        let wide = Picture {
            size: MAX_PICTURE_BYTES + 1,
            ..pic(1)
        };
        let too_big = Picture {
            size: MAX_BANNER_BYTES + 1,
            ..pic(2)
        };
        let refused = chain(&mut evs, &member, Payload::CommunityBannerSet { banner: Some(Banner::Image(pic(3))) });
        let oversized = chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Image(too_big)) });
        chain(&mut evs, &admin, Payload::CommunityBannerSet { banner: Some(Banner::Image(wide)) });
        let st = replay_with(owner.id(), &dag_of(evs.clone()), &[]);
        assert!(st.rejected.contains(&(refused, Rejection::NoPermission)));
        assert!(st.rejected.contains(&(oversized, Rejection::Malformed)));
        assert_eq!(st.banner.map(|(_, p)| p), Some(Some(Banner::Image(wide))));
        // The owner's later removal clears it.
        chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: None });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert_eq!(st.banner.map(|(_, p)| p), Some(None));
    }

    #[test]
    fn a_profile_banner_is_the_authors_own_and_the_latest_wins() {
        let owner = User::new();
        let bob = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        chain(&mut evs, &bob, Payload::Join { display_name: "bob".into() });
        chain(&mut evs, &owner, Payload::ProfileBannerSet { banner: Some(Banner::Preset(1)) });
        chain(&mut evs, &bob, Payload::ProfileBannerSet { banner: Some(Banner::Image(pic(2))) });
        chain(&mut evs, &bob, Payload::ProfileBannerSet { banner: Some(Banner::Preset(4)) });
        let bad = chain(&mut evs, &bob, Payload::ProfileBannerSet { banner: Some(Banner::Preset(BANNER_PRESETS)) });
        let st = replay_with(owner.id(), &dag_of(evs.clone()), &[]);
        assert!(st.rejected.contains(&(bad, Rejection::Malformed)));
        // Each author sets only their own: bob's events never touch the owner's banner.
        assert_eq!(st.profile_banners[&owner.id()].1, Some(Banner::Preset(1)));
        assert_eq!(st.profile_banners[&bob.id()].1, Some(Banner::Preset(4)));
        chain(&mut evs, &bob, Payload::ProfileBannerSet { banner: None });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert_eq!(st.profile_banners[&bob.id()].1, None);
        assert_eq!(st.profile_banners[&owner.id()].1, Some(Banner::Preset(1)));
    }

    #[test]
    fn concurrent_profile_banners_converge_on_the_higher_hlc() {
        let bob = User::new();
        let join = event(&bob, vec![], at(1), Payload::Join { display_name: "bob".into() });
        let early = event(&bob, vec![join.id], at(5), Payload::ProfileBannerSet { banner: Some(Banner::Preset(2)) });
        let late = event(&bob, vec![join.id], at(9), Payload::ProfileBannerSet { banner: Some(Banner::Preset(7)) });
        for order in [[early.clone(), late.clone()], [late.clone(), early.clone()]] {
            let mut evs = vec![join.clone()];
            evs.extend(order);
            let st = replay_with(bob.id(), &dag_of(evs), &[]);
            assert_eq!(st.profile_banners[&bob.id()].1, Some(Banner::Preset(7)));
        }
        assert!(crate::is_sealable(&Payload::ProfileBannerSet { banner: None }));
    }

    #[test]
    fn an_animated_banner_may_be_up_to_2_mb_with_a_still_poster() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let gif = Picture { size: 1536 * 1024, ..pic(1) };
        let poster = pic(2);
        let too_big = Picture { size: MAX_ANIMATED_BANNER_BYTES + 1, ..pic(3) };
        let huge_poster = Picture { size: MAX_BANNER_BYTES + 1, ..pic(4) };
        let bad = [
            chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Animated { image: too_big, poster }) }),
            chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Animated { image: gif, poster: huge_poster }) }),
        ];
        chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Animated { image: gif, poster }) });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        for id in bad {
            assert!(st.rejected.contains(&(id, Rejection::Malformed)), "{id:?}");
        }
        let banner = st.banner.unwrap().1.unwrap();
        assert_eq!((banner.picture(), banner.poster()), (Some(gif), Some(poster)));
        assert_eq!(banner.pictures(), vec![gif, poster]);
        // A still image keeps its own cap: 2 MB is only for an animated banner.
        assert!(!Banner::Image(gif).is_valid());
    }

    #[test]
    fn a_preset_banner_must_be_one_of_the_presets() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let unknown = chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Preset(BANNER_PRESETS)) });
        chain(&mut evs, &owner, Payload::CommunityBannerSet { banner: Some(Banner::Preset(BANNER_PRESETS - 1)) });
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.contains(&(unknown, Rejection::Malformed)));
        assert_eq!(st.banner.map(|(_, b)| b), Some(Some(Banner::Preset(BANNER_PRESETS - 1))));
    }

    fn profile(description: &str, traits: &[(&str, &str)]) -> Payload {
        Payload::CommunityProfileSet {
            description: description.into(),
            traits: traits
                .iter()
                .map(|(emoji, label)| crate::Trait {
                    emoji: (*emoji).into(),
                    label: (*label).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_trait_emoji_may_be_any_single_grapheme() {
        let owner = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        let complex = [
            ("👍🏽", "Thumbs"),
            ("🧑\u{200D}💻", "Coders"),
            ("", "No emoji"),
            ("🏳\u{FE0F}\u{200D}🌈", "Pride"),
            ("👩🏽\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}👨🏻", "Kiss"),
        ];
        chain(&mut evs, &owner, profile("", &complex));
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        assert!(st.rejected.is_empty(), "{:?}", st.rejected);
        assert_eq!(st.profile.unwrap().1.traits.len(), 5);
    }

    #[test]
    fn a_profile_over_a_cap_is_rejected_whole() {
        use crate::state::{MAX_DESCRIPTION_CHARS, MAX_TRAIT_LABEL_CHARS};
        let owner = User::new();
        let admin = User::new();
        let member = User::new();
        let mut evs = Vec::new();
        chain(&mut evs, &owner, Payload::Join { display_name: "boss".into() });
        chain(&mut evs, &admin, Payload::Join { display_name: "ada".into() });
        chain(&mut evs, &member, Payload::Join { display_name: "bob".into() });
        chain(&mut evs, &owner, Payload::RoleGrant { user: admin.id(), role: Role::Admin });
        let five = [("🎮", "Games"), ("🎨", "Art"), ("🎵", "Music"), ("📚", "Books"), ("🌱", "Plants")];
        let mut six = five.to_vec();
        six.push(("🍕", "Food"));
        let long_label = "x".repeat(MAX_TRAIT_LABEL_CHARS + 1);
        let long_text = "d".repeat(MAX_DESCRIPTION_CHARS + 1);
        let bad = [
            chain(&mut evs, &owner, profile("", &six)),
            chain(&mut evs, &owner, profile(&long_text, &[])),
            chain(&mut evs, &owner, profile("", &[("🎮", long_label.as_str())])),
            chain(&mut evs, &owner, profile("", &[("🎮🎨 two", "Two")])),
            chain(&mut evs, &owner, profile("", &[("A", "Letter")])),
            chain(&mut evs, &owner, profile("", &[("🎮", "")])),
            chain(&mut evs, &owner, profile(" padded", &[])),
        ];
        let by_member = chain(&mut evs, &member, profile("Bob's", &[]));
        let max = "d".repeat(MAX_DESCRIPTION_CHARS);
        chain(&mut evs, &admin, profile(&max, &five));
        let st = replay_with(owner.id(), &dag_of(evs), &[]);
        for id in bad {
            assert!(st.rejected.contains(&(id, Rejection::Malformed)), "{id:?} should be malformed");
        }
        assert!(st.rejected.contains(&(by_member, Rejection::NoPermission)));
        let (_, kept) = st.profile.expect("the admin's profile stands");
        assert_eq!(kept.description, max);
        assert_eq!(kept.traits.len(), 5);
        assert_eq!(kept.traits[0].emoji, "🎮");
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
