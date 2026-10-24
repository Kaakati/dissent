//! Adversarial membership tests: a member that breaks the commit rules is refused by everyone, a
//! welcome that lists an outsider is refused, and a seeder can admit a joiner while the owner is
//! offline.

use std::time::Duration;

use super::tests::{eventually, general, readable, three_members, Node};
use crate::CommunityId;

/// A key package of a fresh device that has never joined the community.
async fn outsider_key_package(name: &str) -> (Node, Vec<u8>) {
    let mallory = Node::named(name).await;
    let key_package = mallory
        .engine
        .with_mls(|mls| mls.take_key_packages(1))
        .expect("key package")
        .remove(0);
    (mallory, key_package)
}

#[tokio::test(flavor = "multi_thread")]
async fn member_commit_adding_an_outsider_is_rejected_by_everyone() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, _invite) = three_members().await;
    let (mallory, key_package) = outsider_key_package("mallory").await;
    let mallory_device = mallory.identity.device_id();

    // Bob is a legitimate member, but this add brings in a device of no member.
    let forged = bob
        .engine
        .with_mls(|mls| mls.add_members(&id.0, &[key_package]))
        .expect("bob builds the commit");

    for honest in [&alice.engine, &carol.engine] {
        let epoch = honest.mls_epoch(id);
        assert!(
            honest.apply_commit(id, &forged.commit).is_err(),
            "a commit adding an outsider must be refused"
        );
        assert_eq!(honest.mls_epoch(id), epoch, "a refused commit leaves the epoch alone");
        let members = honest
            .with_mls_read(|mls| mls.members(&id.0))
            .expect("members");
        assert!(
            members.iter().all(|c| c.device != mallory_device),
            "the outsider never enters an honest group"
        );
    }

    alice
        .engine
        .send_message(id, general(&alice.engine, id), "secret".into())
        .await
        .expect("alice sends");
    assert!(!readable(&mallory.engine, id, general(&alice.engine, id), "secret"));
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn member_commit_removing_a_current_member_is_rejected_by_everyone() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, _invite) = three_members().await;
    let carol_device = carol.identity.device_id();

    // Bob may remove devices of banned or departed members, not of Carol, who is current.
    let forged = bob
        .engine
        .with_mls(|mls| mls.remove_members(&id.0, &[carol_device]))
        .expect("bob builds the removal");

    for honest in [&alice.engine, &bob.engine] {
        let epoch = honest.mls_epoch(id);
        assert!(
            honest.apply_commit(id, &forged).is_err(),
            "a removal of a current member must be refused"
        );
        assert_eq!(honest.mls_epoch(id), epoch);
    }
    let alice_members = alice
        .engine
        .with_mls_read(|mls| mls.members(&id.0))
        .expect("members");
    assert!(alice_members.iter().any(|c| c.device == carol_device));
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn welcome_listing_a_non_member_is_refused_and_a_real_join_still_works() {
    crate::testutil::bounded(async {
    let (_alice, bob, _carol, id, invite) = three_members().await;
    let (_mallory, key_package) = outsider_key_package("mallory").await;

    let dave = Node::named("dave").await;
    dave.engine
        .join_without_group(&invite)
        .await
        .expect("dave joins the community without a group");
    eventually("dave knows the members", || {
        dave.engine.community_view(id).map(|v| v.members.len()).ok() == Some(4)
    })
    .await;

    // An honest join through Bob is accepted, so the rule does not block real joiners.
    let device = bob.identity.device_id();
    assert!(
        dave.engine.mls_join_via(id, &[device]).await,
        "an honest welcome is accepted"
    );
    assert!(dave.engine.mls_epoch(id).is_some());

    // Bob then adds an outsider. The welcome for that add lists the outsider's device, so a new
    // joiner must refuse it. Bob's group has moved on, so this uses a fresh joiner.
    let eve = Node::named("eve").await;
    eve.engine
        .join_without_group(&invite)
        .await
        .expect("eve joins the community without a group");
    eventually("eve knows the members", || {
        eve.engine.community_view(id).map(|v| v.members.len()).ok() == Some(5)
    })
    .await;
    let forged = bob
        .engine
        .with_mls(|mls| mls.add_members(&id.0, &[key_package]))
        .expect("bob builds the add");
    assert!(
        eve.engine.accept_welcome(id, &forged.welcome).is_err(),
        "a welcome that lists an outsider must be refused"
    );
    assert_eq!(eve.engine.mls_epoch(id), None);
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seeder_admits_a_joiner_while_the_owner_is_offline() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Seeded".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let seeder = Node::named("seeder").await;
    seeder
        .engine
        .join_community(&invite)
        .await
        .expect("seeder joins");
    eventually("seeder has the group", || seeder.engine.mls_epoch(id).is_some()).await;

    // The owner goes offline. Only the plain member is left to admit anyone.
    let _ = alice.stop().await;

    let second_invite = seeder
        .engine
        .create_invite(id)
        .await
        .expect("a member can invite");
    let bob = Node::named("bob").await;
    bob.engine
        .join_community(&second_invite)
        .await
        .expect("bob joins through the seeder");
    eventually("bob has the group", || bob.engine.mls_epoch(id).is_some()).await;

    let channel = general(&seeder.engine, id);
    seeder
        .engine
        .send_message(id, channel, "after bob joined".into())
        .await
        .expect("seeder sends");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !readable(&bob.engine, id, channel, "after bob joined") {
        assert!(std::time::Instant::now() < deadline, "bob never read the message");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    }).await;
}

