//! Typing indicators as others see them: channels, DMs and groups. Loopback only.


use crate::common::*;
use dissent_core::{ChannelId, CommunityId, ConvId, RootId, Status, TypingTarget};
use std::time::Duration;

/// Keeps signalling `typing` until `observer` sees exactly `expect` typing in `watch`. Signals
/// go out at most every 3 s, so this is cheap to call in a loop.
async fn typing_until_seen(
    typist: &Peer,
    target: TypingTarget,
    observer: &Peer,
    watch: TypingTarget,
    expect: &[&str],
) {
    eventually("the observer sees the typer", || {
        let _ = typist.engine.typing(target);
        (observer.engine.typing_in(watch) == expect).then_some(())
    })
    .await;
}

fn channel(id: CommunityId, channel: ChannelId) -> TypingTarget {
    TypingTarget::Channel {
        community: id,
        channel,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_channel_member_sees_the_typer() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");

    typing_until_seen(&alice, channel(id, general), &bob, channel(id, general), &["alice"]).await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_non_member_hears_nothing() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");

    typing_until_seen(&alice, channel(id, general), &bob, channel(id, general), &["alice"]).await;

    // Carol shares no community with Alice, so the topic never reaches her.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(carol.engine.typing_in(channel(id, general)).is_empty());

    finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_typer_expires_six_seconds_after_the_last_signal() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");
    typing_until_seen(&alice, channel(id, general), &bob, channel(id, general), &["alice"]).await;

    // Alice stops signalling, so Bob's view of her runs out after the expiry.
    eventually_within(Duration::from_secs(12), "bob sees alice stop typing", || {
        bob.engine.typing_in(channel(id, general)).is_empty().then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_message_clears_the_typer() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");
    typing_until_seen(&alice, channel(id, general), &bob, channel(id, general), &["alice"]).await;

    alice
        .engine
        .send_message(id, general, "done typing".into())
        .await
        .expect("alice posts");
    eventually_within(Duration::from_secs(5), "the message clears alice", || {
        bob.engine.typing_in(channel(id, general)).is_empty().then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_stop_clears_the_typer_at_once() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");
    typing_until_seen(&alice, channel(id, general), &bob, channel(id, general), &["alice"]).await;

    alice
        .engine
        .stop_typing(channel(id, general))
        .expect("alice stops");
    eventually_within(Duration::from_secs(2), "the stop clears alice", || {
        bob.engine.typing_in(channel(id, general)).is_empty().then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn invisible_sends_nothing() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");
    alice
        .engine
        .set_status(Status::Invisible, None)
        .expect("alice goes invisible");

    for _ in 0..3 {
        alice.engine.typing(channel(id, general)).expect("typing is accepted");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(bob.engine.typing_in(channel(id, general)).is_empty());

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_contact_sees_the_typer_in_a_dm() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let alice_root: RootId = alice.identity.root_id();
    let bob_root: RootId = bob.identity.root_id();
    make_contacts_ready(&alice, &bob).await;

    typing_until_seen(
        &alice,
        TypingTarget::Dm(bob_root),
        &bob,
        TypingTarget::Dm(alice_root),
        &["alice"],
    )
    .await;

    finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn group_members_see_the_typer_and_the_message_clears_it() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    make_contacts_ready(&alice, &bob).await;
    make_contacts_ready(&alice, &carol).await;
    make_contacts_ready(&alice, &dave).await;
    let created = alice
        .engine
        .create_group(
            "club".into(),
            vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()],
        )
        .await
        .expect("alice creates the group");
    let conv: ConvId = created.id.parse().expect("id parses");
    eventually("bob has the group", || {
        bob.engine.conversations().iter().any(|c| c.id == created.id).then_some(())
    })
    .await;

    typing_until_seen(
        &alice,
        TypingTarget::Group(conv),
        &bob,
        TypingTarget::Group(conv),
        &["alice"],
    )
    .await;
    typing_until_seen(
        &alice,
        TypingTarget::Group(conv),
        &carol,
        TypingTarget::Group(conv),
        &["alice"],
    )
    .await;

    alice
        .engine
        .send_to_conversation(conv, "hi club".into())
        .await
        .expect("alice posts");
    eventually_within(Duration::from_secs(5), "the message clears alice for bob", || {
        bob.engine.typing_in(TypingTarget::Group(conv)).is_empty().then_some(())
    })
    .await;

    finish(vec![alice, bob, carol, dave]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn private_channel_typing_reaches_its_readers_only() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let id = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &carol, &invite).await;

    let vault = alice
        .engine
        .create_private_channel(id, "vault".into(), vec![], vec![bob.identity.root_id()])
        .await
        .expect("alice makes vault private for bob")
        .id;
    let vault: ChannelId = vault.parse().expect("channel id");
    eventually("bob lists the private channel", || channel_id(&bob.engine, id, "vault").map(|_| ())).await;
    let general = channel_id(&alice.engine, id, "general").expect("general");

    // Bob reads vault, so Alice's typing reaches him, sent direct to his devices.
    typing_until_seen(&alice, channel(id, vault), &bob, channel(id, vault), &["alice"]).await;
    // Carol is a member but not a reader. Public typing still reaches her on the topic...
    typing_until_seen(&alice, channel(id, general), &carol, channel(id, general), &["alice"]).await;
    // ...but vault's never does.
    for _ in 0..4 {
        let _ = alice.engine.typing(channel(id, vault));
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    assert!(carol.engine.typing_in(channel(id, vault)).is_empty(), "a non-reader hears nothing from a private channel");
    // And a non-reader cannot type into it.
    assert!(carol.engine.typing(channel(id, vault)).is_err(), "a non-reader cannot signal typing in a private channel");

    finish(vec![alice, bob, carol]).await;
    })
    .await;
}
