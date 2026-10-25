//! Replies, reactions, edits and deletes in direct messages, and a DM sent to a peer that is offline.
//! Loopback only; run through `scripts/fwtest.sh dissent-core it --ignored dm_`.

use std::time::{Duration, Instant};

use crate::common::*;
use dissent_core::{start_device_link, DmItem, Engine, ReactionView};
use dissent_identity::RootId;

/// The message with `text` in the direct conversation with `peer`, if it has arrived.
fn find_text(engine: &Engine, peer: RootId, text: &str) -> Option<DmItem> {
    engine.dm_messages(peer, 50).into_iter().find(|m| m.text == text)
}

/// The message with `id` in the direct conversation with `peer`, if it has arrived.
fn find_id(engine: &Engine, peer: RootId, id: &str) -> Option<DmItem> {
    engine.dm_messages(peer, 50).into_iter().find(|m| m.id == id)
}

fn reactions_on(engine: &Engine, peer: RootId, id: &str) -> Vec<ReactionView> {
    find_id(engine, peer, id).map(|m| m.reactions).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn dm_replies_and_reactions_show_on_both_sides() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let first = alice
        .engine
        .send_dm(bob_root, "ask me anything".into())
        .await
        .expect("alice writes");
    let on_bob = eventually("bob reads alice", || find_text(&bob.engine, alice_root, "ask me anything")).await;
    assert_eq!(on_bob.id, first.id, "both sides name the message the same way");

    // Bob answers. The reply quotes alice's message on both sides.
    bob.engine
        .dm_reply(alice_root, &first.id, "about the tools".into())
        .await
        .expect("bob replies");
    let reply = eventually("alice reads the reply", || find_text(&alice.engine, bob_root, "about the tools")).await;
    let quoted = reply.reply_to.as_ref().expect("the reply links its original");
    assert_eq!(quoted.id, first.id);
    assert_eq!(quoted.text.as_deref(), Some("ask me anything"));
    let own = find_text(&bob.engine, alice_root, "about the tools").expect("bob has his reply");
    assert_eq!(own.reply_to.and_then(|q| q.text).as_deref(), Some("ask me anything"));

    // Both react with the same emoji: two reactions on each side, and each side sees its own.
    bob.engine
        .dm_react(alice_root, &first.id, "🔥".into(), true)
        .await
        .expect("bob reacts");
    alice
        .engine
        .dm_react(bob_root, &first.id, "🔥".into(), true)
        .await
        .expect("alice reacts");
    for (name, engine, peer_root) in [("alice", &alice.engine, bob_root), ("bob", &bob.engine, alice_root)] {
        eventually(&format!("{name} counts two reactions"), || {
            reactions_on(engine, peer_root, &first.id)
                .iter()
                .any(|r| r.emoji == "🔥" && r.count == 2 && r.me)
                .then_some(())
        })
        .await;
    }

    // Taking a reaction back shows on both sides.
    alice
        .engine
        .dm_react(bob_root, &first.id, "🔥".into(), false)
        .await
        .expect("alice takes hers back");
    eventually("bob counts one reaction", || {
        reactions_on(&bob.engine, alice_root, &first.id)
            .iter()
            .any(|r| r.emoji == "🔥" && r.count == 1)
            .then_some(())
    })
    .await;

    // An edit replaces the text on both sides and is marked as edited.
    alice
        .engine
        .dm_edit(bob_root, &first.id, "ask me anything, really".into())
        .await
        .expect("alice edits");
    eventually("bob sees the edit", || {
        find_id(&bob.engine, alice_root, &first.id)
            .filter(|m| m.edited && m.text == "ask me anything, really")
            .map(|_| ())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn only_the_author_deletes_and_the_delete_reaches_every_device() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    // Bob links a tablet before the conversation exists, so the tablet is in the group from the start.
    let pending = start_device_link("Tablet").expect("link request");
    let grant = bob
        .engine
        .approve_device(&pending.link().expect("link"))
        .expect("bob approves the tablet");
    let tablet = start(pending.finish(&grant).expect("tablet identity"), scratch("tablet")).await;
    let invite = alice.engine.create_invite(community).await.expect("invite");
    tablet.engine.join_community(&invite).await.expect("tablet joins");
    tokio::time::sleep(Duration::from_secs(2)).await;

    let sent = alice
        .engine
        .send_dm(bob_root, "delete me".into())
        .await
        .expect("alice writes");
    eventually("bob reads it", || find_text(&bob.engine, alice_root, "delete me")).await;
    eventually("the tablet reads it", || find_text(&tablet.engine, alice_root, "delete me")).await;

    assert!(
        bob.engine.dm_delete(alice_root, &sent.id).await.is_err(),
        "bob may not delete alice's message"
    );
    alice
        .engine
        .dm_delete(bob_root, &sent.id)
        .await
        .expect("alice deletes her message");

    for (name, engine) in [("bob", &bob.engine), ("the tablet", &tablet.engine)] {
        eventually(&format!("{name} shows the delete"), || {
            find_id(engine, alice_root, &sent.id)
                .filter(|m| m.deleted && m.text.is_empty())
                .map(|_| ())
        })
        .await;
    }

    finish(vec![alice, bob, tablet]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_dm_to_an_offline_peer_returns_queued_and_arrives_later() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    // Talk both ways first, so the conversation has its group and bob's inboxes are known.
    alice
        .engine
        .send_dm(bob_root, "hello".into())
        .await
        .expect("alice writes");
    eventually("bob reads hello", || find_text(&bob.engine, alice_root, "hello")).await;
    bob.engine
        .send_dm(alice_root, "hi".into())
        .await
        .expect("bob writes");
    eventually("alice reads hi", || find_text(&alice.engine, bob_root, "hi")).await;

    let (identity, dir) = stop(bob).await;
    let started = Instant::now();
    let queued = tokio::time::timeout(
        Duration::from_secs(2),
        alice.engine.send_dm(bob_root, "while you were out".into()),
    )
    .await
    .expect("the send returns within 2 s")
    .expect("the send is accepted");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(queued.queued, "a message to an offline peer is queued, not sending: {}", queued.delivery);

    // Bob comes back, and the message arrives through the queue and the inbox.
    let bob = start(identity, dir).await;
    eventually_within(Duration::from_secs(90), "bob reads the queued message", || {
        find_text(&bob.engine, alice_root, "while you were out").map(|_| ())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn contact_since_and_first_seen_are_recorded() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let bob_root = bob.identity.root_id();

    let before = now();
    make_contacts_ready(&alice, &bob).await;
    let profile = alice.engine.user_profile(bob_root);
    let since = profile.contact_since_ms.expect("the contact has a date");
    assert!(since >= before, "the date is when the contact was made");
    assert!(profile.first_seen_ms.is_some(), "the first time we saw them is kept");

    // A DM row exists once there is a conversation; it carries the same date.
    alice.engine.send_dm(bob_root, "hi".into()).await.expect("alice writes");
    let dm = eventually("the DM is listed", || {
        alice.engine.conversations().into_iter().find(|c| c.peer == bob_root.to_string())
    })
    .await;
    assert_eq!(dm.contact_since_ms, Some(since));

    finish(vec![alice, bob]).await;
    })
    .await;
}
