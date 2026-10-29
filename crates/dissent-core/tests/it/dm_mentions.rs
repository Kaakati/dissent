//! Mentions in direct messages and group DMs: the mentioned member sees the highlight and an unread mention count,
//! the others do not, and marking the conversation read clears the count. Loopback only; run through
//! `scripts/fwtest.sh dissent-core it --ignored dm_mentions::`.

use crate::common::*;
use dissent_core::{ConvId, Mentions};

fn conv_named(p: &Peer, name: &str) -> Option<String> {
    p.engine
        .conversations()
        .into_iter()
        .find(|c| c.kind == "group" && c.name == name)
        .map(|c| c.id)
}

fn mention_count(p: &Peer, id: &str) -> u32 {
    p.engine.conversations().into_iter().find(|c| c.id == id).map_or(0, |c| c.mentions)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_group_mention_highlights_and_counts_for_the_mentioned_member_only() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    for other in [&bob, &carol] {
        make_contacts_ready(&alice, other).await;
    }
    make_contacts_ready(&bob, &carol).await;
    let bob_root = bob.identity.root_id();
    let carol_root = carol.identity.root_id();

    let created = alice
        .engine
        .create_group("trio".into(), vec![bob_root, carol_root])
        .await
        .expect("alice creates a group");
    let conv: ConvId = created.id.parse().expect("id parses");
    eventually("bob joins", || conv_named(&bob, "trio")).await;
    eventually("carol joins", || conv_named(&carol, "trio")).await;

    // `@everyone` is dropped in a conversation, so only bob is mentioned.
    let mentions = Mentions { users: vec![bob_root.0], everyone: true };
    let sent = alice
        .engine
        .send_to_conversation_with(conv, "@bob look".into(), mentions)
        .await
        .expect("alice mentions bob");
    assert!(!sent.mentions_me, "the author's own message is not highlighted for her");
    assert_eq!(sent.mentions.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec![bob_root.to_string()]);

    let on_bob = eventually("bob reads the mention", || {
        bob.engine.conversation_messages(conv, 10).into_iter().find(|m| m.text == "@bob look")
    })
    .await;
    assert!(on_bob.mentions_me, "bob's copy is highlighted");
    assert_eq!(on_bob.mentions.len(), 1);
    eventually("bob counts one unread mention", || (mention_count(&bob, &created.id) == 1).then_some(())).await;

    let on_carol = eventually("carol reads the message", || {
        carol.engine.conversation_messages(conv, 10).into_iter().find(|m| m.text == "@bob look")
    })
    .await;
    assert!(!on_carol.mentions_me, "carol is not mentioned");
    assert_eq!(on_carol.mentions.len(), 1, "carol still sees bob's pill");
    assert_eq!(mention_count(&carol, &created.id), 0);

    // A reply that mentions carol counts for her.
    carol
        .engine
        .conv_reply_with(conv, &on_carol.id, "on it".into(), Mentions::default())
        .await
        .expect("carol replies");
    bob.engine
        .conv_reply_with(conv, &on_bob.id, "@carol yours".into(), Mentions { users: vec![carol_root.0], everyone: false })
        .await
        .expect("bob replies with a mention");
    let reply = eventually("carol reads bob's reply", || {
        carol.engine.conversation_messages(conv, 10).into_iter().find(|m| m.text == "@carol yours")
    })
    .await;
    assert!(reply.mentions_me && reply.reply_to.is_some());
    eventually("carol counts one unread mention", || (mention_count(&carol, &created.id) == 1).then_some(())).await;

    // Marking the conversation read clears the count.
    bob.engine.mark_read(format!("group:{}", created.id)).expect("bob marks read");
    assert_eq!(mention_count(&bob, &created.id), 0);
    finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_direct_message_mention_reaches_the_peer() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .send_dm_with(bob_root, "hey @bob".into(), Mentions { users: vec![bob_root.0], everyone: false })
        .await
        .expect("alice mentions bob");
    let got = eventually("bob reads the mention", || {
        bob.engine.dm_messages(alice_root, 10).into_iter().find(|m| m.text == "hey @bob")
    })
    .await;
    assert!(got.mentions_me);
    let summary = eventually("bob's list counts the mention", || {
        bob.engine.conversations().into_iter().find(|c| c.peer == alice_root.to_string() && c.mentions == 1)
    })
    .await;
    assert_eq!(summary.unread, 1, "the mention is one of the unread messages");

    // A message with no mention stays plain text, as before.
    alice.engine.send_dm(bob_root, "plain".into()).await.expect("alice writes");
    let plain = eventually("bob reads the plain one", || {
        bob.engine.dm_messages(alice_root, 10).into_iter().find(|m| m.text == "plain")
    })
    .await;
    assert!(!plain.mentions_me && plain.mentions.is_empty());
    finish(vec![alice, bob]).await;
    })
    .await;
}
