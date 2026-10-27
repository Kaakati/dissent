//! @mentions on loopback: a mention reaches the person it names and counts once as unread, and
//! nobody else counts it. `@everyone` counts only from the owner and admins. A member's `@everyone`
//! is ignored, even when the member writes it into the payload by hand.

use crate::common::*;
use dissent_core::{ChannelId, CommunityId, Engine, Mentions, ModAction};
use dissent_identity::RootId;
use std::time::Duration;

/// The channel's message with this text, as `engine` sees it.
fn seen(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> Option<dissent_core::MessageItem> {
    engine.channel_messages(id, channel, 100).into_iter().find(|m| m.text == text)
}

/// Unread mentions in `general`, from the community view.
fn unread(engine: &Engine, id: CommunityId) -> u32 {
    engine
        .community_view(id)
        .expect("community view")
        .channels
        .iter()
        .find(|c| c.name == "general")
        .map_or(0, |c| c.mentions)
}

async fn wait_for_message(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> dissent_core::MessageItem {
    until_event(engine, Duration::from_secs(60), &format!("{text:?} to arrive"), || {
        seen(engine, id, channel, text)
    })
    .await
}

fn users(roots: &[RootId]) -> Mentions {
    Mentions {
        users: roots.iter().map(|r| r.0).collect(),
        everyone: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_mention_reaches_the_person_it_names_and_nobody_else() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let id = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &carol, &invite).await;
    let channel = channel_id(&alice.engine, id, "general").expect("general");

    alice
        .engine
        .send_message_with(id, channel, "over to you @bob".into(), users(&[bob.identity.root_id()]))
        .await
        .expect("alice mentions bob");

    let for_bob = wait_for_message(&bob.engine, id, channel, "over to you @bob").await;
    assert!(for_bob.mentions_me, "bob is mentioned");
    assert_eq!(for_bob.mentions.len(), 1);
    assert_eq!(for_bob.mentions[0].id, bob.identity.root_id().to_string());
    assert_eq!(unread(&bob.engine, id), 1, "bob has one unread mention");

    // Carol sees the message and the name, but is not mentioned.
    let for_carol = wait_for_message(&carol.engine, id, channel, "over to you @bob").await;
    assert!(!for_carol.mentions_me, "carol is not mentioned");
    assert_eq!(unread(&carol.engine, id), 0, "carol has no unread mention");

    // Reading clears bob's count.
    bob.engine.mark_mentions_read(id, channel).expect("mark read");
    assert_eq!(unread(&bob.engine, id), 0, "marking read clears the count");

    finish(vec![alice, bob, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn everyone_counts_from_an_admin_and_not_from_a_member() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let id = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &carol, &invite).await;
    let channel = channel_id(&alice.engine, id, "general").expect("general");
    let everyone = Mentions { users: Vec::new(), everyone: true };

    // The owner promotes bob. Bob's @everyone reaches everyone.
    alice
        .engine
        .moderate(id, ModAction::GrantAdmin, bob.identity.root_id())
        .await
        .expect("grant admin");
    // Bob's @everyone is valid only where bob is an admin, so it waits until bob's own replica shows the grant.
    // Carol's copy is waited on too, since she is one of the receivers this test checks.
    let bob_root = bob.identity.root_id().to_string();
    let is_admin = |engine: &Engine| {
        engine
            .community_view(id)
            .ok()
            .filter(|v| v.members.iter().any(|m| m.root_id == bob_root && m.role == "admin"))
            .map(|_| ())
    };
    until_event(&carol.engine, Duration::from_secs(60), "carol sees the admin", || is_admin(&carol.engine)).await;
    until_event(&bob.engine, Duration::from_secs(60), "bob sees his own grant", || is_admin(&bob.engine)).await;
    bob.engine
        .send_message_with(id, channel, "attention all".into(), everyone.clone())
        .await
        .expect("bob sends @everyone");
    assert!(wait_for_message(&alice.engine, id, channel, "attention all").await.mentions_me, "the owner is mentioned");
    assert!(wait_for_message(&carol.engine, id, channel, "attention all").await.mentions_me, "a member is mentioned");
    assert_eq!(unread(&carol.engine, id), 1);

    // Carol is a member. Her @everyone reaches nobody, including a copy written by hand.
    carol
        .engine
        .send_message_with(id, channel, "carol shouts".into(), everyone)
        .await
        .expect("carol sends @everyone");
    assert!(!wait_for_message(&alice.engine, id, channel, "carol shouts").await.mentions_me, "a member's @everyone is ignored");
    assert!(!wait_for_message(&bob.engine, id, channel, "carol shouts").await.mentions_me);
    assert_eq!(unread(&alice.engine, id), 1, "only bob's @everyone counts for the owner");

    finish(vec![alice, bob, carol]).await;
    }).await;
}
