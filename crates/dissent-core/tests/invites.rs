//! Group invite links: a holder asks, the creator approves, and revoked links are refused.

mod common;

use common::*;
use dissent_core::ConvId;

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

async fn group_of_three(alice: &Peer, others: [&Peer; 3]) -> ConvId {
    for o in others {
        meet(alice, o).await;
    }
    let roots: Vec<_> = others.iter().map(|o| o.identity.root_id()).collect();
    let created = alice
        .engine
        .create_group("club".into(), roots)
        .await
        .expect("alice creates the group");
    created.id.parse().expect("id parses")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_holder_asks_and_the_creator_approves() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let erin = peer("erin").await;
    let conv = group_of_three(&alice, [&bob, &carol, &dave]).await;

    let link = alice.engine.create_group_invite(conv).expect("alice issues a link");
    assert!(link.starts_with("dissent://group/"));
    erin.engine
        .join_group_by_invite(&link)
        .await
        .expect("erin asks to join");

    let erin_root = erin.identity.root_id();
    eventually("alice sees erin's request", || {
        alice
            .engine
            .group_join_requests(conv)
            .iter()
            .any(|r| r.user == erin_root.to_string())
            .then_some(())
    })
    .await;
    // Nothing happens until the creator approves.
    assert!(!erin.engine.conversations().iter().any(|c| c.id == conv.to_string()));

    alice
        .engine
        .approve_group_join(conv, erin_root)
        .await
        .expect("alice approves erin");
    eventually("erin joins the group", || {
        erin.engine
            .conversations()
            .iter()
            .any(|c| c.id == conv.to_string() && c.kind == "group")
            .then_some(())
    })
    .await;

    finish(vec![alice, bob, carol, dave, erin]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_link_is_refused() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let frank = peer("frank").await;
    let conv = group_of_three(&alice, [&bob, &carol, &dave]).await;

    let old = alice.engine.create_group_invite(conv).expect("link");
    alice
        .engine
        .revoke_group_invites(conv)
        .expect("alice revokes");
    let refused = frank.engine.join_group_by_invite(&old).await;
    assert!(refused.is_err(), "a link issued before the revoke is refused");
    assert!(alice.engine.group_join_requests(conv).is_empty());

    finish(vec![alice, bob, carol, dave, frank]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_creator_issues_invites() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let conv = group_of_three(&alice, [&bob, &carol, &dave]).await;
    assert!(bob.engine.create_group_invite(conv).is_err());
    finish(vec![alice, bob, carol, dave]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_welcome_from_someone_else_does_not_pull_you_in() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let erin = peer("erin").await;
    // Erin takes direct contact only from people she has accepted.
    erin.engine
        .set_dm_policy(dissent_core::DmPolicy::ContactsOnly)
        .expect("policy set");
    let conv = group_of_three(&alice, [&bob, &carol, &dave]).await;

    let link = alice.engine.create_group_invite(conv).expect("alice issues a link");
    erin.engine.join_group_by_invite(&link).await.expect("erin asks");
    eventually("alice sees erin's request", || {
        (!alice.engine.group_join_requests(conv).is_empty()).then_some(())
    })
    .await;

    // Carol knows Erin's device, but Erin never accepted her. She makes a group with Erin in it.
    carol
        .engine
        .request_contact(&erin.engine.contact_link(), "")
        .await
        .expect("carol asks erin");
    meet(&carol, &bob).await;
    meet(&carol, &dave).await;
    carol
        .engine
        .create_group("trap".into(), vec![erin.identity.root_id(), bob.identity.root_id(), dave.identity.root_id()])
        .await
        .expect("carol makes a group");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(
        erin.engine.conversations().iter().all(|c| c.name != "trap"),
        "a welcome from someone who is not the creator is refused"
    );

    // Alice approves. The welcome from her device is the one the link named, so it is accepted.
    alice
        .engine
        .approve_group_join(conv, erin.identity.root_id())
        .await
        .expect("alice approves");
    eventually("erin joins the group she asked for", || {
        erin.engine
            .conversations()
            .iter()
            .any(|c| c.id == conv.to_string())
            .then_some(())
    })
    .await;
    assert_eq!(erin.engine.pending_group_joins(), 0, "the allowance is spent after joining");

    finish(vec![alice, bob, carol, dave, erin]).await;
}
