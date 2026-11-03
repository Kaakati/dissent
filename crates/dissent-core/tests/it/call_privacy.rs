//! "Who can call you" on real engines: a refused call never rings or notifies, leaves a quiet missed entry
//! in its conversation, and looks to the caller exactly like a call nobody picks up.

use std::time::{Duration, Instant};

use dissent_core::{CallPolicy, ConvId, DmPolicy, EngineEvent};
use tokio::sync::broadcast;

use crate::calls::group_id_named;
use crate::common::*;

/// Every incoming-call event `rx` sees within `window`.
async fn rings_within(rx: &mut broadcast::Receiver<EngineEvent>, window: Duration) -> Vec<String> {
    let mut rings = Vec::new();
    let _ = tokio::time::timeout(window, async {
        loop {
            match rx.recv().await {
                Ok(e @ (EngineEvent::IncomingCall { .. } | EngineEvent::CallMissed { .. })) => rings.push(format!("{e:?}")),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
    .await;
    rings
}

/// The missed entries in `items`.
fn notices(items: &[dissent_core::DmItem]) -> Vec<String> {
    items.iter().filter_map(|m| m.notice.clone()).collect()
}

/// How long the ring timeout is in these tests (`fast_timings`).
fn ring_timeout() -> Duration {
    dissent_core::test_support::fast_timings().ring_timeout
}

/// alice still lists bob, but bob removed her: she can call him, and to him she is not a friend. bob's DM
/// policy lets anyone message him, so only the call setting decides.
async fn one_sided(alice: &Peer, bob: &Peer) {
    make_contacts_ready(alice, bob).await;
    bob.engine.remove_contact(alice.identity.root_id()).expect("bob removes alice");
    bob.engine.set_dm_policy(DmPolicy::Everyone).expect("dm policy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_non_friends_dm_call_never_rings_and_leaves_a_missed_entry() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        one_sided(&alice, &bob).await;
        bob.engine.set_call_policy(CallPolicy::Friends).expect("friends only");
        let mut bob_events = bob.engine.subscribe();
        alice.engine.call_dm(bob.identity.root_id()).await.expect("the call goes out, as to anyone");
        let rings = rings_within(&mut bob_events, ring_timeout() + Duration::from_secs(3)).await;
        assert!(rings.is_empty(), "bob is not rung or told of a missed call: {rings:?}");
        assert!(bob.engine.call_state().is_none(), "bob has no call");
        // bob no longer has alice's card, so the entry names her as he can.
        let entries = notices(&bob.engine.dm_messages(alice.identity.root_id(), 50));
        assert!(
            entries.len() == 1 && entries[0].starts_with("Missed call from ") && entries[0].ends_with(": not in your friends"),
            "{entries:?}"
        );
        eventually("alice's call ends unanswered", || alice.engine.call_state().is_none().then_some(())).await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// The settings against a contact and a blocked caller: a contact rings under Friends only, no one rings under No
/// one and that leaves an entry, and a blocked caller never rings and leaves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn each_setting_against_a_contact_and_a_blocked_caller() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;
        let alice_root = alice.identity.root_id();
        let mut bob_events = bob.engine.subscribe();

        // Friends only: a contact rings.
        bob.engine.set_call_policy(CallPolicy::Friends).expect("friends only");
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call");
        let call = next_incoming(&mut bob_events).await;
        bob.engine.decline_call(call).await.expect("bob declines");
        eventually("alice's call ends", || alice.engine.call_state().is_none().then_some(())).await;

        // No one: nobody rings, the contact included, and the entry says so.
        bob.engine.set_call_policy(CallPolicy::NoOne).expect("no one");
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call");
        let rings = rings_within(&mut bob_events, ring_timeout() + Duration::from_secs(3)).await;
        assert!(rings.is_empty(), "no one rings under No one: {rings:?}");
        assert_eq!(
            notices(&bob.engine.dm_messages(alice_root, 50)),
            vec!["Missed call from alice: you are not taking calls".to_string()]
        );
        eventually("alice's call ends", || alice.engine.call_state().is_none().then_some(())).await;

        // Everyone, but alice is blocked: nothing rings and nothing is written.
        bob.engine.set_call_policy(CallPolicy::Everyone).expect("everyone");
        bob.engine.block_user(alice_root).expect("block");
        alice.engine.call_dm(bob.identity.root_id()).await.expect("the call goes out, as to anyone");
        let rings = rings_within(&mut bob_events, ring_timeout() + Duration::from_secs(3)).await;
        assert!(rings.is_empty(), "a blocked caller never rings: {rings:?}");
        assert_eq!(notices(&bob.engine.dm_messages(alice_root, 50)).len(), 1, "no entry for a blocked caller");
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// carol starts a group call. bob, who takes calls from friends only and is no longer carol's friend, is not rung
/// and gets the entry in the group. alice, who takes calls from everyone, rings.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_group_call_rings_only_whom_the_settings_let() {
    heavy(3, async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        make_contacts_ready(&alice, &bob).await;
        make_contacts_ready(&alice, &carol).await;
        // carol knows bob's devices, so her call reaches him. To bob, she is not a friend.
        make_contacts_ready(&bob, &carol).await;
        bob.engine.remove_contact(carol.identity.root_id()).expect("bob removes carol");
        let created = alice
            .engine
            .create_group("trio".into(), vec![bob.identity.root_id(), carol.identity.root_id()])
            .await
            .expect("alice creates the group");
        let conv: ConvId = created.id.parse().expect("id parses");
        for p in [&bob, &carol] {
            eventually("a member has the group", || group_id_named(p, "trio")).await;
        }
        bob.engine.set_call_policy(CallPolicy::Friends).expect("friends only");
        let mut alice_events = alice.engine.subscribe();
        let mut bob_events = bob.engine.subscribe();
        carol.engine.join_group_call(conv).await.expect("carol starts the group call");
        let alice_call = next_incoming(&mut alice_events).await;
        alice.engine.decline_call(alice_call).await.expect("alice declines");
        let rings = rings_within(&mut bob_events, ring_timeout() + Duration::from_secs(3)).await;
        assert!(rings.is_empty(), "bob is not rung by someone who is not his friend: {rings:?}");
        let entries = notices(&bob.engine.conversation_messages(conv, 50));
        assert!(
            entries.len() == 1 && entries[0].starts_with("Missed call from ") && entries[0].ends_with(": not in your friends"),
            "{entries:?}"
        );
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

/// What the caller sees of one call: each call event's kind, in order and with repeats folded, how long until the
/// call ended, and how the call looked while it rang. `call_speaking` is left out: it is the caller's own
/// microphone, and when its first tick lands against `call_updated` depends on this machine's load, not on the
/// callee.
async fn caller_view(caller: &Peer, callee: &Peer) -> (Vec<String>, Duration, String) {
    let mut events = caller.engine.subscribe();
    let started = Instant::now();
    caller.engine.call_dm(callee.identity.root_id()).await.expect("the call goes out");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ringing = caller
        .engine
        .call_state()
        .map(|v| format!("{:?} {:?} participants={} route={:?}", v.scope, v.topology, v.participants.len(), v.route.title))
        .unwrap_or_default();
    eventually("the call ends", || caller.engine.call_state().is_none().then_some(())).await;
    let took = started.elapsed();
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        let kind = serde_json::to_value(&event).ok().and_then(|v| v["kind"].as_str().map(str::to_string)).unwrap_or_default();
        if kind.contains("call") && kind != "call_speaking" && kinds.last() != Some(&kind) {
            kinds.push(kind);
        }
    }
    (kinds, took, ringing)
}

/// alice calls bob, whose setting turns her away, and carol, who takes calls from everyone and does not pick up.
/// alice sees the same: the same call events in the same order, the same look while it rings, and an end at the
/// same time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_refused_call_looks_to_the_caller_like_an_unanswered_one() {
    heavy(3, async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        one_sided(&alice, &bob).await;
        bob.engine.set_call_policy(CallPolicy::Friends).expect("friends only");
        make_contacts_ready(&alice, &carol).await;
        let (refused, refused_took, refused_ringing) = caller_view(&alice, &bob).await;
        let (unanswered, unanswered_took, unanswered_ringing) = caller_view(&alice, &carol).await;
        assert_eq!(refused, unanswered, "the same call events");
        assert_eq!(refused_ringing, unanswered_ringing, "the same look while it rings");
        let gap = refused_took.abs_diff(unanswered_took);
        assert!(gap < Duration::from_millis(1500), "ends {refused_took:?} and {unanswered_took:?} apart by {gap:?}");
        assert!(refused_took >= ring_timeout(), "the refused call rings the full time: {refused_took:?}");
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}
