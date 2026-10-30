//! Offline delivery through an inbox that another person hosts: Carol, Bob's friend, holds Bob's mail. Bob picks
//! her device from the devices that offer, Alice writes while Bob is offline, and Bob reads it when he is back.
//! See `docs/design/10-dm-delivery.md`, "Choosing an inbox".

use crate::common::*;
use dissent_core::RootId;
use std::time::Duration;

fn carol_device(carol: &Peer) -> String {
    carol.identity.device_id().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_friends_device_holds_a_message_until_the_recipient_is_back() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        // A shared server teaches every device the others' addresses, as it would for friends who share one.
        let community = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(community).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        make_contacts_ready(&bob, &carol).await;
        make_contacts_ready(&alice, &bob).await;
        let alice_root: RootId = alice.identity.root_id();
        let bob_root: RootId = bob.identity.root_id();

        // Before Carol agrees, her device is not offered.
        assert!(bob.engine.inbox_candidates().await.is_empty(), "no device offers yet");

        // Carol hosts Bob. Bob sees her device offered, and picks it.
        carol.engine.host_inbox_for(bob_root, true).expect("carol hosts bob");
        let carol_root = carol.identity.root_id().to_string();
        eventually("bob is told carol holds his mail", || {
            bob.engine
                .inbox_notices()
                .first()
                .is_some_and(|n| n.on && n.root == carol_root && n.device == carol_device(&carol))
                .then_some(())
        })
        .await;
        let offered = bob.engine.inbox_candidates().await;
        assert_eq!(offered.len(), 1, "carol's device is offered: {offered:?}");
        assert_eq!(offered[0].device, carol_device(&carol));
        assert_eq!(offered[0].owner.as_deref(), Some(carol.identity.root_id().to_string().as_str()));
        assert_eq!(offered[0].name, "carol");
        assert!(offered[0].serving && !offered[0].own_device && !offered[0].chosen);
        bob.engine
            .set_inboxes(vec![carol.identity.device_id()])
            .await
            .expect("bob picks carol's device");
        eventually("carol holds bob's key packages", || {
            (carol.engine.inbox_stats().key_packages > 0).then_some(())
        })
        .await;
        let status = bob.engine.inbox_status().await;
        assert!(status[0].chosen && status[0].serving && status[0].key_packages > 0, "{status:?}");
        assert!(bob.engine.inbox_candidates().await.is_empty(), "a chosen inbox is not offered again");

        // Bob goes offline. Alice writes; with no group yet she takes Bob's key package from Carol, and the
        // welcome and the message wait there.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (bob_id, bob_dir) = stop(bob).await;
        alice
            .engine
            .send_dm(bob_root, "held by carol".into())
            .await
            .expect("alice writes");
        eventually("the message is queued at carol's device", || {
            alice
                .engine
                .dm_messages(bob_root, 10)
                .iter()
                .any(|m| m.text == "held by carol" && m.queued)
                .then_some(())
        })
        .await;
        eventually("carol holds the welcome and the message", || {
            (carol.engine.inbox_stats().envelopes >= 2).then_some(())
        })
        .await;
        let hosted = carol.engine.hosted_inboxes();
        assert_eq!(hosted.len(), 1);
        assert!(hosted[0].envelopes >= 2 && hosted[0].bytes > 0);

        // Bob comes back, collects from Carol, reads it, and Carol's copy goes.
        let bob = start(bob_id, bob_dir).await;
        bob.engine.collect_inboxes().await.expect("bob collects");
        eventually("bob reads the held message", || dm_has(&bob.engine, alice_root, "held by carol").then_some(()))
            .await;
        eventually("carol's copy is acknowledged away", || {
            (carol.engine.inbox_stats().envelopes == 0).then_some(())
        })
        .await;

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_inbox_that_stops_hosting_shows_as_not_serving() {
    bounded(async {
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        make_contacts_ready(&bob, &carol).await;
        let bob_root = bob.identity.root_id();

        carol.engine.host_inbox_for(bob_root, true).expect("carol hosts bob");
        bob.engine
            .set_inboxes(vec![carol.identity.device_id()])
            .await
            .expect("bob picks carol's device");
        assert!(bob.engine.inbox_status().await[0].serving);

        carol.engine.host_inbox_for(bob_root, false).expect("carol stops");
        let status = bob.engine.inbox_status().await;
        assert!(status[0].reachable && !status[0].serving, "{status:?}");

        // Unreachable: Carol is offline. Her device stays chosen, and shows as not answering.
        let (_carol_id, carol_dir) = stop(carol).await;
        let status = bob.engine.inbox_status().await;
        assert!(status[0].chosen && !status[0].reachable, "{status:?}");

        finish(vec![bob]).await;
        let _ = std::fs::remove_dir_all(carol_dir);
    })
    .await;
}
