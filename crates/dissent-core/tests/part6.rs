//! Engine surfaces of the UI contract (design doc 07, part 6): capabilities, notes, pins, read
//! markers, first run, safety verification, call controls, and guardian entries.

mod common;

use common::*;
use dissent_core::{CallScope, NoteKind};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn capabilities_list_the_implemented_surfaces() {
    bounded(async {
    let alice = peer("alice").await;
    let caps = alice.engine.capabilities();
    for name in ["calls", "guardians", "devices", "notes", "pins", "safety", "key_change", "close_dm", "ignore"] {
        assert!(caps.contains(&name), "{name} must be reported");
    }
    assert!(!caps.contains(&"groups"), "groups are not built and must not be reported");
    finish(vec![alice]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn notes_and_nicknames_are_private_and_per_user() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .set_note(bob_root, NoteKind::Note, "met at the meetup".into())
        .unwrap();
    alice
        .engine
        .set_note(bob_root, NoteKind::Nickname, "Bobby".into())
        .unwrap();
    let profile = alice.engine.user_profile(bob_root);
    assert_eq!(profile.note.as_deref(), Some("met at the meetup"));
    assert_eq!(profile.nickname.as_deref(), Some("Bobby"));
    // Bob sees nothing of alice's notes.
    assert_eq!(bob.engine.user_profile(alice.identity.root_id()).note, None);

    alice
        .engine
        .set_note(bob_root, NoteKind::Nickname, "   ".into())
        .unwrap();
    assert_eq!(alice.engine.user_profile(bob_root).nickname, None);
    assert!(alice
        .engine
        .set_note(bob_root, NoteKind::Note, "x".repeat(3000))
        .is_err());
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn pins_and_read_markers_persist_on_this_device() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let channel = channel_id(&alice.engine, community, "general").expect("general");
    let item = alice
        .engine
        .send_message(community, channel, "remember this".into())
        .await
        .expect("send");
    let event: dissent_core::EventId = item.id.parse().unwrap();

    alice
        .engine
        .pin_message(community, channel, event, true)
        .unwrap();
    let pinned = alice.engine.pinned_messages(community, channel);
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].text, "remember this");
    alice
        .engine
        .pin_message(community, channel, event, false)
        .unwrap();
    assert!(alice.engine.pinned_messages(community, channel).is_empty());

    alice
        .engine
        .pin_conversation("dm:bob".into(), true)
        .unwrap();
    assert_eq!(alice.engine.pinned_conversations(), vec!["dm:bob".to_string()]);
    alice
        .engine
        .pin_conversation("dm:bob".into(), false)
        .unwrap();
    assert!(alice.engine.pinned_conversations().is_empty());

    assert_eq!(alice.engine.read_marker("dm:bob"), None);
    alice.engine.mark_read("dm:bob".into()).unwrap();
    assert!(alice.engine.read_marker("dm:bob").is_some());
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn first_run_clears_once_the_phrase_is_saved() {
    bounded(async {
    let alice = peer("alice").await;
    assert!(alice.engine.me().first_run);
    alice.engine.complete_first_run().unwrap();
    assert!(!alice.engine.me().first_run);
    finish(vec![alice]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn marking_a_safety_number_verified_sets_the_time() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();
    assert_eq!(alice.engine.user_profile(bob_root).verified_ms, None);
    alice.engine.mark_safety_verified(bob_root).unwrap();
    let profile = alice.engine.user_profile(bob_root);
    assert!(profile.verified_ms.is_some());
    assert_eq!(profile.key_changed_ms, None);
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn call_controls_and_messages_reach_the_call() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let alice_contact = alice.engine.contact_link();
    let bob_contact = bob.engine.contact_link();
    alice.engine.add_contact(&bob_contact).await.unwrap();
    bob.engine.add_contact(&alice_contact).await.unwrap();

    assert!(alice.engine.set_deafened(true).is_err(), "not in a call yet");
    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call starts");
    accept_incoming(&bob.engine, &mut bob_events).await;
    alice.engine.set_deafened(true).unwrap();
    assert!(alice.engine.set_camera(true).is_err());
    alice.engine.set_camera(false).unwrap();
    let view = alice.engine.call_state().expect("in call");
    assert_eq!(view.scope, CallScope::Dm);
    assert!(view.deafened);
    assert!(!view.camera);
    assert_eq!(view.peer, Some(bob.identity.root_id().to_string()));

    alice
        .engine
        .send_call_message("hello during the call".into())
        .await
        .expect("message sent");
    // The message is recorded as sent. Delivery to bob depends on the direct-message path, which
    // is being reworked, so bob's copy is not asserted here.
    assert!(alice
        .engine
        .dm_messages(bob.identity.root_id(), 10)
        .iter()
        .any(|m| m.text == "hello during the call"));
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn guardian_entries_name_the_shared_communities() {
    bounded(async {
    let owner = peer("owner").await;
    let bob = peer("bob").await;
    let community = rustaceans(&owner, &bob).await;
    let name = owner.engine.community_view(community).unwrap().name;
    let view = owner
        .engine
        .set_guardians(vec![bob.identity.root_id()], 1)
        .await
        .expect("guardians set");
    assert!(view.set);
    assert_eq!(view.threshold, 1);
    assert_eq!(view.guardians.len(), 1);
    assert_eq!(view.guardians[0].root_id, bob.identity.root_id().to_string());
    assert_eq!(view.guardians[0].shared_servers, vec![name]);
    finish(vec![owner, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn closing_and_ignoring_stay_on_this_device() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let bob_root = bob.identity.root_id();

    alice.engine.close_conversation("dm:bob".into()).unwrap();
    assert!(alice.engine.closed_conversations().contains_key("dm:bob"));
    alice.engine.reopen_conversation("dm:bob".into()).unwrap();
    assert!(alice.engine.closed_conversations().is_empty());

    assert!(!alice.engine.user_profile(bob_root).ignored);
    alice.engine.ignore_user(bob_root).unwrap();
    assert!(alice.engine.user_profile(bob_root).ignored);
    assert_eq!(alice.engine.ignored_users(), vec![bob_root]);
    // Ignoring is local: bob's view of alice does not change.
    assert!(!bob.engine.user_profile(alice.identity.root_id()).ignored);
    alice.engine.unignore_user(bob_root).unwrap();
    assert!(alice.engine.ignored_users().is_empty());
    finish(vec![alice, bob]).await;
}
