//! Sealed direct messages and group DMs over MLS. Loopback only, relays off.


use crate::common::*;
use dissent_core::{ConvId, CoreError};

fn conv_named(p: &Peer, name: &str) -> Option<String> {
    p.engine
        .conversations()
        .into_iter()
        .find(|c| c.kind == "group" && c.name == name)
        .map(|c| c.id)
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_messages_are_sealed_and_flow_both_ways() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    // Both sides write before either conversation exists. Each keeps its text until the MLS group
    // is ready, and the two first messages must both arrive.
    alice
        .engine
        .send_dm(bob_root, "from alice".into())
        .await
        .expect("alice writes");
    bob
        .engine
        .send_dm(alice_root, "from bob".into())
        .await
        .expect("bob writes");

    eventually("bob reads alice", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.text == "from alice")
            .then_some(())
    })
    .await;
    eventually("alice reads bob", || {
        alice
            .engine
            .dm_messages(bob_root, 10)
            .iter()
            .any(|m| m.text == "from bob")
            .then_some(())
    })
    .await;

    // The conversation is now a sealed MLS group on both sides, and both lists show a 1:1 DM.
    let dm = alice
        .engine
        .conversations()
        .into_iter()
        .find(|c| c.peer == bob_root.to_string())
        .expect("alice lists the DM");
    assert_eq!(dm.kind, "dm");
    assert_eq!(dm.members.len(), 2);

    let conv: ConvId = dm.id.parse().expect("conversation id parses");
    let later = alice
        .engine
        .send_to_conversation(conv, "sealed reply".into())
        .await
        .expect("alice sends into the conversation");
    assert_eq!(later.text, "sealed reply");
    eventually("bob reads the sealed reply", || {
        bob.engine
            .conversation_messages(conv, 10)
            .iter()
            .any(|m| m.text == "sealed reply")
            .then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn group_dm_create_message_rename_and_remove() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let erin = peer("erin").await;
    for other in [&bob, &carol, &dave, &erin] {
        make_contacts_ready(&alice, other).await;
    }
    for (x, y) in [(&bob, &carol), (&bob, &dave), (&carol, &dave)] {
        make_contacts_ready(x, y).await;
    }
    let bob_root = bob.identity.root_id();
    let carol_root = carol.identity.root_id();
    let dave_root = dave.identity.root_id();

    let created = alice
        .engine
        .create_group("club".into(), vec![bob_root, carol_root, dave_root])
        .await
        .expect("alice creates the group");
    assert_eq!(created.kind, "group");
    assert_eq!(created.members.len(), 4);
    let conv: ConvId = created.id.parse().expect("id parses");

    eventually("bob joins", || conv_named(&bob, "club")).await;
    eventually("dave joins", || conv_named(&dave, "club")).await;
    eventually("carol joins", || conv_named(&carol, "club")).await;

    alice
        .engine
        .send_to_conversation(conv, "hello club".into())
        .await
        .expect("alice writes");
    for p in [&bob, &carol, &dave] {
        eventually("member reads", || {
            p.engine
                .conversation_messages(conv, 10)
                .iter()
                .any(|m| m.text == "hello club")
                .then_some(())
        })
        .await;
    }

    // Any member may rename. The rename reaches everyone.
    bob.engine
        .group_rename(conv, "crew".into())
        .await
        .expect("bob renames");
    eventually("alice sees the rename", || conv_named(&alice, "crew")).await;
    eventually("carol sees the rename", || conv_named(&carol, "crew")).await;

    // Only the creator removes. Bob may not remove Carol.
    let refused = bob.engine.group_remove(conv, carol_root).await;
    assert!(matches!(refused, Err(CoreError::Rejected(_))));

    // The creator removes Dave. Dave is no longer listed, and Carol still reads the group.
    alice
        .engine
        .group_remove(conv, dave_root)
        .await
        .expect("alice removes dave");
    eventually("dave is out", || {
        dave.engine
            .conversations()
            .iter()
            .all(|c| c.id != conv.to_string())
            .then_some(())
    })
    .await;
    alice
        .engine
        .send_to_conversation(conv, "without dave".into())
        .await
        .expect("alice writes");
    eventually("carol reads after removal", || {
        carol
            .engine
            .conversation_messages(conv, 10)
            .iter()
            .any(|m| m.text == "without dave")
            .then_some(())
    })
    .await;

    // Carol leaves. Alice, as creator, removes her, and Carol no longer lists the group.
    carol
        .engine
        .group_leave(conv)
        .await
        .expect("carol leaves");
    eventually("alice drops carol", || {
        (alice
            .engine
            .conversations()
            .iter()
            .find(|c| c.id == conv.to_string())
            .map(|c| c.members.len())
            == Some(2))
        .then_some(())
    })
    .await;
    assert!(carol
        .engine
        .conversations()
        .iter()
        .all(|c| c.id != conv.to_string()));

    finish(vec![alice, bob, carol, dave, erin]).await;
    }).await;
}

/// Counts the group announcements for one conversation that a receiver holds.
fn group_announcements(rx: &mut tokio::sync::broadcast::Receiver<dissent_core::EngineEvent>, conv: ConvId) -> usize {
    let mut n = 0;
    while let Ok(ev) = rx.try_recv() {
        if matches!(&ev, dissent_core::EngineEvent::Group { group_id } if *group_id == conv.to_string()) {
            n += 1;
        }
    }
    n
}

/// A new group message is announced to each member once. A rename changes the conversation and announces no message.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_message_is_announced_once() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    for (x, y) in [(&alice, &bob), (&alice, &carol), (&alice, &dave), (&bob, &carol), (&bob, &dave), (&carol, &dave)] {
        make_contacts_ready(x, y).await;
    }
    let created = alice
        .engine
        .create_group("trio".into(), vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()])
        .await
        .expect("alice creates the group");
    let conv: ConvId = created.id.parse().expect("id parses");
    eventually("bob joins", || conv_named(&bob, "trio")).await;
    eventually("carol joins", || conv_named(&carol, "trio")).await;
    eventually("dave joins", || conv_named(&dave, "trio")).await;
    let mut events = bob.engine.subscribe();

    alice
        .engine
        .send_to_conversation(conv, "announced once".into())
        .await
        .expect("alice writes");
    eventually("bob reads", || {
        bob.engine
            .conversation_messages(conv, 10)
            .iter()
            .any(|m| m.text == "announced once")
            .then_some(())
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(group_announcements(&mut events, conv), 1, "one announcement for the new message");

    alice
        .engine
        .group_rename(conv, "trio renamed".into())
        .await
        .expect("alice renames");
    eventually("bob sees the rename", || conv_named(&bob, "trio renamed")).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(group_announcements(&mut events, conv), 0, "a rename announces no message");

    finish(vec![alice, bob, carol, dave]).await;
    }).await;
}

/// A new direct message is announced to its recipient once, as a Dm event. A change to the conversation announces
/// no message.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_message_is_announced_once() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    let mut events = bob.engine.subscribe();

    alice
        .engine
        .send_dm(bob_root, "announced once".into())
        .await
        .expect("alice writes");
    eventually("bob reads", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.text == "announced once")
            .then_some(())
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut dms = 0;
    while let Ok(ev) = events.try_recv() {
        if matches!(&ev, dissent_core::EngineEvent::Dm { peer } if *peer == alice_root.to_string()) {
            dms += 1;
        }
    }
    assert_eq!(dms, 1, "one direct-message announcement");
    finish(vec![alice, bob]).await;
    }).await;
}
