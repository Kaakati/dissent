//! Guardian recovery and device linking over the loopback network.

mod common;

use common::*;
use dissent_core::{start_device_link, Identity};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn linked_device_posts_until_its_root_revokes_it() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let channel = channel_id(&alice.engine, community, "general").expect("general channel");

    let pending = start_device_link("Laptop").expect("link request");
    let grant = alice
        .engine
        .approve_device(&pending.link().expect("link"))
        .expect("alice approves the laptop");
    let laptop_identity: Identity = pending.finish(&grant).expect("laptop identity");
    let laptop_device = laptop_identity.device_id();
    let laptop = start(laptop_identity, scratch("laptop")).await;

    let invite = alice
        .engine
        .create_invite(community)
        .await
        .expect("invite");
    laptop
        .engine
        .join_community(&invite)
        .await
        .expect("laptop joins with the root's community");
    laptop
        .engine
        .send_message(community, channel, "from the laptop".into())
        .await
        .expect("laptop posts");
    eventually("bob reads the laptop's message", || {
        has_message(&bob.engine, community, channel, "from the laptop").then_some(())
    })
    .await;

    let devices = alice.engine.devices().expect("devices");
    assert!(devices.iter().any(|d| d.this_device));
    let laptop_view = devices
        .iter()
        .find(|d| d.device_id == laptop_device.to_string())
        .expect("laptop is listed");
    assert_eq!(laptop_view.name, "Laptop");
    assert!(!laptop_view.this_device);

    alice
        .engine
        .revoke_device(laptop_device)
        .await
        .expect("revoke");
    assert!(alice
        .engine
        .devices()
        .expect("devices")
        .iter()
        .all(|d| d.device_id != laptop_device.to_string()));

    // The revoked device has not heard yet, so it still posts. Bob must refuse the post.
    let _ = laptop
        .engine
        .send_message(community, channel, "after the revoke".into())
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !has_message(&bob.engine, community, channel, "after the revoke"),
        "a message from a revoked device must be rejected"
    );
    assert!(
        has_message(&bob.engine, community, channel, "from the laptop"),
        "events from before the revoke stay valid"
    );
    finish(vec![alice, bob, laptop]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn two_of_three_guardians_carry_the_owners_role_to_a_new_root() {
    bounded(async {
    let owner = peer("owner").await;
    let bob = peer("bob").await;
    let community = rustaceans(&owner, &bob).await;
    let g1 = peer("guardian one").await;
    let g2 = peer("guardian two").await;
    let g3 = peer("guardian three").await;

    let guardians = owner
        .engine
        .set_guardians(
            vec![g1.identity.root_id(), g2.identity.root_id(), g3.identity.root_id()],
            2,
        )
        .await
        .expect("guardians set");
    assert!(guardians.set);
    assert_eq!(guardians.threshold, 2);
    assert_eq!(guardians.guardians.len(), 3);
    let share_link = guardians.share_link.expect("share link");
    assert_eq!(
        owner.engine.guardians().expect("guardians").guardians,
        guardians.guardians
    );

    // The owner's root is lost. The recovering device has a new root and only an invite.
    let recovered = peer("recovered").await;
    let invite = owner.engine.create_invite(community).await.expect("invite");
    let request = recovered
        .engine
        .start_recovery(owner.identity.root_id())
        .expect("recovery request");
    let approval_one = g1
        .engine
        .approve_recovery(&request)
        .expect("guardian one approves");
    let approval_two = g2
        .engine
        .approve_recovery(&request)
        .expect("guardian two approves");
    let published = recovered
        .engine
        .complete_recovery(&[approval_one, approval_two], &share_link, &[invite])
        .await
        .expect("rotation published");
    assert!(published >= 1);

    let new_root = recovered.identity.root_id().to_string();
    eventually("bob sees the owner's role under the new root", || {
        bob.engine
            .community_view(community)
            .ok()
            .filter(|v| v.members.iter().any(|m| m.root_id == new_root && m.role == "owner"))
            .map(|_| ())
    })
    .await;
    finish(vec![owner, bob, g1, g2, g3, recovered]).await;
    }).await;
}
