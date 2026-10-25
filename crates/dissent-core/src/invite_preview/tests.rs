//! Invite previews on loopback. A member answers only a holder of the invite secret, signs what it
//! answers, and answers each requester a fixed number of times a minute.

use dissent_identity::{CommunityId, CommunityKey, DeviceKey};

use super::{InviteStatus, ASKS_PER_WINDOW};
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
