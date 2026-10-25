//! Set reconciliation: a joiner gets a long history in one go, and two diverged histories
//! converge in both directions.


use crate::common::*;
use dissent_core::Identity;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn joiner_receives_six_thousand_messages() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let server = alice
        .engine
        .create_community("Bulk".into())
        .await
        .expect("create community");
    let id: dissent_core::CommunityId = server.id.parse().unwrap();
    let general = channel_id(&alice.engine, id, "general").expect("general channel");

    let started = Instant::now();
    for i in 0..6000 {
        alice
            .engine
            .send_message(id, general, format!("m{i}"))
            .await
            .expect("alice posts");
    }
    eprintln!("authored 6000 messages in {:?}", started.elapsed());

    let invite = alice.engine.create_invite(id).await.expect("invite");
    let joined_at = Instant::now();
    bob.engine
        .join_community(&invite)
        .await
        .expect("bob joins");
    eprintln!("bob joined in {:?}", joined_at.elapsed());

    eventually("bob holds all 6000 messages", || {
        (bob.engine.channel_messages(id, general, 100_000).len() == 6000).then_some(())
    })
    .await;
    // Bob joined after these were sent, so the group could not decrypt them. They are locked.
    let items = bob.engine.channel_messages(id, general, 100_000);
    assert_eq!(items.iter().filter(|m| m.locked).count(), 6000, "all locked for a late joiner");

    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_diverged_histories_converge() {
    bounded(async {
    let (port_a, port_b) = (free_port(), free_port());
    let alice = start_on(Identity::create(now()), scratch("alice-sync"), port_a).await;
    alice
        .engine
        .set_display_name("alice".into())
        .await
        .expect("name");
    let bob = start_on(Identity::create(now()), scratch("bob-sync"), port_b).await;
    bob.engine.set_display_name("bob".into()).await.expect("name");
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general channel");

    // Bob is away while Alice writes.
    let (bob_id, bob_dir) = stop(bob).await;
    for i in 0..40 {
        alice
            .engine
            .send_message(id, general, format!("alice-{i}"))
            .await
            .expect("alice posts");
    }

    // Alice is away while Bob writes. Bob restarts, so he has Alice's address and nothing new.
    let (alice_id, alice_dir) = stop(alice).await;
    let bob = start_on(bob_id, bob_dir, port_b).await;
    for i in 0..30 {
        bob.engine
            .send_message(id, general, format!("bob-{i}"))
            .await
            .expect("bob posts");
    }

    // Alice comes back. Her next message may reach Bob before his gossip neighbours are up, so
    // keep posting until each side holds the other's history.
    let alice = start_on(alice_id, alice_dir, port_a).await;
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut n = 0;
    loop {
        let converged = count_prefixed(&bob.engine, id, general, "alice-") == 40
            && count_prefixed(&alice.engine, id, general, "bob-") == 30;
        if converged {
            break;
        }
        assert!(Instant::now() < deadline, "histories did not converge");
        alice
            .engine
            .send_message(id, general, format!("ping-{n}"))
            .await
            .expect("alice pings");
        n += 1;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    for i in 0..40 {
        assert!(has_message(&bob.engine, id, general, &format!("alice-{i}")));
    }
    for i in 0..30 {
        assert!(has_message(&alice.engine, id, general, &format!("bob-{i}")));
    }

    finish(vec![alice, bob]).await;
    }).await;
}
