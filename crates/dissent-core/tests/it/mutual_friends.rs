//! Mutual friends on loopback. Contacts see the whole contact list of a contact who shares it. Nobody
//! else does: not with the setting off, not for someone blocked, not for a stranger.

use crate::common::*;
use dissent_core::RootId;

/// Alice, Bob and Carol, each a contact of the other two.
async fn triangle() -> (Peer, Peer, Peer) {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    make_contacts_ready(&alice, &bob).await;
    make_contacts_ready(&alice, &carol).await;
    make_contacts_ready(&bob, &carol).await;
    (alice, bob, carol)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_sees_the_one_friend_it_shares_with_another_contact() {
    bounded(async {
        let (alice, bob, carol) = triangle().await;
        let mutual = alice.engine.mutual_friends(bob.identity.root_id()).await;
        let roots: Vec<String> = mutual.iter().map(|f| f.root_id.clone()).collect();
        assert_eq!(roots, vec![carol.identity.root_id().to_string()], "{mutual:?}");
        assert_eq!(mutual[0].display_name, "carol");

        let profile = alice.engine.user_profile_full(bob.identity.root_id()).await;
        assert_eq!(profile.mutual_friends.len(), 1, "the profile carries the same list");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_sharing_off_no_mutual_friends_show() {
    bounded(async {
        let (alice, bob, carol) = triangle().await;
        // Carol asks first, so Alice's own answer is not cached from before the change.
        assert_eq!(carol.engine.mutual_friends(bob.identity.root_id()).await.len(), 1, "shown while on");
        bob.engine.set_friends_shared(false).expect("bob turns sharing off");
        assert!(!bob.engine.friends_shared());
        let bob_root: RootId = bob.identity.root_id();
        assert!(alice.engine.mutual_friends(bob_root).await.is_empty());
        assert!(alice.engine.user_profile_full(bob_root).await.mutual_friends.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_contact_gets_no_list() {
    bounded(async {
        let (alice, bob, carol) = triangle().await;
        let bob_root = bob.identity.root_id();
        // Carol asks first, so Alice's own answer is not cached from before the block.
        assert_eq!(carol.engine.mutual_friends(bob_root).await.len(), 1, "shown before the block");
        bob.engine.block_user(alice.identity.root_id()).expect("bob blocks alice");
        assert!(alice.engine.mutual_friends(bob_root).await.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_sees_no_mutual_friends() {
    bounded(async {
        let (_alice, bob, _carol) = triangle().await;
        let stranger = peer("stranger").await;
        assert!(stranger.engine.mutual_friends(bob.identity.root_id()).await.is_empty());
    })
    .await;
}
