//! Voice calls over the loopback network. Every engine runs on this machine with relays disabled.

mod common;

use common::*;
use dissent_core::{CallScope, ChannelId, CommunityId, DmPolicy, EngineEvent, ModAction, PathView, SenderConfig, Topology};
use std::time::Duration;

/// Makes `a` and `b` contacts of each other, so a DM call between them is allowed.
async fn make_contacts(a: &Peer, b: &Peer) {
    let a_link = a.engine.contact_link();
    let b_link = b.engine.contact_link();
    a.engine.add_contact(&b_link).await.expect("a adds b");
    b.engine.add_contact(&a_link).await.expect("b adds a");
}

/// Creates a community with a voice channel and joins every peer to it. Returns the community,
/// the voice channel, and the peers in the same order.
async fn voice_community(owner: &Peer, others: &[&Peer]) -> (CommunityId, ChannelId) {
    let community = rustaceans(owner, others[0]).await;
    for other in &others[1..] {
        let invite = owner
            .engine
            .create_invite(community)
            .await
            .expect("invite");
        other
            .engine
            .join_community(&invite)
            .await
            .expect("join community");
    }
    let info = owner
        .engine
        .create_voice_channel(community, "Lounge".into())
        .await
        .expect("voice channel");
    let channel: ChannelId = info.id.parse().expect("channel id");
    // Everyone must see the channel as voice before they try to join it.
    for other in others {
        eventually("the voice channel reaches every member", || {
            other
                .engine
                .community_view(community)
                .ok()
                .filter(|v| v.channels.iter().any(|c| c.id == info.id && c.voice))
        })
        .await;
    }
    (community, channel)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dm_call_exchanges_authenticated_frames() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts(&alice, &bob).await;

    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call starts");
    accept_incoming(&bob.engine, &mut bob_events).await;

    let caller = eventually("alice sees the call", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;
    let callee = eventually("bob sees the call", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;

    assert_eq!(caller.scope, CallScope::Dm);
    assert_eq!(caller.topology, Topology::P2p);
    assert_eq!(callee.participants[0].root_id, alice.identity.root_id().to_string());
    assert_eq!(caller.participants[0].root_id, bob.identity.root_id().to_string());
    assert_eq!(caller.participants[0].path, PathView::Direct);
    assert!(!caller.muted);
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_engines_in_a_voice_channel_form_a_mesh() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let (community, channel) = voice_community(&alice, &[&bob, &carol, &dave]).await;

    for p in [&alice, &bob, &carol, &dave] {
        p.engine
            .join_call(community, channel)
            .await
            .expect("join the voice channel");
    }

    for p in [&alice, &bob, &carol, &dave] {
        let view = eventually_within(Duration::from_secs(20), "a mesh of three", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 3 && v.frames_received > 0)
        })
        .await;
        assert_eq!(view.scope, CallScope::Channel);
        assert_eq!(view.topology, Topology::Mesh);
    }
    finish(vec![alice, bob, carol, dave]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_and_muting_update_the_call_view() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts(&alice, &bob).await;
    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call starts");
    accept_incoming(&bob.engine, &mut bob_events).await;
    eventually("the call is up", || {
        bob.engine.call_state().filter(|v| !v.participants.is_empty())
    })
    .await;

    eventually("alice is connected", || {
        alice.engine.call_state().filter(|v| !v.participants.is_empty())
    })
    .await;
    alice.engine.set_mute(true).await.expect("mute");
    assert!(alice.engine.call_state().expect("in call").muted);
    eventually("bob sees alice muted", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.iter().any(|p| p.muted))
    })
    .await;

    alice.engine.leave_call().await.expect("leave");
    assert!(alice.engine.call_state().is_none());
    eventually("bob sees alice leave", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.is_empty())
    })
    .await;
    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn banned_member_is_removed_from_the_call_and_cannot_rejoin() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;

    for p in [&alice, &bob, &carol] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    eventually_within(Duration::from_secs(20), "a mesh before the ban", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.len() == 2)
    })
    .await;

    alice
        .engine
        .moderate(community, ModAction::Ban, carol.identity.root_id())
        .await
        .expect("ban");

    eventually_within(Duration::from_secs(20), "carol leaves bob's call", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.len() == 1)
    })
    .await;

    // Carol leaves, then tries to rejoin. She may not have the ban yet, so retry until her own
    // engine refuses the join.
    carol.engine.leave_call().await.expect("carol leaves");
    let mut refused = false;
    for _ in 0..100 {
        if carol.engine.join_call(community, channel).await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(refused, "a banned member must not be able to join the call");
    assert_eq!(
        bob.engine.call_state().map(|v| v.participants.len()),
        Some(1),
        "the banned member must stay out of the call"
    );
    finish(vec![alice, bob, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unanswered_call_rings_then_counts_as_missed_with_no_media() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts(&alice, &bob).await;
    alice.engine.set_ring_timeout(Duration::from_secs(2));
    bob.engine.set_ring_timeout(Duration::from_secs(2));
    let mut bob_events = bob.engine.subscribe();

    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call rings");
    let call_id = next_incoming(&mut bob_events).await;

    // While it rings, bob has no call and no media link exists.
    assert!(bob.engine.call_state().is_none(), "nothing starts before accepting");
    assert!(
        alice
            .engine
            .call_state()
            .is_none_or(|v| v.participants.is_empty()),
        "nobody is connected while it rings"
    );

    let missed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match bob_events.recv().await {
                Ok(EngineEvent::CallMissed { call_id, .. }) => break call_id,
                Ok(_) => {}
                Err(e) => panic!("engine events closed: {e}"),
            }
        }
    })
    .await
    .expect("the missed call is reported");
    assert_eq!(missed, call_id);
    assert!(bob.engine.call_state().is_none());
    eventually("alice's unanswered call ends", || {
        alice.engine.call_state().is_none().then_some(())
    })
    .await;
    assert!(
        bob.engine.accept_call(call_id).await.is_err(),
        "a missed call cannot be accepted"
    );
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declined_call_never_connects() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts(&alice, &bob).await;
    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call rings");
    let call_id = next_incoming(&mut bob_events).await;

    bob.engine.decline_call(call_id.clone()).await.expect("decline");
    eventually("alice's call ends", || alice.engine.call_state().is_none().then_some(())).await;
    assert!(bob.engine.call_state().is_none());
    assert!(bob.engine.accept_call(call_id).await.is_err());
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_outside_the_dm_policy_is_refused() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    // Alice has bob as a contact, but bob does not have alice, and bob only takes contacts.
    alice
        .engine
        .add_contact(&bob.engine.contact_link())
        .await
        .expect("alice adds bob");
    bob.engine
        .set_dm_policy(DmPolicy::ContactsOnly)
        .expect("policy set");
    let mut bob_events = bob.engine.subscribe();

    let result = alice.engine.call_dm(bob.identity.root_id()).await;
    assert!(result.is_err(), "the call is refused");
    assert!(alice.engine.call_state().is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(3), next_incoming(&mut bob_events))
            .await
            .is_err(),
        "bob's phone must not ring"
    );
    assert!(bob.engine.call_state().is_none());
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn banned_member_loses_the_call_key_on_rekey() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;
    for p in [&alice, &bob, &carol] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    for p in [&alice, &bob, &carol] {
        eventually_within(Duration::from_secs(20), "a three-way call with audio", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 2 && v.frames_received > 0)
        })
        .await;
    }
    let epoch_before = alice.engine.call_state().expect("in call").key_epoch;

    alice
        .engine
        .moderate(community, ModAction::Ban, carol.identity.root_id())
        .await
        .expect("ban");

    // Both remaining members rekey to the new epoch and drop carol's link.
    for p in [&alice, &bob] {
        eventually_within(Duration::from_secs(30), "rekey and drop the banned member", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 1 && v.key_epoch > epoch_before)
        })
        .await;
    }

    // The banned engine decrypts nothing sent after the rekey.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let settled = carol.engine.call_state().expect("carol is still in her call").frames_received;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = carol.engine.call_state().expect("carol is still in her call").frames_received;
    assert_eq!(settled, later, "the banned member must decrypt no frame after the rekey");

    // The others keep exchanging frames.
    let bob_before = bob.engine.call_state().expect("in call").frames_received;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let bob_after = bob.engine.call_state().expect("in call").frames_received;
    assert!(bob_after > bob_before, "the remaining members keep talking");
    finish(vec![alice, bob, carol]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_pipeline_carries_audio_between_peers() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts(&alice, &bob).await;
    // Noise suppression would remove a test signal, so the test uses the plain level detector.
    let plain = SenderConfig {
        noise_suppression: false,
        agc: false,
        ..SenderConfig::default()
    };
    alice.engine.set_voice_sender_config(plain);
    alice.engine.set_voice_audio(true);
    bob.engine.set_voice_audio(true);
    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call rings");
    accept_incoming(&bob.engine, &mut bob_events).await;
    eventually_within(Duration::from_secs(20), "the audio link is up", || {
        bob.engine
            .call_state()
            .filter(|v| !v.participants.is_empty())
    })
    .await;

    // Four seconds of a 440 Hz tone, paced like a microphone, and what bob plays out.
    let mut peak = 0.0f32;
    for i in 0..200 {
        let frame: Vec<f32> = (0..960)
            .map(|n| {
                // Pseudo-random noise, so the voice detector sees speech-like energy.
                let x = ((i * 960 + n) as u32).wrapping_mul(2_654_435_761);
                0.3 * ((x >> 8) as f32 / (1u32 << 23) as f32 - 1.0)
            })
            .collect();
        alice.engine.push_capture(&frame).expect("captured");
        let mut out = vec![0.0f32; 960];
        bob.engine.pull_playout(&mut out);
        peak = out.iter().fold(peak, |m, x| m.max(x.abs()));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(peak > 0.05, "bob hears alice (peak {peak})");
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ten_engines_in_a_voice_channel_go_forwarded() {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..10 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;

    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();
    for p in &everyone {
        p.engine
            .join_call(community, channel)
            .await
            .expect("join the voice channel");
    }

    for p in &everyone {
        eventually_within(Duration::from_secs(60), "a forwarded call with nine others", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 9 && v.topology == Topology::Forwarded)
        })
        .await;
    }

    // The forwarder relays, so every leaf hears the others. At most one engine, the forwarder,
    // hears nothing, because it has no leaf link.
    eventually_within(Duration::from_secs(60), "most engines hear the call", || {
        let hearing = everyone
            .iter()
            .filter(|p| p.engine.call_state().is_some_and(|v| v.frames_received > 0))
            .count();
        (hearing >= 9).then_some(())
    })
    .await;

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
}
