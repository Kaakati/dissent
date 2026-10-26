//! Invite previews on loopback. A member answers only a holder of the invite secret, signs what it
//! answers, and answers each requester a fixed number of times a minute.

use dissent_identity::{CommunityId, CommunityKey, DeviceKey};

use super::{proven_name, InviteStatus, ASKS_PER_WINDOW};
use crate::access::community_token;
use crate::membership::tests::Node;
use crate::rpc::Response;

async fn gated_community() -> (Node, CommunityId) {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Vault".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    (alice, id)
}

fn token_of(alice: &Node, id: CommunityId) -> [u8; 32] {
    let secret = alice.engine.access_secret(id).expect("alice holds the community secret");
    community_token(&secret, &id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_holder_gets_a_preview_that_verifies() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let token = token_of(&alice, id);
        let reply = alice
            .engine
            .answer_invite_preview(DeviceKey::generate().id(), id, token, None);
        let Response::InvitePreview(signed) = reply else {
            panic!("expected a preview, got {reply:?}");
        };
        signed.verify(&id).expect("the preview is signed by alice's device");
        assert_eq!(signed.preview.name, "Vault");
        assert_eq!(signed.preview.member_count, 1);
        assert_eq!(signed.preview.invite, InviteStatus::Live);
        assert!(signed.verify(&CommunityKey::generate().id()).is_err(), "a preview is for one community");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_without_the_secret_is_refused() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let asker = DeviceKey::generate().id();
        let wrong = community_token(&[9; 32], &id);
        let reply = alice.engine.answer_invite_preview(asker, id, wrong, None);
        assert!(matches!(reply, Response::Error(_)), "{reply:?}");

        let stranger = CommunityKey::generate().id();
        let reply = alice.engine.answer_invite_preview(asker, stranger, [0; 32], None);
        assert!(matches!(reply, Response::Error(_)), "{reply:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_requester_gets_a_fixed_number_of_previews_a_minute() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let token = token_of(&alice, id);
        let asker = DeviceKey::generate().id();
        for _ in 0..ASKS_PER_WINDOW {
            let reply = alice.engine.answer_invite_preview(asker, id, token, None);
            assert!(matches!(reply, Response::InvitePreview(_)), "{reply:?}");
        }
        let reply = alice.engine.answer_invite_preview(asker, id, token, None);
        assert!(matches!(reply, Response::Refused(_)), "{reply:?}");

        let other = DeviceKey::generate().id();
        let reply = alice.engine.answer_invite_preview(other, id, token, None);
        assert!(matches!(reply, Response::InvitePreview(_)), "another requester is not limited: {reply:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_invite_is_reported_as_revoked() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let invite_id: [u8; 16] = rand::random();
        alice
            .engine
            .author_publish(
                id,
                dissent_events::ChannelId::CONTROL,
                dissent_events::Payload::InviteCreate {
                    invite_id,
                    expires_ms: None,
                },
            )
            .await
            .expect("alice records the invite");
        alice
            .engine
            .revoke_invite(id, invite_id)
            .await
            .expect("alice revokes it");
        let token = token_of(&alice, id);
        let reply = alice
            .engine
            .answer_invite_preview(DeviceKey::generate().id(), id, token, Some(invite_id));
        let Response::InvitePreview(signed) = reply else {
            panic!("expected a preview, got {reply:?}");
        };
        assert_eq!(signed.preview.invite, InviteStatus::Revoked);
    })
    .await;
}

/// Stand-in bytes for a cropped WebP banner, larger than an icon may be.
fn banner_bytes() -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(5u8, 300 * 1024));
    bytes
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_reaches_the_full_preview_with_the_owners_proof() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        alice.engine.rename_community(id, "  Vault Two ".into()).await.expect("the owner renames");
        alice.engine.set_community_banner(id, banner_bytes()).await.expect("the owner sets a banner");
        let token = token_of(&alice, id);
        let keys = alice.engine.access_keys(id).expect("alice holds the keys");

        let reply = alice.engine.answer_invite_preview_full(DeviceKey::generate().id(), id, token, None);
        let Response::InvitePreviewFull(full) = reply else {
            panic!("expected a full preview, got {reply:?}");
        };
        full.verify(&id).expect("the full preview is signed by alice's device");
        // The plain part keeps the genesis name, which older clients check against the genesis.
        assert_eq!(full.base.preview.name, "Vault");
        assert!(full.extra.banner.is_some(), "the banner travels in the extra");
        let rename = full.extra.rename.as_ref().expect("the owner's rename is the proof");
        let ring = [(keys.generation, keys.secret)];
        assert_eq!(proven_name(rename, &full.base.genesis, &ring).as_deref(), Some("Vault Two"));
        // Without the invite secret the rename proves nothing, so the genesis name stands.
        assert_eq!(proven_name(rename, &full.base.genesis, &[(keys.generation, [9; 32])]), None);

        // A changed extra no longer verifies.
        let mut forged = (*full).clone();
        forged.extra.banner = None;
        assert!(forged.verify(&id).is_err(), "the extra is covered by the signature");

        // The plain preview answers older clients with the genesis name.
        let reply = alice.engine.answer_invite_preview(DeviceKey::generate().id(), id, token, None);
        let Response::InvitePreview(plain) = reply else {
            panic!("expected a preview, got {reply:?}");
        };
        plain.verify(&id).expect("the plain preview still verifies after a rename");
        assert_eq!(plain.preview.name, "Vault");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_by_someone_other_than_the_genesis_owner_proves_nothing() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        alice.engine.rename_community(id, "Vault Two".into()).await.expect("the owner renames");
        let token = token_of(&alice, id);
        let keys = alice.engine.access_keys(id).expect("alice holds the keys");
        let Response::InvitePreviewFull(full) = alice.engine.answer_invite_preview_full(DeviceKey::generate().id(), id, token, None) else {
            panic!("expected a full preview");
        };
        // The same rename checked against another community's genesis is refused.
        let (bob, other) = {
            let bob = crate::membership::tests::Node::named("bob").await;
            let other: CommunityId = bob.engine.create_community("Other".into()).await.expect("create").id.parse().unwrap();
            (bob, other)
        };
        let genesis = bob.engine.read().communities[&other].genesis.clone();
        let rename = full.extra.rename.as_ref().unwrap();
        assert_eq!(proven_name(rename, &genesis, &[(keys.generation, keys.secret)]), None);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_preview_shows_the_new_name_and_banner() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let link = alice.engine.create_invite(id).await.expect("invite");
        alice.engine.rename_community(id, "Vault Two".into()).await.expect("the owner renames");
        alice.engine.set_community_banner(id, banner_bytes()).await.expect("the owner sets a banner");
        let view = alice.engine.invite_preview(&link).await.expect("own preview");
        assert!(view.already_member);
        assert_eq!(view.name, "Vault Two");
        let banner = view.banner.expect("the banner file is on this device");
        assert_eq!(std::fs::read(banner).unwrap(), banner_bytes());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_traits_reach_the_view_the_own_preview_and_the_full_preview() {
    crate::testutil::bounded(async {
        let (alice, id) = gated_community().await;
        let link = alice.engine.create_invite(id).await.expect("invite");
        let traits = vec![
            dissent_events::Trait { emoji: "🧑\u{200D}💻".into(), label: "Coders".into() },
            dissent_events::Trait { emoji: "👍🏽".into(), label: "Friendly".into() },
            dissent_events::Trait { emoji: String::new(), label: "No emoji".into() },
        ];
        alice
            .engine
            .set_community_profile(id, "We build.".into(), traits.clone())
            .await
            .expect("the owner saves the profile");
        let view = alice.engine.community_view(id).expect("view");
        assert_eq!((view.description.as_str(), &view.traits), ("We build.", &traits));
        let own = alice.engine.invite_preview(&link).await.expect("own preview");
        assert_eq!(own.traits, traits);
        let token = token_of(&alice, id);
        let Response::InvitePreviewFull(full) = alice.engine.answer_invite_preview_full(DeviceKey::generate().id(), id, token, None) else {
            panic!("expected a full preview");
        };
        full.verify(&id).expect("verifies");
        assert_eq!(full.extra.profile.expect("the profile travels").traits, traits);
    })
    .await;
}
