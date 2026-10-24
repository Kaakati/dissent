//! Contact requests, declines, the daily limit, DM privacy, and persistence. Loopback only.

mod common;

use common::*;
use dissent_core::DmPolicy;

#[tokio::test(flavor = "multi_thread")]
async fn a_link_sends_a_request_that_the_recipient_accepts() {
    bounded(async {
    let alice = peer("alice").await;
    let carol = peer("carol").await;
    let alice_root = alice.identity.root_id();
    let carol_root = carol.identity.root_id();

    let asked = carol
        .engine
        .request_contact(&alice.engine.contact_link(), "we met at the meetup")
        .await
        .expect("carol asks alice");
    assert!(!asked.is_contact, "a request does not make a contact");

    let requests = eventually("alice sees the request", || {
        let list = alice.engine.contact_requests();
        (!list.is_empty()).then_some(list)
    })
    .await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].user, carol_root.to_string());
    assert_eq!(requests[0].note, "we met at the meetup");
    assert!(!alice.engine.user_profile(carol_root).is_contact);

    alice
        .engine
        .accept_request(carol_root)
        .await
        .expect("alice accepts");
    assert!(alice.engine.contact_requests().is_empty());
    assert!(alice.engine.user_profile(carol_root).is_contact);

    eventually("carol learns she was accepted", || {
        carol
            .engine
            .user_profile(alice_root)
            .is_contact
            .then_some(())
    })
    .await;
    assert!(carol.engine.sent_contact_requests().is_empty());

    finish(vec![alice, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_request_is_silent_and_sticks() {
    bounded(async {
    let alice = peer("alice").await;
    let carol = peer("carol").await;
    let alice_root = alice.identity.root_id();
    let carol_root = carol.identity.root_id();

    carol
        .engine
        .add_contact(&alice.engine.contact_link())
        .await
        .expect("carol asks alice");
    eventually("alice sees the request", || {
        (!alice.engine.contact_requests().is_empty()).then_some(())
    })
    .await;

    alice
        .engine
        .decline_request(carol_root)
        .expect("alice declines");
    assert!(alice.engine.contact_requests().is_empty());

    // Carol sends again. Alice drops it without a reply, so nothing comes back to Carol.
    carol
        .engine
        .add_contact(&alice.engine.contact_link())
        .await
        .expect("carol asks again");
    assert!(
        alice.engine.contact_requests().is_empty(),
        "a declined sender is not listed again"
    );
    assert!(!carol.engine.user_profile(alice_root).is_contact);
    assert!(!alice.engine.user_profile(carol_root).is_contact);

    finish(vec![alice, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_are_rate_limited_per_sender() {
    bounded(async {
    let alice = peer("alice").await;
    let carol = peer("carol").await;
    let link = alice.engine.contact_link();

    // The daily allowance is five. Each call replaces Carol's queued request, and the sixth is
    // refused. A refused request leaves Carol's outbox, so she sees nothing pending.
    for i in 0..5 {
        carol
            .engine
            .request_contact(&link, &format!("try {i}"))
            .await
            .expect("carol asks");
    }
    assert_eq!(carol.engine.sent_contact_requests().len(), 1);
    carol
        .engine
        .request_contact(&link, "one too many")
        .await
        .expect("carol asks once more");
    assert!(
        carol.engine.sent_contact_requests().is_empty(),
        "the sixth request in a day is refused"
    );
    assert_eq!(alice.engine.contact_requests().len(), 1);

    finish(vec![alice, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dm_policy_gates_direct_messages() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    // Alice and Bob share a server. Carol shares none with either.
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    let carol_root = carol.identity.root_id();
    // Carol needs Alice's device before she can send anything to her.
    carol
        .engine
        .add_contact(&alice.engine.contact_link())
        .await
        .expect("carol asks alice");
    assert_eq!(alice.engine.dm_policy(), DmPolicy::SharedServers);

    bob
        .engine
        .send_dm(alice_root, "from a shared server".into())
        .await
        .expect("bob sends");
    eventually("alice reads bob over a shared server", || {
        dm_has(&alice.engine, bob_root, "from a shared server").then_some(())
    })
    .await;

    let from_carol = carol
        .engine
        .send_dm(alice_root, "from a stranger".into())
        .await
        .expect("carol sends");
    assert_eq!(from_carol.delivery, "failed", "a stranger is refused by default");
    assert!(from_carol.reason.is_some(), "the refusal says why");
    assert!(!dm_has(&alice.engine, carol_root, "from a stranger"));

    alice
        .engine
        .set_dm_policy(DmPolicy::ContactsOnly)
        .expect("policy set");
    bob
        .engine
        .send_dm(alice_root, "contacts only".into())
        .await
        .expect("bob sends");
    assert!(
        !dm_has(&alice.engine, bob_root, "contacts only"),
        "under contacts-only a shared server is not enough"
    );

    alice
        .engine
        .accept_request(carol_root)
        .await
        .expect("alice accepts carol");
    carol
        .engine
        .send_dm(alice_root, "now a contact".into())
        .await
        .expect("carol sends");
    eventually("alice reads a contact under contacts-only", || {
        dm_has(&alice.engine, carol_root, "now a contact").then_some(())
    })
    .await;

    alice
        .engine
        .set_dm_policy(DmPolicy::Everyone)
        .expect("policy set");
    bob
        .engine
        .send_dm(alice_root, "open to all".into())
        .await
        .expect("bob sends");
    eventually("alice reads bob under everyone", || {
        dm_has(&alice.engine, bob_root, "open to all").then_some(())
    })
    .await;

    finish(vec![alice, bob, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_requests_survive_a_restart() {
    bounded(async {
    let alice = peer("alice").await;
    let carol = peer("carol").await;
    let carol_root = carol.identity.root_id();

    carol
        .engine
        .add_contact(&alice.engine.contact_link())
        .await
        .expect("carol asks alice");
    eventually("alice sees the request", || {
        (!alice.engine.contact_requests().is_empty()).then_some(())
    })
    .await;

    let (identity, dir) = stop(alice).await;
    let alice = start(identity, dir).await;
    let kept = alice.engine.contact_requests();
    assert_eq!(kept.len(), 1, "the request is still waiting after restart");
    assert_eq!(kept[0].user, carol_root.to_string());
    alice
        .engine
        .accept_request(carol_root)
        .await
        .expect("alice accepts after restart");

    finish(vec![alice, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_message_fails_with_a_reason_and_is_not_retried() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let bob_root = bob.identity.root_id();
    bob.engine
        .set_dm_policy(DmPolicy::ContactsOnly)
        .expect("policy set");

    let item = alice
        .engine
        .send_dm(bob_root, "hello?".into())
        .await
        .expect("alice writes");
    assert_eq!(item.delivery, "failed");
    assert!(item.reason.as_deref().is_some_and(|r| !r.is_empty()));
    assert!(!dm_has(&bob.engine, alice.identity.root_id(), "hello?"));

    // Nothing is retried: the message stays failed and never reaches Bob.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let later = alice.engine.dm_messages(bob_root, 10);
    assert!(later.iter().any(|m| m.text == "hello?" && m.delivery == "failed"));
    assert!(!dm_has(&bob.engine, alice.identity.root_id(), "hello?"));

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_sender_is_dropped_without_a_reply() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    bob.engine.block_user(alice_root).expect("bob blocks alice");

    let item = alice
        .engine
        .send_dm(bob_root, "still there?".into())
        .await
        .expect("alice writes");
    // Blocking is not visible to the sender. The message is not reported as refused.
    assert_ne!(item.delivery, "failed");
    assert!(!dm_has(&bob.engine, alice_root, "still there?"));
    assert!(bob
        .engine
        .list_contacts()
        .blocked
        .iter()
        .any(|e| e.root_id == alice_root.to_string()));

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_are_rate_limited_per_sender() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let bob_root = bob.identity.root_id();
    bob.engine
        .set_dm_policy(DmPolicy::ContactsOnly)
        .expect("policy set");

    // The first refusal is explicit, so Alice sees it as failed.
    let first = alice
        .engine
        .send_dm(bob_root, "first try".into())
        .await
        .expect("alice writes");
    assert_eq!(first.delivery, "failed");

    // Within the window Bob answers with a silent acknowledgement, so the second send does not
    // show a refusal. That keeps repeated sends from revealing Bob's policy or presence.
    let second = alice
        .engine
        .send_dm(bob_root, "second try".into())
        .await
        .expect("alice writes again");
    assert_ne!(second.delivery, "failed", "a repeat refusal is silent");
    assert!(!dm_has(&bob.engine, alice.identity.root_id(), "second try"));

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_who_just_answered_is_online() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();
    alice
        .engine
        .add_contact(&bob.engine.contact_link())
        .await
        .expect("alice asks bob");
    eventually("bob sees the request", || {
        (!bob.engine.contact_requests().is_empty()).then_some(())
    })
    .await;
    bob.engine.accept_request(alice.identity.root_id()).await.expect("bob accepts");
    eventually("bob's device is seen", || {
        alice
            .engine
            .list_contacts()
            .contacts
            .iter()
            .any(|e| e.root_id == bob_root.to_string() && e.online)
            .then_some(())
    })
    .await;
    finish(vec![alice, bob]).await;
}
