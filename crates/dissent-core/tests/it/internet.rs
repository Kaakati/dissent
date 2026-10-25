//! End-to-end over the real network: n0 relays, the Mainline DHT, and no loopback. Bob joins with
//! an invite that names no peer, so he must find Alice through her DHT record.
//!
//! Ignored by default. Run it through the firewall-approved script:
//!   scripts/fwtest.sh dissent-core internet --ignored --nocapture


use crate::common::*;
use dissent_core::{Engine, Identity, NodeConfig};
use dissent_discovery::{Discovery, Invite};
use std::time::{Duration, Instant};

/// Each wait for the network gets this long.
const LIMIT: Duration = Duration::from_secs(60);

async fn start_real(name: &str) -> Peer {
    let identity = Identity::create(now());
    let dir = scratch(name);
    let (engine, events) = Engine::start(identity.clone(), dir.clone(), NodeConfig::default(), dissent_core::test_support::vault_for(&dir))
        .await
        .expect("engine starts");
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
#[ignore]
async fn join_over_the_dht_then_message_and_dm() {
    bounded(async {
    let t0 = Instant::now();
    let alice = start_real("alice-net").await;
    eprintln!("[{:?}] alice started", t0.elapsed());
    let bob = start_real("bob-net").await;
    eprintln!("[{:?}] bob started", t0.elapsed());
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let server = alice
        .engine
        .create_community("Over the DHT".into())
        .await
        .expect("alice creates the community");
    let id: dissent_core::CommunityId = server.id.parse().unwrap();

    let url = alice.engine.create_invite(id).await.expect("invite");
    let invite = Invite::parse(&url).expect("invite parses");
    // Alice's record must reach the DHT before Bob can find her there.
    let discovery = Discovery::new().expect("dht client");
    let started = Instant::now();
    let record = loop {
        match tokio::time::timeout(Duration::from_secs(30), discovery.resolve(&id, &invite.secret)).await {
            Ok(Ok(Some(record))) => break record,
            Ok(Ok(None)) => eprintln!("[{:?}] no record yet", started.elapsed()),
            Ok(Err(e)) => eprintln!("resolve failed, retrying: {e}"),
            Err(_) => eprintln!("resolve timed out, retrying"),
        }
        assert!(started.elapsed() < Duration::from_secs(120), "no DHT record after 120 s");
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    println!("DHT record visible after {:?}: {record:?}", started.elapsed());

    // An invite with no peers forces Bob through the DHT record.
    let url = alice.engine.create_invite(id).await.expect("invite");
    let mut invite = Invite::parse(&url).expect("invite parses");
    invite.peers.clear();
    let dht_only = invite.to_url().to_string();

    let joining = Instant::now();
    let joined = bob
        .engine
        .join_community(&dht_only)
        .await
        .expect("bob joins over the DHT");
    println!("bob joined over the DHT in {:?}", joining.elapsed());
    assert_eq!(joined.id, server.id);

    let general = channel_id(&bob.engine, id, "general").expect("general channel");
    let sent_at = Instant::now();
    bob.engine
        .send_message(id, general, "hello from bob".into())
        .await
        .expect("bob posts");
    let alice_general = channel_id(&alice.engine, id, "general").expect("general channel");
    wait_seen(&alice, id, alice_general, "hello from bob").await;
    println!("alice received bob's message after {:?}", sent_at.elapsed());

    let sent_at = Instant::now();
    alice
        .engine
        .send_dm(bob_root, "hi bob".into())
        .await
        .expect("alice DMs bob");
    eventually_within(LIMIT, "bob receives alice's DM", || {
        dm_has(&bob.engine, alice_root, "hi bob").then_some(())
    })
    .await;
    println!("bob received alice's DM after {:?}", sent_at.elapsed());

    let sent_at = Instant::now();
    bob.engine
        .send_dm(alice_root, "hi alice".into())
        .await
        .expect("bob replies");
    eventually_within(LIMIT, "alice receives bob's reply", || {
        dm_has(&alice.engine, bob_root, "hi alice").then_some(())
    })
    .await;
    println!("alice received bob's reply after {:?}", sent_at.elapsed());

    // Both views must show a DHT size estimate and at least one gossip neighbour.
    for (name, engine) in [("alice", &alice.engine), ("bob", &bob.engine)] {
        let deadline = Instant::now() + LIMIT;
        let status = loop {
            let status = engine.net_status().await;
            let ready = status.dht.as_ref().is_some_and(|d| d.size_estimate > 0)
                && status.peers >= 1;
            if ready || Instant::now() >= deadline {
                break status;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        println!("{name} net_status: {status:?}");
        let dht = status.dht.as_ref().expect("dht stats present");
        assert!(dht.size_estimate > 0, "{name}: DHT size estimate is zero");
        assert!(status.peers >= 1, "{name}: no gossip neighbours");
    }

    finish(vec![alice, bob]).await;
    }).await;
}
