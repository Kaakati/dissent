//! Replies and reactions on loopback: a reply shows a preview of its original, and reactions
//! count per emoji, with the reactor's own mark and names.


use crate::common::*;
use dissent_core::{ChannelId, CommunityId, Engine, EventId};
use std::time::{Duration, Instant};

fn item(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> Option<dissent_core::MessageItem> {
    engine
        .channel_messages(id, channel, 100)
        .into_iter()
        .find(|m| m.text == text)
}

async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = check() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn replies_show_a_preview_and_reactions_count_per_emoji() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let channel = channel_id(&alice.engine, id, "general").expect("general");

    let original = alice
        .engine
        .send_message(id, channel, "the original".into())
        .await
        .expect("alice posts");
    let target: EventId = original.id.parse().expect("event id");

    // Bob can only reply to a message he has: wait until it reaches him.
    wait_for("bob sees the original", || item(&bob.engine, id, channel, "the original")).await;

    // Bob replies. His reply shows a preview of the original.
    let reply = bob
        .engine
        .send_reply(id, channel, target, "a reply".into())
        .await
        .expect("bob replies");
    assert_eq!(reply.reply_to.as_ref().and_then(|r| r.text.clone()).as_deref(), Some("the original"));

    // Alice sees the reply with the same preview, once it arrives.
    let seen = wait_for("alice sees the reply", || item(&alice.engine, id, channel, "a reply")).await;
    let preview = seen.reply_to.expect("preview");
    assert_eq!(preview.id, original.id);
    assert_eq!(preview.text.as_deref(), Some("the original"));
    assert!(!preview.locked);

    // Bob reacts. Alice sees the count, and Bob sees his own mark.
    bob.engine
        .react(id, channel, target, "👍".into())
        .await
        .expect("bob reacts");
    let for_alice = wait_for("alice sees the reaction", || {
        let msg = item(&alice.engine, id, channel, "the original")?;
        msg.reactions.first().filter(|r| r.count == 1).cloned()
    })
    .await;
    assert_eq!(for_alice.emoji, "👍");
    assert!(!for_alice.me, "alice did not react");
    assert_eq!(for_alice.names, vec!["bob".to_string()]);
    let for_bob = wait_for("bob sees his own reaction", || {
        let msg = item(&bob.engine, id, channel, "the original")?;
        msg.reactions.first().filter(|r| r.count == 1).cloned()
    })
    .await;
    assert!(for_bob.me);

    // Bob takes it back, and the reaction disappears.
    bob.engine
        .unreact(id, channel, target, "👍".into())
        .await
        .expect("bob unreacts");
    wait_for("the reaction is gone for alice", || {
        let msg = item(&alice.engine, id, channel, "the original")?;
        msg.reactions.is_empty().then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    }).await;
}
