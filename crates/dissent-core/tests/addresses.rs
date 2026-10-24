//! A peer that restarts on a new port stays reachable. Relays are off, so Alice can only learn
//! Bob's new address from his own signed announcement.

mod common;

use common::*;
use dissent_core::Identity;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn restarted_peer_is_reachable_on_new_port() {
    bounded(async {
    let alice = peer("alice").await;
    let first_port = free_port();
    let bob = start_on(Identity::create(now()), scratch("bob-addr"), first_port).await;
    bob.engine
        .set_display_name("bob".into())
        .await
        .expect("name");
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .send_dm(bob_root, "before restart".into())
        .await
        .expect("alice DMs bob");
    eventually("bob has the first DM", || {
        dm_has(&bob.engine, alice_root, "before restart").then_some(())
    })
    .await;

    // Bob restarts on a different port. His old address is now wrong for everyone.
    let mut second_port = free_port();
    while second_port == first_port {
        second_port = free_port();
    }
    let (identity, dir) = stop(bob).await;
    let bob = start_on(identity, dir, second_port).await;

    // Bob's startup announcement reaches Alice, who then dials the new port. Until it lands,
    // a send waits in the outbox and reports pending, so retry until one is delivered.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let sent = alice
            .engine
            .send_dm(bob_root, "after restart".into())
            .await
            .expect("alice DMs bob");
        if !sent.pending {
            break;
        }
        assert!(Instant::now() < deadline, "alice never reached bob's new address");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eventually("bob receives the message after restart", || {
        dm_has(&bob.engine, alice_root, "after restart").then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    }).await;
}
