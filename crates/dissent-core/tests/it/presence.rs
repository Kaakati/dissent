//! Presence as others see it: statuses, invisibility, timed statuses, and who receives a beacon.
//! Loopback only.


use crate::common::*;
use dissent_core::{CommunityId, EngineEvent, NoteKind, Presence, RootId, Status};
use std::time::Duration;
use tokio::time::timeout;

/// What `viewer` shows for `user` as a member of community `id`.
fn member_presence(viewer: &Peer, id: CommunityId, user: RootId) -> Option<Presence> {
    viewer
        .engine
        .community_view(id)
        .ok()?
        .members
        .iter()
        .find(|m| m.root_id == user.to_string())
        .map(|m| m.status)
}

/// What `viewer` shows for `user` on the contacts page.
fn contact_presence(viewer: &Peer, user: RootId) -> Option<Presence> {
    viewer
        .engine
        .list_contacts()
        .contacts
        .iter()
        .find(|c| c.root_id == user.to_string())
        .map(|c| c.status)
}

/// Alice and Bob share three servers. Alice changes status, and Bob's member list in each server shows
/// it within two seconds. Beacons are a minute apart, so the periodic beacon cannot be the cause.
/// Networked: run with `scripts/fwtest.sh dissent-core it --ignored presence::a_status_change`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run through scripts/fwtest.sh"]
async fn a_status_change_reaches_every_shared_server_at_once() {
    bounded(async {
    let alice = peer_slow_beacons("alice").await;
    let bob = peer_slow_beacons("bob").await;
    let alice_root = alice.identity.root_id();
    let mut servers = Vec::new();
    for name in ["Rustaceans", "Makers", "Readers"] {
        let id: CommunityId = alice
            .engine
            .create_community(name.into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &bob, &invite).await;
        servers.push(id);
    }
    // Bob hears Alice's beacon from the joins, so every list shows her online before the change.
    eventually("bob sees alice online in all three", || {
        servers
            .iter()
            .all(|id| member_presence(&bob, *id, alice_root) == Some(Presence::Online))
            .then_some(())
    })
    .await;

    alice
        .engine
        .set_status(Status::Idle, None)
        .expect("alice goes idle");
    eventually_within(Duration::from_secs(2), "bob sees alice idle in all three servers", || {
        servers
            .iter()
            .all(|id| member_presence(&bob, *id, alice_root) == Some(Presence::Idle))
            .then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

/// Alice, Bob and Carol share a server. Carol leaves the network before Alice changes status, so the
/// server's topic has one neighbour left. The status still reaches Bob within two seconds. Beacons are a
/// minute apart. Networked: run with `scripts/fwtest.sh dissent-core it --ignored presence::a_status_change_reaches_with_a_neighbour_gone`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run through scripts/fwtest.sh"]
async fn a_status_change_reaches_with_a_neighbour_gone() {
    bounded(async {
    let alice = peer_slow_beacons("alice").await;
    let bob = peer_slow_beacons("bob").await;
    let carol = peer_slow_beacons("carol").await;
    let alice_root = alice.identity.root_id();
    let id: CommunityId = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &bob, &invite).await;
    join_ready(&alice, &carol, &invite).await;
    eventually("bob and carol see alice online", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Online)
            && member_presence(&carol, id, alice_root) == Some(Presence::Online))
            .then_some(())
    })
    .await;

    // Carol leaves the network. Bob is still there.
    let (_, _) = stop(carol).await;
    // Alice's topic gossip is dropped, as it is when a topic has no live neighbour. Only the direct beacon can
    // carry the change to Bob.
    dissent_core::test_support::mute_gossip_for_tests(alice.identity.device_id(), true);

    alice
        .engine
        .set_status(Status::Idle, None)
        .expect("alice goes idle");
    eventually_within(Duration::from_secs(2), "bob sees alice idle", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Idle)).then_some(())
    })
    .await;
    dissent_core::test_support::mute_gossip_for_tests(alice.identity.device_id(), false);

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invisible_member_reads_as_offline_while_messaging() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    // Bob goes invisible before he joins, so he never sends a beacon.
    bob.engine
        .set_status(Status::Invisible, None)
        .expect("bob goes invisible");
    let id: CommunityId = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &bob, &invite).await;
    let channel = channel_id(&alice.engine, id, "general").expect("general channel");

    bob.engine
        .send_message(id, channel, "hello from bob".into())
        .await
        .expect("bob posts");
    wait_seen(&alice, id, channel, "hello from bob").await;

    // Alice's beacons reach Bob, so the beacon path works in this community.
    eventually("bob sees alice online", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Online)).then_some(())
    })
    .await;
    assert_eq!(member_presence(&alice, id, bob_root), Some(Presence::Offline));
    assert_eq!(alice.engine.user_profile(bob_root).status, Presence::Offline);

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_and_dnd_show_to_a_member() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let alice_root = alice.identity.root_id();
    let id = rustaceans(&alice, &bob).await;

    alice
        .engine
        .set_status(Status::Idle, None)
        .expect("alice goes idle");
    eventually("bob sees alice idle", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Idle)).then_some(())
    })
    .await;

    alice
        .engine
        .set_status(Status::Dnd, None)
        .expect("alice sets do not disturb");
    eventually("bob sees alice do not disturb", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Dnd)).then_some(())
    })
    .await;
    assert_eq!(alice.engine.status(), (Status::Dnd, None));

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_sees_a_status_over_the_direct_connection() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .add_contact(&bob.engine.contact_link())
        .await
        .expect("alice asks bob");
    eventually("bob sees the request", || {
        (!bob.engine.contact_requests().is_empty()).then_some(())
    })
    .await;
    bob.engine
        .accept_request(alice_root)
        .await
        .expect("bob accepts");
    eventually("alice counts bob as a contact", || {
        alice.engine.user_profile(bob_root).is_contact.then_some(())
    })
    .await;

    alice
        .engine
        .set_status(Status::Dnd, None)
        .expect("alice sets do not disturb");
    eventually("bob's contacts show alice do not disturb", || {
        (contact_presence(&bob, alice_root) == Some(Presence::Dnd)).then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_status_reverts_to_online_and_says_so() {
    bounded(async {
    let alice = peer("alice").await;
    let mut events = alice.engine.subscribe();
    let until = now() + 1_500;

    alice
        .engine
        .set_status(Status::Dnd, Some(until))
        .expect("alice sets a timed status");
    assert_eq!(alice.engine.status(), (Status::Dnd, Some(until)));
    assert!(
        alice.engine.set_status(Status::Dnd, Some(now() - 1)).is_err(),
        "a status cannot end in the past"
    );

    let reverted = timeout(Duration::from_secs(20), async {
        loop {
            match events.recv().await {
                Ok(EngineEvent::StatusChanged {
                    status: Status::Online,
                    until_ms: None,
                }) => break,
                Ok(_) => {}
                Err(e) => panic!("engine events closed: {e}"),
            }
        }
    })
    .await;
    assert!(reverted.is_ok(), "the expiry is announced");
    assert_eq!(alice.engine.status(), (Status::Online, None));

    finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_gets_no_beacon() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let alice_root = alice.identity.root_id();
    let id = rustaceans(&alice, &bob).await;

    alice
        .engine
        .set_status(Status::Dnd, None)
        .expect("alice sets do not disturb");
    eventually("bob sees alice do not disturb", || {
        (member_presence(&bob, id, alice_root) == Some(Presence::Dnd)).then_some(())
    })
    .await;

    // Carol shares no community and no contact with Alice, so she hears nothing from her.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(carol.engine.user_profile(alice_root).status, Presence::Offline);

    finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_data_dir_from_before_statuses_still_loads() {
    bounded(async {
    // A device that never set a status writes no status file, as an install from before statuses did.
    let alice = peer("alice").await;
    let bob_root = dissent_core::Identity::create(now()).root_id();
    alice
        .engine
        .pin_conversation("ch:general".into(), true)
        .expect("pin");
    alice
        .engine
        .set_note(bob_root, NoteKind::Note, "met at the meetup".into())
        .expect("note");
    let (identity, dir) = stop(alice).await;

    let alice = start(identity, dir).await;
    assert_eq!(alice.engine.pinned_conversations(), vec!["ch:general".to_string()]);
    assert_eq!(
        alice.engine.user_profile(bob_root).note.as_deref(),
        Some("met at the meetup")
    );
    assert_eq!(alice.engine.status(), (Status::Online, None));

    finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_status_survives_a_restart() {
    bounded(async {
    let alice = peer("alice").await;
    let until = now() + 60 * 60 * 1000;
    alice
        .engine
        .set_status(Status::Dnd, Some(until))
        .expect("alice sets a timed status");
    let (identity, dir) = stop(alice).await;

    let alice = start(identity, dir).await;
    assert_eq!(alice.engine.status(), (Status::Dnd, Some(until)));

    finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn going_invisible_drops_a_seen_member_at_once() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();
    let id = rustaceans(&alice, &bob).await;
    eventually("alice sees bob online", || {
        (member_presence(&alice, id, bob_root) == Some(Presence::Online)).then_some(())
    })
    .await;

    bob.engine
        .set_status(Status::Invisible, None)
        .expect("bob goes invisible");
    // The offline beacon arrives within seconds. Without it, the last beacon would show him for a minute.
    eventually_within(Duration::from_secs(5), "alice sees bob offline", || {
        (member_presence(&alice, id, bob_root) == Some(Presence::Offline)).then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_who_shuts_down_reads_as_offline_at_once() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();
    let id = rustaceans(&alice, &bob).await;
    eventually("alice sees bob online", || {
        (member_presence(&alice, id, bob_root) == Some(Presence::Online)).then_some(())
    })
    .await;

    let (_identity, dir) = stop(bob).await;
    eventually_within(Duration::from_secs(5), "alice sees bob offline", || {
        (member_presence(&alice, id, bob_root) == Some(Presence::Offline)).then_some(())
    })
    .await;

    finish(vec![alice]).await;
    let _ = std::fs::remove_dir_all(dir);
    })
    .await;
}
