//! Voice calls over the loopback network. Every engine runs on this machine with relays disabled.


use crate::common::*;
use dissent_core::{
    start_device_link, CallScope, ChannelId, CommunityId, DmPolicy, EngineEvent, ModAction, PathView,
    SenderConfig, Topology,
};
use std::time::Duration;

/// Makes `a` and `b` contacts of each other, so a DM call between them is allowed.
/// Creates a community with a voice channel and joins every peer to it. Returns the community,
/// the voice channel, and the peers in the same order.
pub(crate) async fn voice_community(owner: &Peer, others: &[&Peer]) -> (CommunityId, ChannelId) {
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn dm_call_exchanges_authenticated_frames() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;

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
#[ignore = "networked: run with scripts/fwtest.sh"]
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn leaving_and_muting_update_the_call_view() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;
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
#[ignore = "networked: run with scripts/fwtest.sh"]
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn unanswered_call_rings_then_counts_as_missed_with_no_media() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn declined_call_never_connects() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;
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
#[ignore = "networked: run with scripts/fwtest.sh"]
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn banned_member_loses_the_call_key_on_rekey() {
    heavy(3, async {
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
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn voice_pipeline_carries_audio_between_peers() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;
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

    // Pace noise in like a microphone, and pull what bob plays out, until bob hears it. The
    // deadline is generous, so a loaded machine only delays the first audible frame.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut peak = 0.0f32;
    let mut i = 0usize;
    while peak <= 0.05 && std::time::Instant::now() < deadline {
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
        i += 1;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(peak > 0.05, "bob hears alice within 20 s (peak {peak}, {i} frames sent)");
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn ten_engines_in_a_voice_channel_go_forwarded() {
    heavy(10, async {
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

    every_device(&everyone, Duration::from_secs(60), "a forwarded call with nine others", |p| {
        p.engine.call_state().is_some_and(|v| v.participants.len() == 9 && v.topology == Topology::Forwarded)
    })
    .await;

    // The forwarder relays between the leaves, and its own speech goes through its own relay, so
    // every engine, the forwarder included, hears the others.
    eventually_within(Duration::from_secs(60), "most engines hear the call", || {
        let hearing = everyone
            .iter()
            .filter(|p| p.engine.call_state().is_some_and(|v| v.frames_received > 0))
            .count();
        (hearing == 10).then_some(())
    })
    .await;

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn direct_call_rings_every_device_and_the_first_to_accept_wins() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    make_contacts_ready(&alice, &bob).await;

    // Bob links a second device, which joins the same community with his root.
    let pending = start_device_link("Tablet").expect("link request");
    let grant = bob
        .engine
        .approve_device(&pending.link().expect("link"))
        .expect("bob approves the tablet");
    let tablet = start(pending.finish(&grant).expect("tablet identity"), scratch("tablet")).await;
    let invite = alice
        .engine
        .create_invite(community)
        .await
        .expect("invite");
    tablet
        .engine
        .join_community(&invite)
        .await
        .expect("tablet joins");
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut bob_events = bob.engine.subscribe();
    let mut tablet_events = tablet.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call rings");
    let bob_call = next_incoming(&mut bob_events).await;
    let tablet_call = next_incoming(&mut tablet_events).await;
    assert_eq!(bob_call, tablet_call, "both devices ring for the same call");

    bob.engine.accept_call(bob_call).await.expect("bob accepts");
    eventually_within(Duration::from_secs(30), "alice is connected to bob", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 1)
    })
    .await;
    let view = alice.engine.call_state().expect("in call");
    assert_eq!(view.participants[0].root_id, bob.identity.root_id().to_string());

    // The tablet stops ringing, and its call ends.
    let answered = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match tablet_events.recv().await {
                Ok(EngineEvent::CallAnsweredElsewhere { .. }) => break,
                Ok(_) => {}
                Err(e) => panic!("engine events closed: {e}"),
            }
        }
    })
    .await;
    assert!(answered.is_ok(), "the tablet is told the call was answered elsewhere");
    assert!(tablet.engine.call_state().is_none());
    finish(vec![alice, bob, tablet]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn banned_member_of_a_forwarded_call_loses_audio_after_the_rekey() {
    heavy(7, async {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..7 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    every_device(&everyone, Duration::from_secs(60), "a forwarded call with six others", |p| {
        p.engine.call_state().is_some_and(|v| v.participants.len() == 6 && v.topology == Topology::Forwarded && v.frames_received > 0)
    })
    .await;

    // Find the forwarder from any engine that is not it. Ban a member that is neither the
    // forwarder nor the banning owner, so the call keeps its relay.
    let forwarder_root = everyone
        .iter()
        .filter_map(|p| p.engine.call_state())
        .find_map(|v| v.participants.iter().find(|x| x.forwarder).map(|x| x.root_id.clone()))
        .expect("someone names the forwarder");
    let victim = others
        .iter()
        .find(|p| p.identity.root_id().to_string() != forwarder_root)
        .expect("a non-forwarder member");
    let victim_root = victim.identity.root_id();
    let epoch_before = owner.engine.call_state().expect("in call").key_epoch;

    owner
        .engine
        .moderate(community, ModAction::Ban, victim_root)
        .await
        .expect("ban");

    // Everyone still in the call moves to the new key epoch.
    for p in &everyone {
        if p.identity.root_id() == victim_root {
            continue;
        }
        eventually_within(Duration::from_secs(60), "rekey the forwarded call", || {
            p.engine
                .call_state()
                .filter(|v| v.key_epoch > epoch_before)
        })
        .await;
    }

    // The banned member decrypts nothing sent after the rekey.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let settled = victim.engine.call_state().map(|v| v.frames_received).unwrap_or(0);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let later = victim.engine.call_state().map(|v| v.frames_received).unwrap_or(0);
    assert_eq!(settled, later, "the banned member must decrypt no frame after the rekey");

    // The others keep hearing each other.
    let keeper = everyone
        .iter()
        .find(|p| p.identity.root_id() != victim_root && p.identity.root_id() != owner.identity.root_id())
        .expect("a remaining member");
    let before = keeper.engine.call_state().expect("in call").frames_received;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = keeper.engine.call_state().expect("in call").frames_received;
    assert!(after > before, "the remaining members keep talking");

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_late_joiner_is_verified_without_being_flagged() {
    heavy(3, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let (community, channel) = voice_community(&alice, &[&bob]).await;
    for p in [&alice, &bob] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    eventually_within(Duration::from_secs(30), "alice and bob are connected", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;

    // Carol joins the community after the call started, so the call learns her certificate late.
    let invite = alice
        .engine
        .create_invite(community)
        .await
        .expect("invite");
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins the community");
    // Alice only admits members whose join has reached her, so wait for carol's join to arrive.
    let carol_root = carol.identity.root_id();
    eventually_within(Duration::from_secs(60), "alice sees carol as a member", || {
        alice
            .engine
            .community_view(community)
            .ok()
            .filter(|v| v.members.iter().any(|m| m.root_id == carol_root.to_string()))
            .map(|_| ())
    })
    .await;
    carol
        .engine
        .join_call(community, channel)
        .await
        .expect("carol joins the call");

    eventually_within(Duration::from_secs(60), "alice hears carol", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 2)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    for p in [&alice, &bob] {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.quality.flagged, 0, "no sender is flagged as an impersonator");
    }
    finish(vec![alice, bob, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_call_grows_from_three_to_eight_and_back_to_four_without_dropping_audio() {
    heavy(8, async {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..8 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();

    // Three devices: a mesh with audio.
    for p in &everyone[..3] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    for p in &everyone[..3] {
        eventually_within(Duration::from_secs(60), "a three-way mesh with audio", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 2 && v.topology == Topology::Mesh && v.frames_received > 0)
        })
        .await;
    }

    // Grow to eight: the call moves to a forwarded tier once the roster holds for ten seconds.
    for p in &everyone[3..] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    every_device(&everyone, Duration::from_secs(150), "a forwarded call of eight", |p| {
        p.engine.call_state().is_some_and(|v| v.participants.len() == 7 && v.topology == Topology::Forwarded && v.frames_received > 0)
    })
    .await;

    // Shrink to four: the call moves back to a mesh.
    for p in &everyone[4..] {
        p.engine.leave_call().await.expect("leave");
    }
    for p in &everyone[..4] {
        eventually_within(Duration::from_secs(150), "a mesh of four after shrinking", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 3 && v.topology == Topology::Mesh && v.frames_received > 0)
        })
        .await;
    }

    // Audio kept flowing on the remaining members: their counts still grow.
    let before = everyone[0].engine.call_state().expect("in call").frames_received;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = everyone[0].engine.call_state().expect("in call").frames_received;
    assert!(after > before, "the call keeps carrying audio after the migrations");

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_call_above_the_cascade_threshold_relays_through_a_tree() {
    heavy(9, async {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..9 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();
    // A small threshold, so the test stays tractable on loopback.
    for p in &everyone {
        p.engine.set_cascade_at(9);
    }

    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    for p in &everyone {
        eventually_within(Duration::from_secs(240), "a cascade of twelve", || {
            p.engine.call_state().filter(|v| {
                v.participants.len() == 8
                    && v.topology == Topology::Cascade
                    && (v.frames_received > 0 || p.engine.call_role() == Some("standby"))
            })
        })
        .await;
    }

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn losing_the_cascade_root_keeps_everyone_hearing() {
    heavy(9, async {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..9 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();
    for p in &everyone {
        p.engine.set_cascade_at(9);
    }
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    for p in &everyone {
        eventually_within(Duration::from_secs(240), "a cascade of twelve", || {
            p.engine.call_state().filter(|v| {
                v.participants.len() == 8
                    && v.topology == Topology::Cascade
                    && (v.frames_received > 0 || p.engine.call_role() == Some("standby"))
            })
        })
        .await;
    }

    // Find the root and a standby, then drop the root from the call.
    let root = everyone
        .iter()
        .copied()
        .find(|p| p.engine.call_role() == Some("root"))
        .expect("a root");
    assert!(
        everyone.iter().any(|p| p.engine.call_role() == Some("standby")),
        "a standby root exists"
    );
    root.engine.leave_call().await.expect("the root leaves");

    // Everyone still in the call keeps hearing audio after the failover.
    let remaining: Vec<&Peer> = everyone
        .iter()
        .copied()
        .filter(|p| !std::ptr::eq(*p, root))
        .collect();
    let before: Vec<u64> = remaining
        .iter()
        .map(|p| p.engine.call_state().map_or(0, |v| v.frames_received))
        .collect();
    tokio::time::sleep(Duration::from_secs(8)).await;
    for (p, was) in remaining.iter().zip(before) {
        let now = p.engine.call_state().map_or(0, |v| v.frames_received);
        assert!(now > was, "every remaining member keeps hearing the call after the root is lost");
    }

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

/// The root id of the forwarder as a call sees it, if the call is forwarded.
pub(crate) fn forwarder_root(engine: &dissent_core::Engine) -> Option<String> {
    engine.call_state().and_then(|v| {
        v.participants
            .iter()
            .find(|p| p.forwarder)
            .map(|p| p.root_id.clone())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarder_that_slows_down_is_replaced_by_a_clearly_better_device() {
    heavy(7, async {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..7 {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();

    // The owner is slow. Everyone else has more capacity, so the forwarder is one of them.
    owner.engine.set_capacity_override(Some(1_000));
    for p in &others {
        p.engine.set_capacity_override(Some(5_000));
    }
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    for p in &everyone {
        eventually_within(Duration::from_secs(150), "a forwarded call of seven", || {
            p.engine.call_state().filter(|v| {
                v.participants.len() == 6 && v.topology == Topology::Forwarded && v.frames_received > 0
            })
        })
        .await;
    }
    let first = others.iter().find_map(|p| forwarder_root(&p.engine)).expect("a forwarder is named");

    // The forwarder now measures a much lower capacity. A device with clearly more takes over.
    let forwarder = everyone
        .iter()
        .find(|p| p.identity.root_id().to_string() == first)
        .expect("the forwarder is in the call");
    forwarder.engine.set_capacity_override(Some(300));
    eventually_within(Duration::from_secs(240), "the forwarder is replaced", || {
        others.iter().find_map(|p| forwarder_root(&p.engine)).filter(|root| *root != first)
    })
    .await;

    // Audio keeps flowing through the new forwarder.
    let before = owner.engine.call_state().expect("in call").frames_received;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = owner.engine.call_state().expect("in call").frames_received;
    assert!(after > before, "audio keeps flowing after the forwarder changes");

    let mut all = vec![owner];
    all.extend(others);
    finish(all).await;
    }).await;
}

/// The id of the group named `name`, as this engine sees it once it has joined.
fn group_id_named(p: &Peer, name: &str) -> Option<dissent_core::ConvId> {
    p.engine
        .conversations()
        .into_iter()
        .find(|c| c.kind == "group" && c.name == name)
        .and_then(|c| c.id.parse().ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_group_call_rings_every_member_and_late_joiners_join_quietly() {
    heavy(4, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    for other in [&bob, &carol, &dave] {
        make_contacts_ready(&alice, other).await;
    }
    for (x, y) in [(&bob, &carol), (&bob, &dave), (&carol, &dave)] {
        make_contacts_ready(x, y).await;
    }
    let created = alice
        .engine
        .create_group(
            "club".into(),
            vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()],
        )
        .await
        .expect("alice creates the group");
    let conv: dissent_core::ConvId = created.id.parse().expect("id parses");
    for p in [&bob, &carol, &dave] {
        eventually("a member has the group", || group_id_named(p, "club")).await;
    }

    let mut alice_events = alice.engine.subscribe();
    let mut bob_events = bob.engine.subscribe();
    let mut carol_events = carol.engine.subscribe();
    let mut dave_events = dave.engine.subscribe();
    alice
        .engine
        .join_group_call(conv)
        .await
        .expect("alice starts the group call");

    // Everyone else rings. Bob and carol answer. Dave declines.
    let bob_call = next_incoming(&mut bob_events).await;
    let carol_call = next_incoming(&mut carol_events).await;
    let dave_call = next_incoming(&mut dave_events).await;
    bob.engine.accept_call(bob_call).await.expect("bob accepts");
    carol.engine.accept_call(carol_call).await.expect("carol accepts");
    dave.engine.decline_call(dave_call).await.expect("dave declines");
    eventually_within(Duration::from_secs(90), "alice hears bob and carol", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 2 && v.frames_received > 0)
    })
    .await;

    // Dave joins late. Nobody rings for that: the people in the call answer, and he is in.
    dave.engine.join_group_call(conv).await.expect("dave joins late");
    eventually_within(Duration::from_secs(90), "dave is in the call with three others", || {
        dave.engine
            .call_state()
            .filter(|v| v.participants.len() == 3 && v.frames_received > 0)
    })
    .await;
    let quiet = tokio::time::timeout(Duration::from_secs(3), next_incoming(&mut bob_events)).await;
    assert!(quiet.is_err(), "a late join rings nobody");
    let quiet = tokio::time::timeout(Duration::from_secs(1), next_incoming(&mut alice_events)).await;
    assert!(quiet.is_err(), "the caller is not rung for a late join");

    finish(vec![alice, bob, carol, dave]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_outsider_cannot_join_a_group_call() {
    heavy(5, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let outsider = peer("outsider").await;
    for other in [&bob, &carol, &dave] {
        make_contacts_ready(&alice, other).await;
    }
    let created = alice
        .engine
        .create_group("club".into(), vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()])
        .await
        .expect("alice creates the group");
    let conv: dissent_core::ConvId = created.id.parse().expect("id parses");
    eventually("bob has the group", || group_id_named(&bob, "club")).await;

    // The outsider has no copy of the group, so it cannot join its call.
    assert!(
        outsider.engine.join_group_call(conv).await.is_err(),
        "a non-member is refused"
    );
    assert!(outsider.engine.call_state().is_none());
    finish(vec![alice, bob, carol, dave, outsider]).await;
    }).await;
}

/// The live call a DM with `peer` shows on `engine`, if any.
fn live_dm_with(engine: &dissent_core::Engine, peer: dissent_core::RootId) -> Option<dissent_core::LiveCallView> {
    engine
        .conversations()
        .into_iter()
        .find(|c| c.kind == "dm" && c.peer == peer.to_string())
        .and_then(|c| c.live_call)
}

/// The live call a group shows on `engine`, if any.
fn live_group(engine: &dissent_core::Engine, conv: dissent_core::ConvId) -> Option<dissent_core::LiveCallView> {
    engine
        .conversations()
        .into_iter()
        .find(|c| c.id == conv.to_string())
        .and_then(|c| c.live_call)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_group_call_shows_as_live_to_a_member_outside_it_who_joins_and_hears_it() {
    heavy(4, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    for other in [&bob, &carol, &dave] {
        make_contacts_ready(&alice, other).await;
    }
    for (x, y) in [(&bob, &carol), (&bob, &dave), (&carol, &dave)] {
        make_contacts_ready(x, y).await;
    }
    let created = alice
        .engine
        .create_group(
            "club".into(),
            vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()],
        )
        .await
        .expect("alice creates the group");
    let conv: dissent_core::ConvId = created.id.parse().expect("id parses");
    for p in [&bob, &carol, &dave] {
        eventually("a member has the group", || group_id_named(p, "club")).await;
    }

    let mut bob_events = bob.engine.subscribe();
    let mut carol_events = carol.engine.subscribe();
    let mut dave_events = dave.engine.subscribe();
    alice
        .engine
        .join_group_call(conv)
        .await
        .expect("alice starts the group call");
    let bob_call = next_incoming(&mut bob_events).await;
    let carol_call = next_incoming(&mut carol_events).await;
    let dave_call = next_incoming(&mut dave_events).await;
    bob.engine.accept_call(bob_call).await.expect("bob accepts");
    carol.engine.accept_call(carol_call).await.expect("carol accepts");
    dave.engine.decline_call(dave_call).await.expect("dave stays out");

    // Dave is a member outside the call. His list shows it live: three people in it, not him.
    let live = eventually_within(Duration::from_secs(90), "dave sees the live call", || {
        live_group(&dave.engine, conv).filter(|l| l.participants.len() == 3)
    })
    .await;
    assert_eq!(live.total, 4, "everyone in the group, dave included");
    assert!(live.not_in.is_empty(), "every member is in the call");
    assert_eq!(live.route.kind, "direct");
    assert_eq!(live.scope, None, "a group call has no dm scope");
    let in_call: Vec<String> = live.participants.iter().map(|p| p.root_id.clone()).collect();
    for p in [&alice, &bob, &carol] {
        assert!(in_call.contains(&p.identity.root_id().to_string()), "the call lists everyone in it");
    }
    assert!(!in_call.contains(&dave.identity.root_id().to_string()), "dave is not in it yet");

    // Dave joins through the lobby and hears the call.
    dave.engine.join_group_call(conv).await.expect("dave joins the live call");
    eventually_within(Duration::from_secs(90), "dave hears the call", || {
        dave.engine
            .call_state()
            .filter(|v| v.participants.len() == 3 && v.frames_received > 0)
    })
    .await;

    finish(vec![alice, bob, carol, dave]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_dm_call_stays_live_after_one_side_steps_away_and_rejoins() {
    heavy(2, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;

    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("call starts");
    accept_incoming(&bob.engine, &mut bob_events).await;
    eventually_within(Duration::from_secs(90), "alice is in the call with bob", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;
    eventually_within(Duration::from_secs(90), "bob is in the call with alice", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;

    // Alice steps away. Bob stays, so the call stays live for alice, and it is no longer live for bob.
    alice.engine.leave_call().await.expect("alice steps away");
    let live = eventually_within(Duration::from_secs(30), "alice sees the call still live", || {
        live_dm_with(&alice.engine, bob.identity.root_id()).filter(|l| l.participants.len() == 1)
    })
    .await;
    assert_eq!(live.scope, Some("dm"));
    assert_eq!(live.total, 2);
    assert!(live.not_in.is_empty());
    assert_eq!(live.participants[0].root_id, bob.identity.root_id().to_string());
    eventually_within(Duration::from_secs(30), "bob sees alice has left", || {
        live_dm_with(&bob.engine, alice.identity.root_id()).is_none().then_some(())
    })
    .await;

    // Alice rejoins through the live call and hears bob again.
    alice
        .engine
        .call_dm(bob.identity.root_id())
        .await
        .expect("alice rejoins the live call");
    eventually_within(Duration::from_secs(90), "alice hears bob again", || {
        alice
            .engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;
    eventually_within(Duration::from_secs(90), "bob hears alice again", || {
        bob.engine
            .call_state()
            .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;

    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_outsider_sees_no_live_call() {
    heavy(7, async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let dave = peer("dave").await;
    let outsider = peer("outsider").await;
    for other in [&bob, &carol, &dave] {
        make_contacts_ready(&alice, other).await;
    }
    let created = alice
        .engine
        .create_group(
            "club".into(),
            vec![bob.identity.root_id(), carol.identity.root_id(), dave.identity.root_id()],
        )
        .await
        .expect("alice creates the group");
    let conv: dissent_core::ConvId = created.id.parse().expect("id parses");
    eventually("bob has the group", || group_id_named(&bob, "club")).await;

    let mut bob_events = bob.engine.subscribe();
    alice
        .engine
        .join_group_call(conv)
        .await
        .expect("alice starts the group call");
    accept_incoming(&bob.engine, &mut bob_events).await;
    // Bob's list shows the call, so the call's roster has reached a member.
    eventually_within(Duration::from_secs(90), "bob sees the live call", || {
        live_group(&bob.engine, conv).filter(|l| l.participants.len() == 1)
    })
    .await;

    // The outsider has no copy of the group, so no conversation of theirs shows a live call.
    assert!(live_group(&outsider.engine, conv).is_none());
    assert!(outsider.engine.conversations().iter().all(|c| c.live_call.is_none()));
    finish(vec![alice, bob, carol, dave, outsider]).await;
    }).await;
}

/// Puts `count` peers in one voice channel at boost `level`, each announcing `kbps` of upload.
pub(crate) async fn boosted_call(count: usize, level: u8, kbps: u32) -> (Vec<Peer>, CommunityId, ChannelId) {
    let owner = peer("owner").await;
    let mut others = Vec::new();
    for n in 1..count {
        others.push(peer(&format!("member {n}")).await);
    }
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = voice_community(&owner, &refs).await;
    let mut everyone = vec![owner];
    everyone.extend(others);
    for p in &everyone {
        dissent_core::test_support::set_boost_level(&p.engine, Some(level));
        p.engine.set_capacity_override(Some(kbps));
    }
    (everyone, community, channel)
}

/// Waits until every peer in `peers` sees a call of `peers.len()` at `topology`, with audio.
pub(crate) async fn all_see(peers: &[Peer], topology: Topology, what: &str) {
    every_device(peers, Duration::from_secs(150), what, |p| {
        p.engine.call_state().is_some_and(|v| {
            v.participants.len() == peers.len() - 1 && v.topology == topology && v.frames_received > 0
        })
    })
    .await;
}

/// Waits until each device in `peers`, in turn, meets `seen`. A timeout names every device's call, and marks the ones not
/// there yet, so a failure shows which device is stuck and how.
pub(crate) async fn every_device<P: std::borrow::Borrow<Peer>>(peers: &[P], limit: Duration, what: &str, seen: impl Fn(&Peer) -> bool) {
    for p in peers {
        let deadline = std::time::Instant::now() + scaled(limit);
        while !seen(p.borrow()) {
            if std::time::Instant::now() >= deadline {
                let lines: Vec<String> = peers
                    .iter()
                    .map(|p| format!("{} {}", if seen(p.borrow()) { "  " } else { "!!" }, call_summary(p.borrow())))
                    .collect();
                panic!("timed out waiting for {what}. Each device, marked if it is not there yet:\n{}", lines.join("\n"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// One line on where a device's call stands, for a test that timed out.
pub(crate) fn call_summary(p: &Peer) -> String {
    let name = p.engine.me().display_name;
    let Some(v) = p.engine.call_state() else {
        return format!("{name}: not in a call");
    };
    let forwarders = v.participants.iter().filter(|m| m.forwarder).map(|m| m.root_id[..8].to_string()).collect::<Vec<_>>();
    format!(
        "{name} ({}): {:?} participants={} frames={} key_epoch={} forwarded_by={:?} forwarders={forwarders:?} route={:?} / {:?}",
        &p.identity.root_id().to_string()[..8],
        v.topology,
        v.participants.len(),
        v.frames_received,
        v.key_epoch,
        v.forwarded_by,
        v.route.title,
        v.route.detail,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn boosted_mesh_grows_to_the_level_limit_then_forwards() {
    heavy(9, async {
    // Level 2 keeps a mesh up to 8. Every device has the upload for seven copies of audio.
    let (everyone, community, channel) = boosted_call(9, 2, 5_000).await;
    for p in &everyone[..8] {
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone[..8], Topology::Mesh, "a boosted mesh of eight").await;
    // Well past the tier sustain, eight is still a mesh. Level 0 would have forwarded it at 7.
    tokio::time::sleep(Duration::from_secs(8)).await;
    for p in &everyone[..8] {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.topology, Topology::Mesh, "eight stays a mesh at level 2");
        assert_eq!(view.participants.len(), 7);
    }

    // The ninth device takes the call over the limit, so it is forwarded.
    everyone[8].engine.join_call(community, channel).await.expect("join");
    all_see(&everyone, Topology::Forwarded, "a forwarded call of nine").await;
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_released_device_holds_a_boosted_call_to_level_zero_and_still_takes_part() {
    heavy(7, async {
    let (everyone, community, channel) = boosted_call(7, 2, 5_000).await;
    // One device runs a released build. It sends no `Features`, and answers each one with the
    // decode error a released build gives.
    let released = &everyone[6];
    dissent_core::test_support::act_as_released_calls(&released.engine);
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    // Seven is forwarded, as at level 0, and the released device hears the call.
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven with a released device").await;

    // Signalling after the refused `Features` still reaches it and comes back from it.
    released.engine.set_mute(true).await.expect("mute");
    for p in &everyone[..6] {
        eventually("the released device's mute is seen", || {
            p.engine.call_state().filter(|v| {
                v.participants
                    .iter()
                    .any(|m| m.muted && m.root_id == released.identity.root_id().to_string())
            })
        })
        .await;
    }
    everyone[0].engine.set_mute(true).await.expect("mute");
    eventually("the released device sees a new device's mute", || {
        released.engine.call_state().filter(|v| {
            v.participants
                .iter()
                .any(|m| m.muted && m.root_id == everyone[0].identity.root_id().to_string())
        })
    })
    .await;
    let before = released.engine.call_state().expect("in call").frames_received;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = released.engine.call_state().expect("in call").frames_received;
    assert!(after > before, "the released device keeps receiving audio");
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_boosted_call_with_a_device_short_of_upload_forwards_at_seven() {
    heavy(7, async {
    let (everyone, community, channel) = boosted_call(7, 2, 5_000).await;
    // Six copies of audio need 192 kbps. This device has 150.
    everyone[3].engine.set_capacity_override(Some(150));
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn measured_upload_elects_a_forwarder() {
    heavy(7, async {
    // No capacity overrides: every device announces what it measures.
    let (everyone, community, channel) = boosted_call(7, 0, 0).await;
    for p in &everyone {
        p.engine.set_capacity_override(None);
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;
    // Each device's announced upload covers a forwarder's load for seven, so the election over the
    // measured numbers alone picks one.
    for p in &everyone {
        eventually_within(Duration::from_secs(60), "an election over measured capacities", || {
            let (measured, elected) = dissent_core::test_support::measured_election(&p.engine);
            (measured == 7).then_some(elected).flatten()
        })
        .await;
    }
    // The forwarder sends a copy of the top speakers to six listeners, so it measures the most.
    let most = everyone
        .iter()
        .filter_map(|p| dissent_core::test_support::own_call_capacity(&p.engine))
        .max()
        .expect("measured");
    assert!(most >= 7 * 96, "the forwarder's measured upload covers seven listeners: {most} kbps");
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_boosted_mesh_holds_on_measured_upload() {
    heavy(7, async {
    let (everyone, community, channel) = boosted_call(7, 2, 0).await;
    for p in &everyone {
        p.engine.set_capacity_override(None);
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Mesh, "a boosted mesh of seven").await;
    // Wait until every device has heard every other's measured upload, then past the tier sustain.
    for p in &everyone {
        eventually_within(Duration::from_secs(60), "every device's upload is measured", || {
            (dissent_core::test_support::measured_election(&p.engine).0 == 7).then_some(())
        })
        .await;
    }
    tokio::time::sleep(Duration::from_secs(8)).await;
    for p in &everyone {
        let upload = dissent_core::test_support::own_call_capacity(&p.engine).expect("measured");
        assert!(upload >= 6 * 32, "six copies of audio are measured: {upload} kbps");
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.topology, Topology::Mesh, "measured upload carries a mesh of seven at level 2");
    }
    finish(everyone).await;
    }).await;
}

/// A forwarded call of seven with video layers, every device at level 0. The devices at `released`
/// act as released builds: no `Features` and no layer messages, one video stream. They announce
/// less upload, so a device that knows layers is elected to forward. Returns the peers and the
/// forwarder's index.
async fn simulcast_call(released: &[usize]) -> (Vec<Peer>, usize) {
    let (everyone, community, channel) = boosted_call(7, 0, 5_000).await;
    for i in released {
        dissent_core::test_support::act_as_released_calls(&everyone[*i].engine);
        everyone[*i].engine.set_capacity_override(Some(2_000));
    }
    for p in &everyone {
        p.engine.set_test_capture(dissent_core::TestCapture::Pattern);
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;
    let forwarder = everyone.iter().find_map(|p| forwarder_root(&p.engine)).expect("a forwarder");
    let index = everyone.iter().position(|p| root(p) == forwarder).expect("the forwarder is in the call");
    assert!(!released.contains(&index), "a device that knows layers forwards");
    (everyone, index)
}

pub(crate) fn root(p: &Peer) -> String {
    p.identity.root_id().to_string()
}

/// Mutes every device but `senders`. The forwarder sends video only of the top three speakers, so
/// the senders must be them.
pub(crate) async fn only_senders_speak(everyone: &[Peer], senders: &[usize]) {
    for (i, p) in everyone.iter().enumerate() {
        if !senders.contains(&i) {
            p.engine.set_mute(true).await.expect("mute");
        }
    }
}

/// Holds each sender's estimate of its link at a rate that carries every layer. These tests check which layer the
/// forwarder picks; on a loaded runner the machine's own delay reads as congestion and keeps senders off their top
/// layers, which is not what they test.
pub(crate) fn hold_sender_rates(senders: &[&Peer]) {
    for s in senders {
        assert!(dissent_core::test_support::force_link_estimate(&s.engine, Some(8_000)), "the sender is in a call");
    }
}

/// The layer of the last frame `viewer` received of `sender`'s camera.
pub(crate) fn layer_of(viewer: &Peer, sender: &Peer) -> Option<u8> {
    dissent_core::test_support::received_layer(&viewer.engine, &root(sender), false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn simulcast_sends_a_narrow_tile_the_low_layer_and_a_wide_tile_a_high_one() {
    heavy(7, async {
    let (everyone, forwarder) = simulcast_call(&[]).await;
    let others: Vec<usize> = (0..everyone.len()).filter(|i| *i != forwarder).collect();
    let (sender, narrow, wide) = (&everyone[others[0]], &everyone[others[1]], &everyone[others[2]]);
    only_senders_speak(&everyone, &[others[0]]).await;
    hold_sender_rates(&[sender]);
    sender.engine.set_camera(true).expect("camera on");
    narrow.engine.set_video_tile(&root(sender), false, 200);
    wide.engine.set_video_tile(&root(sender), false, 1920);

    eventually_within(Duration::from_secs(60), "the narrow tile gets layer 0 and the wide one a higher layer", || {
        let (n, w) = (layer_of(narrow, sender), layer_of(wide, sender));
        (n == Some(0) && w.is_some_and(|l| l > 0)).then_some(())
    })
    .await;
    let mask = dissent_core::test_support::wanted_layers(&sender.engine, false).expect("the sender was asked for layers");
    assert!(mask & 1 != 0 && mask > 1, "the base layer and a higher one are asked for: {mask:#b}");
    // It holds: each keeps its layer.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(layer_of(narrow, sender), Some(0), "the narrow tile stays on layer 0");
        assert!(layer_of(wide, sender).is_some_and(|l| l > 0), "the wide tile stays on a higher layer");
    }
    sender.engine.set_camera(false).expect("camera off");
    finish(everyone).await;
    }).await;
}

/// A voice channel on a level 0 server, as a mesh: a 1440p screen goes out at its own size, while the
/// camera keeps its boost ceiling.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_level_zero_voice_channel_shares_a_1440p_screen_at_its_own_size() {
    heavy(3, async {
    let (everyone, community, channel) = boosted_call(3, 0, 5_000).await;
    for p in &everyone {
        p.engine.set_test_capture(dissent_core::TestCapture::Document { width: 2560, height: 1440 });
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Mesh, "a mesh of three").await;
    let sender = &everyone[0];
    sender.engine.set_screen_share(true, None).expect("screen share on");
    eventually_within(Duration::from_secs(30), "the share reaches 2560x1440", || {
        dissent_core::test_support::screen_plan(&sender.engine).filter(|plan| (plan.0, plan.1) == (2560, 1440))
    })
    .await;
    eventually_within(Duration::from_secs(30), "and it reaches the others", || {
        everyone[1].engine.call_state().filter(|c| c.participants.iter().any(|p| p.root_id == root(sender) && p.screen))
    })
    .await;
    sender.engine.set_screen_share(false, None).expect("screen share off");
    finish(everyone).await;
    }).await;
}

/// A forwarder passes a screen's sound to everyone, though it holds no key. The source is a test tone.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarder_passes_a_screens_sound_to_every_viewer() {
    heavy(7, async {
    let (everyone, forwarder) = simulcast_call(&[]).await;
    let others: Vec<usize> = (0..everyone.len()).filter(|i| *i != forwarder).collect();
    let (sender, viewer) = (&everyone[others[0]], &everyone[others[1]]);
    sender.engine.set_screen_audio(true).expect("sound on");
    sender.engine.set_screen_share(true, None).expect("screen share on");
    eventually_within(Duration::from_secs(60), "a viewer and the forwarder both get the sound", || {
        let hears = |p: &Peer| p.engine.call_state().is_some_and(|c| c.participants.iter().any(|q| q.root_id == root(sender) && q.screen && q.screen_audio));
        (hears(viewer) && hears(&everyone[forwarder])).then_some(())
    })
    .await;
    let mut most = 0f32;
    for _ in 0..50 {
        let mut out = vec![0f32; 960];
        viewer.engine.pull_playout(&mut out);
        most = most.max(out.iter().map(|s| s * s).sum::<f32>() / 960.0);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(most > 0.01, "the viewer's playout has the tone: {most}");
    sender.engine.set_screen_share(false, None).expect("screen share off");
    finish(everyone).await;
    }).await;
}

/// A forwarded 1440p screen: a wide tile gets the top layer, a narrow one the base, and a viewer whose
/// budget cannot fit the top gets 720p, which it can still read.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarded_1440p_screen_gives_each_viewer_a_layer_it_can_carry() {
    // Seven engines, and the sender encodes 1440p with two layers below it, which costs about what three more
    // engines do on a debug build. The count stretches every wait to match. `heavy` already runs it alone.
    heavy(10, async {
    let (everyone, forwarder) = simulcast_call(&[]).await;
    let others: Vec<usize> = (0..everyone.len()).filter(|i| *i != forwarder).collect();
    let (sender, narrow, wide) = (&everyone[others[0]], &everyone[others[1]], &everyone[others[2]]);
    only_senders_speak(&everyone, &[others[0]]).await;
    dissent_core::test_support::use_link_estimates(&everyone[forwarder].engine, false).await;
    sender.engine.set_test_capture(dissent_core::TestCapture::Document { width: 2560, height: 1440 });
    sender.engine.set_screen_bitrate(6_000);
    sender.engine.set_screen_share(true, None).expect("screen share on");
    // This tests the forwarder's choice of layer, so the sender's rate is held rather than left to the runner's load.
    assert!(dissent_core::test_support::force_link_estimate(&sender.engine, Some(8_000)));
    narrow.engine.set_video_tile(&root(sender), true, 200);
    wide.engine.set_video_tile(&root(sender), true, 2400);

    let received = |viewer: &Peer| dissent_core::test_support::received_layer(&viewer.engine, &root(sender), true);
    eventually_within(Duration::from_secs(90), "the narrow tile gets the base and the wide one a higher layer", || {
        (received(narrow) == Some(0) && received(wide).is_some_and(|l| l > 0)).then_some(())
    })
    .await;
    // A level 0 server: no boost ceiling applies to a screen.
    let plan = dissent_core::test_support::screen_plan(&sender.engine).expect("a share runs");
    assert_eq!((plan.0, plan.1), (2560, 1440), "the top layer is the screen's own size");
    // The layers' rates are what the sender sends now. The sender's rate is held above, and the wait is a backstop: a
    // budget above a top layer that has not climbed would rightly keep the top. The budget goes between the middle and
    // the top.
    let fwd = &everyone[forwarder].engine;
    let mut table = Vec::new();
    let climbed = tokio::time::Instant::now() + scaled(Duration::from_secs(60));
    while tokio::time::Instant::now() < climbed {
        table = dissent_core::test_support::sender_screen_layers(fwd, &root(sender)).await;
        if table.len() == 3 && table[2].1 >= table[1].1 + 400 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(table.len() == 3 && table[2].1 >= table[1].1 + 400, "the screen's top layer climbs clear of the middle: {table:?}");
    let budget = table[1].1 + (table[2].1 - table[1].1) / 2;
    assert!(dissent_core::test_support::set_viewer_budget(fwd, &root(wide), budget).await);
    // A failure says what the viewer last got and what the forwarder and the sender were doing.
    let deadline = tokio::time::Instant::now() + scaled(Duration::from_secs(60));
    while received(wide) != Some(1) {
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "the constrained viewer moves to the middle layer did not happen: got {}; forwarder: {}; sender plan {:?}, layers {:?}",
                dissent_core::test_support::video_received_debug(&wide.engine, &root(sender), true),
                dissent_core::test_support::forwarder_video_debug(&everyone[forwarder].engine, &root(wide), &root(sender), true).await,
                dissent_core::test_support::screen_plan(&sender.engine),
                dissent_core::test_support::wanted_layers(&sender.engine, true),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    sender.engine.set_screen_share(false, None).expect("screen share off");
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn simulcast_keeps_the_speaker_high_for_a_constrained_viewer_and_lowers_the_others() {
    heavy(7, async {
    let (everyone, forwarder) = simulcast_call(&[]).await;
    // Only budgets set by this test limit the viewers, so a loaded runner's link estimates can't.
    assert!(
        dissent_core::test_support::use_link_estimates(&everyone[forwarder].engine, false).await,
        "the forwarder ignores estimates"
    );
    let others: Vec<usize> = (0..everyone.len()).filter(|i| *i != forwarder).collect();
    let senders = &others[..3];
    let (constrained, free) = (&everyone[others[3]], &everyone[others[4]]);
    only_senders_speak(&everyone, senders).await;
    hold_sender_rates(&senders.iter().map(|s| &everyone[*s]).collect::<Vec<_>>());
    for s in senders {
        everyone[*s].engine.set_camera(true).expect("camera on");
        for viewer in [constrained, free] {
            viewer.engine.set_video_tile(&root(&everyone[*s]), false, 1920);
        }
    }
    // Every viewer draws every camera large. With room for all, both get them high.
    eventually_within(Duration::from_secs(60), "both viewers get every camera high", || {
        senders
            .iter()
            .all(|s| [constrained, free].iter().all(|v| layer_of(v, &everyone[*s]).is_some_and(|l| l > 0)))
            .then_some(())
    })
    .await;

    // Only the first sender keeps talking. Equally loud senders let the forwarder's choice of speaker
    // move between them under load, which would move the sharp layer with it.
    only_senders_speak(&everyone, &senders[..1]).await;
    let fwd = &everyone[forwarder].engine;
    let wanted = root(&everyone[senders[0]]);
    let mut speaker = None;
    for _ in 0..200 {
        speaker = dissent_core::test_support::active_speaker(fwd).await.filter(|r| *r == wanted);
        if speaker.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let speaker = speaker.expect("the forwarder hears the one sender still talking");
    // A budget that carries one camera on its top layer and two on the base, with less than one
    // step up to spare. The top rung follows the sender's link, so the budget is sized from what
    // the senders announced.
    let mut tables = Vec::new();
    for s in senders {
        tables.push(dissent_core::test_support::sender_layers(fwd, &root(&everyone[*s])).await);
    }
    assert!(tables.iter().all(|t| t.len() >= 2), "every sender announced layers: {tables:?}");
    let top = tables.iter().map(|t| t[t.len() - 1].1).max().expect("three senders");
    let base = tables.iter().map(|t| t[0].1).max().expect("three senders");
    let step = tables.iter().map(|t| t[1].1 - t[0].1).min().expect("three senders");
    let budget = top + 2 * base + step / 2;
    assert!(
        dissent_core::test_support::set_viewer_budget(fwd, &root(constrained), budget).await,
        "the forwarder takes the budget"
    );
    let speaker_peer = senders.iter().map(|s| &everyone[*s]).find(|p| root(p) == speaker).expect("the speaker sends");
    let quiet: Vec<&Peer> = senders.iter().map(|s| &everyone[*s]).filter(|p| root(p) != speaker).collect();
    // The forwarder's plan, read from the forwarder: the speaker high, the quiet cameras on the base,
    // none paused. This is what it decides, independent of which frames have reached a viewer yet.
    let planned = |viewer: &Peer, sender: &Peer| {
        let (viewer, sender) = (root(viewer), root(sender));
        async move { dissent_core::test_support::planned_layer(fwd, &viewer, &sender, false).await }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    for viewer in [constrained, free] {
        loop {
            let sharp = planned(viewer, speaker_peer).await;
            let mut low = Vec::new();
            for q in &quiet {
                low.push(planned(viewer, q).await);
            }
            if sharp.is_some_and(|l| l > 0) && low.iter().all(|l| *l == Some(0)) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the plan keeps the speaker high and the quiet cameras on the base: speaker {sharp:?}, others {low:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    // Live frames: the quiet cameras keep flowing, so each viewer's last frame of them moves down to
    // the base. Before, a camera whose sender stopped talking was not forwarded at all.
    // A failure here says, per viewer and camera, what the viewer last got and what the forwarder was doing.
    let deadline = tokio::time::Instant::now() + scaled(Duration::from_secs(30));
    for viewer in [constrained, free] {
        loop {
            if layer_of(viewer, speaker_peer).is_some_and(|l| l > 0) && quiet.iter().all(|q| layer_of(viewer, q) == Some(0)) {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let mut report = Vec::new();
                for v in [constrained, free] {
                    for s in std::iter::once(speaker_peer).chain(quiet.iter().copied()) {
                        let role = if root(s) == speaker { "speaker" } else { "quiet" };
                        report.push(format!(
                            "viewer {} {} {}: got {}; forwarder: {}",
                            &root(v)[..8],
                            role,
                            &root(s)[..8],
                            dissent_core::test_support::video_received_debug(&v.engine, &root(s), false),
                            dissent_core::test_support::forwarder_video_debug(fwd, &root(v), &root(s), false).await,
                        ));
                    }
                }
                panic!("the quiet cameras keep arriving, on the base, did not happen:\n{}", report.join("\n"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(
        dissent_core::test_support::active_speaker(fwd).await.as_deref(),
        Some(speaker.as_str()),
        "the speaker did not change"
    );
    // The budget fits every base, so nothing is paused for either viewer.
    for viewer in [constrained, free] {
        let view = viewer.engine.call_state().expect("in the call");
        assert!(
            view.participants.iter().all(|p| !p.video_paused),
            "nothing is paused: {:?}",
            view.participants.iter().map(|p| (&p.display_name, p.video_paused)).collect::<Vec<_>>()
        );
    }
    // A budget that fits only one base layer pauses the quiet cameras for that viewer, and its view
    // says so; the speaker's camera keeps going. Room again brings them back.
    let paused_roots = |viewer: &Peer| -> Vec<String> {
        viewer.engine.call_state().map_or(Vec::new(), |v| v.participants.iter().filter(|p| p.video_paused).map(|p| p.root_id.clone()).collect())
    };
    let mut quiet_roots: Vec<String> = quiet.iter().map(|q| root(q)).collect();
    quiet_roots.sort();
    assert!(dissent_core::test_support::set_viewer_budget(fwd, &root(constrained), base + base / 2).await);
    eventually_within(Duration::from_secs(30), "the quiet cameras show as paused", || {
        let mut paused = paused_roots(constrained);
        paused.sort();
        (paused == quiet_roots).then_some(())
    })
    .await;
    assert!(paused_roots(free).is_empty(), "another viewer's budget pauses nothing here");
    assert!(dissent_core::test_support::set_viewer_budget(fwd, &root(constrained), 100_000).await);
    eventually_within(Duration::from_secs(30), "the paused cameras come back", || {
        (paused_roots(constrained).is_empty() && quiet.iter().all(|q| layer_of(constrained, q) == Some(0))).then_some(())
    })
    .await;
    for s in senders {
        everyone[*s].engine.set_camera(false).expect("camera off");
    }
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn simulcast_carries_a_released_sender_to_everyone_and_gives_a_released_viewer_the_base() {
    heavy(7, async {
    // Devices 5 and 6 run a released build: 5 sends one stream, 6 knows no layers.
    let (everyone, forwarder) = simulcast_call(&[5, 6]).await;
    let (old_sender, old_viewer) = (&everyone[5], &everyone[6]);
    let others: Vec<usize> = (0..5).filter(|i| *i != forwarder).collect();
    let (new_sender, wide) = (&everyone[others[0]], &everyone[others[1]]);
    only_senders_speak(&everyone, &[5, others[0]]).await;
    hold_sender_rates(&[old_sender, new_sender]);
    for s in [old_sender, new_sender] {
        s.engine.set_camera(true).expect("camera on");
        for viewer in [wide, old_viewer] {
            viewer.engine.set_video_tile(&root(s), false, 1920);
        }
    }
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(60));
    loop {
        let layers = [layer_of(wide, new_sender), layer_of(wide, old_sender), layer_of(old_viewer, new_sender), layer_of(old_viewer, old_sender)];
        if layers[0].is_some_and(|l| l > 0) && layers[1] == Some(0) && layers[2] == Some(0) && layers[3] == Some(0) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let lines: Vec<String> = everyone.iter().map(call_summary).collect();
            panic!("timed out waiting for every viewer to get both cameras at its layer. Layers wide/new, wide/old, old/new, old/old: {layers:?}\n{}", lines.join("\n"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        dissent_core::test_support::wanted_layers(&old_sender.engine, false),
        None,
        "a released sender is never asked for layers"
    );
    // A released viewer only ever gets the base layer, however wide it draws the camera.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(layer_of(old_viewer, new_sender), Some(0), "the released viewer stays on the base layer");
        assert_eq!(layer_of(wide, old_sender), Some(0), "the released sender's one stream reaches the wide tile");
    }
    for s in [old_sender, new_sender] {
        s.engine.set_camera(false).expect("camera off");
    }
    finish(everyone).await;
    }).await;
}

/// A device that sends again within one MLS epoch has the same key and the same sender id: after leaving and rejoining,
/// and after a restart as well. So a key id and counter must not repeat. See `dissent_media::e2ee` and the threat model,
/// "Audio and video nonces".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejoin_and_a_restart_in_one_epoch_have_the_same_key_and_sender() {
    bounded(async {
        let alice = peer("rejoin-key").await;
        let server = alice.engine.create_community("Rejoin".into()).await.expect("create");
        let community: CommunityId = server.id.parse().unwrap();
        let info = alice.engine.create_voice_channel(community, "Lounge".into()).await.expect("voice channel");
        let channel: ChannelId = info.id.parse().expect("channel id");
        alice.engine.join_call(community, channel).await.expect("join");
        let first = dissent_core::test_support::call_key_state(&alice.engine).expect("in a call");
        alice.engine.leave_call().await.expect("leave");
        alice.engine.join_call(community, channel).await.expect("rejoin");
        assert_eq!(dissent_core::test_support::call_key_state(&alice.engine), Some(first), "a rejoin");
        alice.engine.leave_call().await.expect("leave again");
        let (identity, dir) = stop(alice).await;
        let alice = start(identity, dir).await;
        alice.engine.join_call(community, channel).await.expect("rejoin after a restart");
        assert_eq!(dissent_core::test_support::call_key_state(&alice.engine), Some(first), "a restart");
        finish(vec![alice]).await;
    })
    .await;
}

/// Someone who leaves a channel call and comes back in one MLS epoch counts from 0 again. Whoever stayed must hear them
/// at once, not only when their count passes where it had got to, here half a minute of talking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn someone_who_rejoins_in_one_epoch_is_heard_by_whoever_stayed() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let (community, channel) = voice_community(&alice, &[&bob]).await;
        alice.engine.join_call(community, channel).await.expect("alice joins");
        bob.engine.join_call(community, channel).await.expect("bob joins");
        eventually("alice hears bob", || alice.engine.call_state().filter(|v| v.frames_received > 0)).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        let epoch = alice.engine.call_state().expect("in call").key_epoch;

        bob.engine.leave_call().await.expect("bob leaves");
        eventually("alice sees bob leave", || alice.engine.call_state().filter(|v| v.participants.is_empty())).await;
        let before = alice.engine.call_state().expect("in call").frames_received;
        bob.engine.join_call(community, channel).await.expect("bob rejoins");
        eventually("alice sees bob back", || alice.engine.call_state().filter(|v| !v.participants.is_empty())).await;

        // Well inside the half minute bob talked before, and unscaled for that reason.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let view = alice.engine.call_state().expect("in call");
            assert_eq!(view.key_epoch, epoch, "the test needs one epoch throughout");
            if view.frames_received > before + 20 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "alice does not hear bob after he came back: {} frames then {}", before, view.frames_received);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// Measures how a sharer's estimate of its link responds to a busy CPU on loopback: a 1440p share, 15 s as is, 20 s
/// with every core busy, then 30 s after. Prints the estimate every second. Loopback has no congestion, so any fall is
/// the machine's own delay read as the network's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked measurement: run with scripts/fwtest.sh and --nocapture"]
async fn measure_a_shares_estimate_under_a_cpu_hog() {
    heavy(8, async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;
        let mut bob_events = bob.engine.subscribe();
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call starts");
        crate::common::accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up", || alice.engine.call_state().filter(|v| v.participants.len() == 1 && v.frames_received > 0)).await;
        alice.engine.set_test_capture(dissent_core::TestCapture::Document { width: 2560, height: 1440 });
        alice.engine.set_screen_bitrate(6_000);
        alice.engine.set_screen_share(true, None).expect("screen share on");
        // (second, phase, estimate, the share's plan as width, height, fps and kbps)
        type Sample = (u64, &'static str, Option<u32>, Option<(u32, u32, u32, u32)>);
        let mut trace: Vec<Sample> = Vec::new();
        let sample = async |second: u64, phase: &'static str| {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let e = dissent_core::test_support::link_estimate(&alice.engine);
            let plan = dissent_core::test_support::screen_plan(&alice.engine);
            println!("EST {second:>3} s {phase:<6} estimate {e:?} kbps, plan {plan:?}");
            (second, phase, e, plan)
        };
        for s in 0..15 {
            trace.push(sample(s, "before").await);
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let hogs: Vec<_> = (0..cores)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 0u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    }
                    x
                })
            })
            .collect();
        for s in 15..35 {
            trace.push(sample(s, "hog").await);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for hog in hogs {
            let _ = hog.join();
        }
        for s in 35..65 {
            trace.push(sample(s, "after").await);
        }
        let of = |phase: &str| trace.iter().filter(|t| t.1 == phase).filter_map(|t| t.2).collect::<Vec<u32>>();
        let (before, hog, after) = (of("before"), of("hog"), of("after"));
        let mean = |v: &[u32]| v.iter().map(|x| u64::from(*x)).sum::<u64>() / v.len().max(1) as u64;
        let before_mean = mean(&before[5.min(before.len())..]);
        let recovered = trace.iter().filter(|t| t.1 == "after").find(|t| t.2.is_some_and(|e| u64::from(e) * 10 >= before_mean * 8)).map(|t| t.0 - 35);
        println!(
            "EST summary on {cores} cores: before {before_mean} kbps, lowest under the hog {:?}, lowest after {:?}, last {:?}, back to 80% after {recovered:?} s",
            hog.iter().min(),
            after.iter().min(),
            after.last()
        );
        alice.engine.set_screen_share(false, None).expect("screen share off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

