//! Peer exchange over the network. A changed address reaches a member that was offline through a
//! shared member, and address lookups are answered to members and refused to strangers. Relays and
//! the DHT are off, so every answer comes from another member. Networked, so ignored. Run with:
//!   scripts/fwtest.sh dissent-core it --ignored peer_exchange --nocapture

use crate::common::*;
use dissent_core::{Engine, Identity, NodeConfig};
use dissent_identity::DeviceId;
use std::time::Instant;

/// A seeder: a headless member that answers address lookups for its communities.
async fn start_seeder(name: &str) -> Peer {
    let identity = Identity::create(now());
    let dir = scratch(name);
    let config = NodeConfig {
        serves_lookups: true,
        ..offline_on(0)
    };
    let (engine, events) = Engine::start_with_timings(
        identity.clone(),
        dir.clone(),
        config,
        dissent_core::test_support::vault_for(&dir),
        dissent_core::test_support::fast_timings(),
    )
    .await
    .expect("seeder starts");
    engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_changed_address_reaches_an_offline_member_through_a_shared_member() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        let alice_device: DeviceId = alice.identity.device_id();

        // Bob goes offline before alice moves, so the only member who can carry the news is carol.
        let (bob_identity, bob_dir) = stop(bob).await;
        eventually("carol holds alice's first record", || {
            carol.engine.known_address(alice_device).map(|_| ())
        })
        .await;

        let (alice_identity, alice_dir) = stop(alice).await;
        let alice = start_on(alice_identity, alice_dir, free_port()).await;
        let moved = alice.engine.my_address().expect("alice has an address");
        eventually("carol holds alice's new address", || {
            (carol.engine.known_address(alice_device).as_ref() == Some(&moved)).then_some(())
        })
        .await;

        // Alice goes offline again, so bob can reach her only through carol's copy of her record.
        let (_, alice_dir) = stop(alice).await;
        let _ = std::fs::remove_dir_all(&alice_dir);
        let restarted = Instant::now();
        let bob = start_on(bob_identity, bob_dir, free_port()).await;
        eventually("bob learns alice's new address from carol", || {
            (bob.engine.known_address(alice_device).as_ref() == Some(&moved)).then_some(())
        })
        .await;
        eprintln!("bob learned alice's new address {} ms after restarting", restarted.elapsed().as_millis());

        // The dial stats show how dials were resolved. With relays and the DHT off, none is a DHT dial.
        let status = bob.engine.net_status().await;
        eprintln!("bob's dials after restart: {:?}", status.dials);
        assert!(!status.dials.by_source.contains_key("dht"));

        finish(vec![bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_seeder_answers_a_member_and_refuses_a_stranger() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let seeder = start_seeder("seeder").await;
        let stranger = peer("stranger").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &seeder, &invite).await;
        let alice_device = alice.identity.device_id();
        let seeder_addr = seeder.engine.my_address().expect("seeder has an address");

        // The seeder holds alice's record once they have exchanged records on connecting.
        eventually("the seeder holds alice's record", || {
            seeder.engine.known_address(alice_device).map(|_| ())
        })
        .await;

        // A member gets the record, labelled as a seeder's answer, and it is alice's own address.
        let asked = Instant::now();
        let answer = bob
            .engine
            .lookup_addr(&seeder_addr, alice_device)
            .await
            .expect("a member is answered")
            .expect("the seeder holds the record");
        eprintln!("seeder answered a member's lookup in {} ms", asked.elapsed().as_millis());
        assert!(answer.seeder);
        assert_eq!(Some(answer.addr), alice.engine.my_address());

        // A member asking for a device the seeder holds no record of gets no answer, not a refusal.
        let unknown = bob
            .engine
            .lookup_addr(&seeder_addr, stranger.identity.device_id())
            .await
            .expect("a member is answered");
        assert!(unknown.is_none());

        // A stranger, sharing no community with the seeder or with alice, is refused by both.
        let alice_addr = alice.engine.my_address().expect("alice has an address");
        assert!(
            stranger
                .engine
                .lookup_addr(&seeder_addr, alice_device)
                .await
                .is_err(),
            "a stranger is refused by the seeder"
        );
        assert!(
            stranger
                .engine
                .lookup_addr(&alice_addr, bob.identity.device_id())
                .await
                .is_err(),
            "a stranger is refused by a member"
        );

        finish(vec![alice, bob, seeder, stranger]).await;
    })
    .await;
}
