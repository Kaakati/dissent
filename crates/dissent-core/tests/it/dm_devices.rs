//! Direct conversations with more than one device per person. A device linked after the conversation was made
//! is added to it, and reads from its link on. Each side's devices see the other side's messages and their
//! own. A revoked device stops reading. A device from before multi-device conversations is never added, and the
//! conversation still works with the devices it has.

use crate::common::*;
use dissent_core::{start_device_link, CommunityId, Engine};
use dissent_identity::RootId;
use std::time::Duration;

fn has_text(engine: &Engine, peer: RootId, text: &str) -> bool {
    engine.dm_messages(peer, 100).iter().any(|m| m.text == text)
}

/// Each party runs its device sync four times, a second apart. The party with the higher root yields on a pass
/// where the epoch has moved, so it needs a pass with the epoch still, after the other side's commit.
async fn settle(peers: &[&Peer]) {
    for _ in 0..4 {
        for peer in peers {
            peer.engine.sync_all_dm_devices().await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// `has_text` as the wait closures want it.
fn seen(engine: &Engine, peer: RootId, text: &str) -> Option<()> {
    has_text(engine, peer, text).then_some(())
}

/// Links a device to `owner`'s root, and has it join `community` through `inviter`, so the group's members learn its
/// certificate.
async fn link_device(owner: &Peer, name: &str, community: CommunityId, inviter: &Peer) -> Peer {
    let pending = start_device_link(name).expect("link request");
    let grant = owner
        .engine
        .approve_device(&pending.link().expect("link"))
        .expect("owner approves the device");
    let device = start(pending.finish(&grant).expect("device identity"), scratch(name)).await;
    let invite = inviter.engine.create_invite(community).await.expect("invite");
    device.engine.join_community(&invite).await.expect("device joins the shared server");
    device
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn both_sides_linked_devices_see_the_conversation() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let alice_root = alice.identity.root_id();
        let bob_root = bob.identity.root_id();

        alice.engine.send_dm(bob_root, "first".into()).await.expect("alice writes");
        eventually("bob reads first", || seen(&bob.engine, alice_root, "first")).await;

        // Both sides link a device after the conversation exists.
        let bob_tablet = link_device(&bob, "Tablet", community, &alice).await;
        let alice_laptop = link_device(&alice, "Laptop", community, &alice).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        settle(&[&alice, &bob]).await;

        alice.engine.send_dm(bob_root, "second".into()).await.expect("alice writes");
        eventually("bob reads second", || seen(&bob.engine, alice_root, "second")).await;
        eventually("bob's tablet reads second", || seen(&bob_tablet.engine, alice_root, "second")).await;
        eventually("alice's laptop sees her own second", || seen(&alice_laptop.engine, bob_root, "second")).await;
        assert!(!has_text(&bob_tablet.engine, alice_root, "first"), "a linked device starts from its link");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_revoked_device_stops_reading() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let alice_root = alice.identity.root_id();
        let bob_root = bob.identity.root_id();
        let bob_tablet = link_device(&bob, "Tablet", community, &alice).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        settle(&[&alice, &bob]).await;
        alice.engine.send_dm(bob_root, "before".into()).await.expect("alice writes");
        eventually("the tablet reads before", || seen(&bob_tablet.engine, alice_root, "before")).await;

        bob.engine
            .revoke_device(bob_tablet.identity.device_id())
            .await
            .expect("bob revokes the tablet");
        tokio::time::sleep(Duration::from_secs(4)).await;
        settle(&[&alice, &bob]).await;
        alice.engine.send_dm(bob_root, "after".into()).await.expect("alice writes");
        eventually("bob reads after", || seen(&bob.engine, alice_root, "after")).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!has_text(&bob_tablet.engine, alice_root, "after"), "the revoked tablet reads nothing new");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_older_device_is_never_added_and_the_conversation_still_works() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let alice_root = alice.identity.root_id();
        let bob_root = bob.identity.root_id();
        // The conversation exists before the older device links, so only the later add is gated.
        alice.engine.send_dm(bob_root, "first".into()).await.expect("alice writes");
        eventually("bob reads first", || seen(&bob.engine, alice_root, "first")).await;
        let bob_tablet = link_device(&bob, "Tablet", community, &alice).await;
        dissent_core::test_support::set_dm_devices_support(&bob_tablet.engine, false);
        tokio::time::sleep(Duration::from_secs(2)).await;
        settle(&[&alice, &bob]).await;

        alice.engine.send_dm(bob_root, "solo".into()).await.expect("alice writes");
        eventually("bob reads solo", || seen(&bob.engine, alice_root, "solo")).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!has_text(&bob_tablet.engine, alice_root, "solo"), "an older device is not added");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn both_parties_link_a_device_at_the_same_time() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let alice_root = alice.identity.root_id();
        let bob_root = bob.identity.root_id();
        alice.engine.send_dm(bob_root, "first".into()).await.expect("alice writes");
        eventually("bob reads first", || seen(&bob.engine, alice_root, "first")).await;

        // Both devices link, and then both sides run their sync at the same moment.
        let alice_laptop = link_device(&alice, "Laptop", community, &alice).await;
        let bob_tablet = link_device(&bob, "Tablet", community, &alice).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        for _ in 0..4 {
            tokio::join!(alice.engine.sync_all_dm_devices(), bob.engine.sync_all_dm_devices());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        alice.engine.send_dm(bob_root, "after".into()).await.expect("alice writes");
        eventually("bob reads after", || seen(&bob.engine, alice_root, "after")).await;
        eventually("bob's tablet reads after", || seen(&bob_tablet.engine, alice_root, "after")).await;
        eventually("alice's laptop reads after", || seen(&alice_laptop.engine, bob_root, "after")).await;
    })
    .await;
}
