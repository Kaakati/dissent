//! Message history across versions. While an older member is in the community, messages keep the older form,
//! which joiners cannot read. A message sent before that member joined uses the history form.

use std::sync::atomic::Ordering;

use crate::membership::tests::{eventually, general, readable, Node};
use crate::testutil::on_own_thread;

#[test]
fn a_message_sent_while_an_older_member_is_present_stays_locked_for_joiners() {
    on_own_thread(|| async {
        let alice = Node::named("alice-mixed").await;
        let id = alice.engine.create_community("Mixed".into()).await.expect("create").id.parse().unwrap();
        alice
            .engine
            .set_history_visibility(id, dissent_events::HistorySetting::All)
            .await
            .expect("owner chooses all");
        let channel = general(&alice.engine, id);
        let first = alice.engine.send_message(id, channel, "first".into()).await.expect("send first");
        assert!(alice.engine.history_seal(id, "probe").is_some(), "a community of one uses the history form");

        // Bob is on a version that predates history messages.
        let bob = Node::named("bob-mixed").await;
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        let invite = alice.engine.create_invite(id).await.expect("invite");
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;

        let second = alice.engine.send_message(id, channel, "second".into()).await.expect("send second");
        assert!(alice.engine.history_seal(id, "probe").is_none(), "an older member means the older form");

        let carol = Node::named("carol-mixed").await;
        carol.engine.join_community(&invite).await.expect("carol joins");
        eventually("carol reads the first message in the history form", || {
            readable(&carol.engine, id, channel, "first")
        })
        .await;
        eventually("carol sees the second message locked", || {
            carol
                .engine
                .channel_messages(id, channel, 50)
                .iter()
                .any(|m| m.id == second.id && m.locked && m.text.is_empty())
        })
        .await;
        assert!(first.id != second.id);
    });
}

#[test]
fn a_send_with_every_other_member_offline_returns_quickly() {
    on_own_thread(|| async {
        let alice = Node::named("alice-offline").await;
        let id = alice.engine.create_community("Quiet".into()).await.expect("create").id.parse().unwrap();
        let channel = general(&alice.engine, id);
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-offline").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        // Bob goes offline, so no check of his device can answer.
        let _ = bob.stop().await;

        let started = std::time::Instant::now();
        alice.engine.send_message(id, channel, "into the void".into()).await.expect("send");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "a send took {:?}", started.elapsed());
    });
}

#[test]
fn the_history_form_switches_on_once_the_flag_is_set() {
    on_own_thread(|| async {
        let alice = Node::named("alice-flag").await;
        let id = alice.engine.create_community("Flag".into()).await.expect("create").id.parse().unwrap();
        // Nobody else is a member, so there is nothing to ask.
        assert!(alice.engine.history_seal(id, "solo").is_some());

        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-flag").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        // Alice admitted bob, and checked his devices before the welcome, so the answer is already known.
        assert!(alice.engine.history_seal(id, "admitted").is_some());
        assert!(alice.engine.refresh_history_ready(id).await, "bob supports history messages");
        assert!(alice.engine.history_seal(id, "supported").is_some());

        // Bob turns support off, and the next check flips the flag back.
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        assert!(!alice.engine.refresh_history_ready(id).await);
        assert!(alice.engine.history_seal(id, "unsupported").is_none());
    });
}

#[test]
fn a_thousand_sends_read_the_key_file_at_most_once() {
    on_own_thread(|| async {
        let alice = Node::named("alice-reads").await;
        let id = alice.engine.create_community("Reads".into()).await.expect("create").id.parse().unwrap();
        let before = alice.engine.inner.history_file_reads.load(Ordering::Relaxed);
        let started = std::time::Instant::now();
        for n in 0..1000 {
            assert!(alice.engine.history_seal(id, &format!("message {n}")).is_some(), "send {n}");
        }
        let reads = alice.engine.inner.history_file_reads.load(Ordering::Relaxed) - before;
        assert!(reads <= 1, "the key file was read {reads} times");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "1000 sends took {:?}", started.elapsed());
    });
}

#[test]
fn the_new_format_switches_on_within_two_seconds_of_the_last_unsupported_member_updating() {
    on_own_thread(|| async {
        let alice = Node::named("alice-wake").await;
        let id = alice.engine.create_community("Wake".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-wake").await;
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(!alice.engine.refresh_history_ready(id).await, "bob does not support it yet");
        assert!(alice.engine.history_seal(id, "before").is_none());

        // Bob updates, and his presence beacon reaches Alice.
        bob.engine.inner.history_support.store(true, Ordering::Relaxed);
        alice.engine.history_member_seen(bob.identity.root_id());
        let started = std::time::Instant::now();
        while alice.engine.history_seal(id, "after").is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(2), "still the older form after {:?}", started.elapsed());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
}

#[test]
fn a_restart_starts_from_the_last_readiness_answer() {
    on_own_thread(|| async {
        let alice = Node::named("alice-restart-ready").await;
        let id = alice.engine.create_community("Restart".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-restart-ready").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(alice.engine.refresh_history_ready(id).await, "bob supports it");

        let (identity, dir, port) = alice.stop().await;
        let again = Node::boot(identity, dir, port).await;
        // No check runs after the restart: the saved "yes" is the answer, so the first send is in the new form.
        assert!(again.engine.history_seal(id, "right after the restart").is_some());
    });
}

#[test]
fn a_member_admitted_by_us_is_counted_before_the_welcome_goes_out() {
    on_own_thread(|| async {
        let alice = Node::named("alice-admit").await;
        let id = alice.engine.create_community("Admit".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-admit").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        // Alice has admitted bob, so the answer for his devices is already known, with no check left to run.
        eventually("alice has admitted bob", || {
            alice.engine.community_view(id).map(|v| v.members.len() == 2).unwrap_or(false)
        })
        .await;
        assert!(alice.engine.history_seal(id, "no check to wait for").is_some());
    });
}
