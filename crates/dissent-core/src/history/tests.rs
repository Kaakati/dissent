//! Message history across versions. While an older member is in the community, messages keep the older form,
//! which joiners cannot read. A message sent before that member joined uses the history form.

use std::sync::atomic::Ordering;

use crate::membership::tests::{eventually, general, readable, Node};
use crate::testutil::bounded;

#[tokio::test(flavor = "multi_thread")]
async fn a_message_sent_while_an_older_member_is_present_stays_locked_for_joiners() {
    bounded(async {
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
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_with_every_other_member_offline_returns_quickly() {
    bounded(async {
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
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_history_form_switches_on_once_the_flag_is_set() {
    bounded(async {
        let alice = Node::named("alice-flag").await;
        let id = alice.engine.create_community("Flag".into()).await.expect("create").id.parse().unwrap();
        // Nobody else is a member, so there is nothing to ask.
        assert!(alice.engine.history_seal(id, "solo").is_some());

        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-flag").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        // A new member device has no answer yet, so the older form is used until a check runs.
        assert!(alice.engine.history_seal(id, "unknown").is_none());
        assert!(alice.engine.refresh_history_ready(id).await, "bob supports history messages");
        assert!(alice.engine.history_seal(id, "supported").is_some());

        // Bob turns support off, and the next check flips the flag back.
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        assert!(!alice.engine.refresh_history_ready(id).await);
        assert!(alice.engine.history_seal(id, "unsupported").is_none());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thousand_sends_read_the_key_file_at_most_once() {
    bounded(async {
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
    })
    .await;
}
