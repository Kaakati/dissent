//! Sealed direct messages and group DMs over MLS. Loopback only, relays off.

mod common;

use common::*;
use dissent_core::{ConvId, CoreError};

/// Makes `a` and `b` know each other's devices and addresses, through a contact request each way.
async fn meet(a: &Peer, b: &Peer) {
    a.engine
        .request_contact(&b.engine.contact_link(), "")
        .await
        .expect("a asks b");
    b.engine
        .request_contact(&a.engine.contact_link(), "")
        .await
        .expect("b asks a");
}

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
        meet(&alice, other).await;
    }
    for (x, y) in [(&bob, &carol), (&bob, &dave), (&carol, &dave)] {
        meet(x, y).await;
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
