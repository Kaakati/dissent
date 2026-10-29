//! A member who joins after messages were posted. Its text is sealed under the history key of its epoch.
//! Members hand those keys to joiners by the server's history setting (`docs/design/05-encrypted-servers.md`,
//! "History messages"): `All` shows the earlier text, `SinceJoin` keeps it locked.

use crate::common::*;
use dissent_core::{ChannelId, CommunityId, HistorySetting};

const EARLIER: [&str; 3] = ["Hala", "test", "works"];

/// A server whose owner chose `All`, so joiners get the keys of the earlier messages. New servers default to
/// `SinceJoin`, which the last test checks.
async fn server_with_three_messages() -> (Peer, CommunityId, ChannelId, String) {
    let alice = peer("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Eclipse".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    alice
        .engine
        .set_history_visibility(id, HistorySetting::All)
        .await
        .expect("owner chooses all");
    let channel = channel_id(&alice.engine, id, "general").expect("general channel");
    for text in EARLIER {
        alice.engine.send_message(id, channel, text.into()).await.expect("send");
    }
    let link = alice.engine.create_invite(id).await.expect("invite");
    (alice, id, channel, link)
}

/// Waits until every earlier message reads, then returns the channel's messages.
async fn readable_earlier(peer: &Peer, id: CommunityId, channel: ChannelId) -> Vec<dissent_core::MessageItem> {
    eventually("the earlier messages read", || {
        let items: Vec<_> = peer.engine.channel_messages(id, channel, 50).into_iter().filter(|m| m.text != "after").collect();
        (items.len() == EARLIER.len() && items.iter().all(|m| !m.locked && !m.text.is_empty())).then_some(items)
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_joiner_reads_the_earlier_messages_under_the_default_history_setting() {
    bounded(async {
        let (alice, id, channel, link) = server_with_three_messages().await;
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        let items = readable_earlier(&bob, id, channel).await;
        let texts: Vec<&str> = items.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, EARLIER);
        assert!(items.iter().all(|m| !m.before_join || m.locked), "{items:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_after_the_join_reads_too() {
    bounded(async {
        let (alice, id, channel, link) = server_with_three_messages().await;
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        alice.engine.send_message(id, channel, "after".into()).await.expect("send");
        wait_seen(&bob, id, channel, "after").await;
        readable_earlier(&bob, id, channel).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn since_join_keeps_the_earlier_messages_locked() {
    bounded(async {
        let (alice, id, channel, link) = server_with_three_messages().await;
        alice
            .engine
            .set_history_visibility(id, HistorySetting::SinceJoin)
            .await
            .expect("owner keeps since-join history");
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        alice.engine.send_message(id, channel, "after".into()).await.expect("send");
        wait_seen(&bob, id, channel, "after").await;
        // Give the key request time to run and be refused.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let items = bob.engine.channel_messages(id, channel, 50);
        let earlier: Vec<_> = items.iter().filter(|m| m.text != "after").collect();
        assert_eq!(earlier.len(), EARLIER.len(), "{items:?}");
        assert!(earlier.iter().all(|m| m.locked && m.text.is_empty() && m.before_join), "{items:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_earlier_messages_stay_readable_after_the_sender_goes_offline() {
    bounded(async {
        let (alice, id, channel, link) = server_with_three_messages().await;
        // Carol joins while Alice is online, so she holds the keys she can pass on.
        let carol = peer("carol").await;
        join_ready(&alice, &carol, &link).await;
        readable_earlier(&carol, id, channel).await;
        // Bob joins, and Alice goes offline right after admitting him.
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        stop(alice).await;
        readable_earlier(&bob, id, channel).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_right_after_a_join_reads_for_the_joiner_with_no_warm_up() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Warm".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("owner shares past messages");
        let channel = channel_id(&alice.engine, id, "general").expect("general channel");
        let link = alice.engine.create_invite(id).await.expect("invite");
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        // No warm-up message: the first send after the join is the one that must read.
        alice.engine.send_message(id, channel, "first after the join".into()).await.expect("send");
        wait_seen(&bob, id, channel, "first after the join").await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_joiner_reads_a_message_sent_right_after_the_join_within_three_seconds() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Quick".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("owner shares past messages");
        let channel = channel_id(&alice.engine, id, "general").expect("general channel");
        let link = alice.engine.create_invite(id).await.expect("invite");
        let bob = peer("bob").await;
        join_ready(&alice, &bob, &link).await;
        alice.engine.send_message(id, channel, "right after the join".into()).await.expect("send");
        let started = std::time::Instant::now();
        loop {
            let read = bob
                .engine
                .channel_messages(id, channel, 50)
                .iter()
                .any(|m| m.text == "right after the join" && !m.locked);
            if read {
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(3), "not readable after {:?}", started.elapsed());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
}

/// A member that did not admit the joiner (bob) keeps the history form for the joiner's first message without
/// asking anyone: carol's join carries her `HistorySupport`. Bob's checks reach nobody here, so only that event
/// can count her. Carol reads the message.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_that_did_not_admit_the_joiner_sends_it_the_history_form_at_once() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("owner shares past messages");
        let channel = channel_id(&bob.engine, id, "general").expect("general channel");
        let link = alice.engine.create_invite(id).await.expect("invite");
        eventually("bob is ready for the history form", || {
            dissent_core::test_support::history_ready_now(&bob.engine, id).then_some(())
        })
        .await;

        dissent_core::test_support::set_history_probes(&bob.engine, false);
        let carol = peer("carol").await;
        join_ready(&alice, &carol, &link).await;
        let carol_root = carol.identity.root_id().to_string();
        eventually("bob lists carol", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.members.iter().any(|m| m.root_id == carol_root))
        })
        .await;
        eventually("bob counts carol without asking", || {
            dissent_core::test_support::history_ready_now(&bob.engine, id).then_some(())
        })
        .await;
        bob.engine.send_message(id, channel, "hello carol".into()).await.expect("send");
        wait_seen(&carol, id, channel, "hello carol").await;
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}
