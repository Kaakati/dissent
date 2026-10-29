//! Volunteer forwarders: a member who is not in a voice channel call forwards it. See
//! `docs/design/13-forwarded-calls.md`, phase 3. Every engine runs on loopback with relays off.

use crate::calls::{all_see, boosted_call, forwarder_root, layer_of, only_senders_speak, root};
use crate::common::*;
use dissent_core::test_support::{act_as_released_calls, forward_offers, offer_to_forward, relay_downlinks};
use dissent_core::{ChannelId, CommunityId, ModAction, Topology};
use std::time::{Duration, Instant};

/// Seven members in a voice channel call at level 0, and an eighth, `members[7]`, who is not in it
/// and volunteers to forward calls in the server. The devices at `released` act as released builds.
/// Returns once the call is forwarded and every participant hears it.
async fn volunteer_call(released: &[usize]) -> (Vec<Peer>, Peer, CommunityId, ChannelId) {
    let (mut everyone, community, channel) = boosted_call(8, 0, 5_000).await;
    let volunteer = everyone.pop().expect("eight members");
    // A volunteer offers the election's default upload, which with its bonus beats the participants'.
    volunteer.engine.set_capacity_override(None);
    volunteer.engine.set_forward_calls(community, true).expect("volunteer");
    assert!(volunteer.engine.forwards_calls(community));
    for i in released {
        act_as_released_calls(&everyone[*i].engine);
    }
    for p in &everyone {
        p.engine.set_test_capture(dissent_core::TestCapture::Pattern);
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;
    (everyone, volunteer, community, channel)
}

/// Waits until every participant names `volunteer` as the call's forwarder.
async fn all_forwarded_by(everyone: &[Peer], volunteer: &Peer) {
    let name = volunteer.engine.me().display_name;
    for p in everyone {
        eventually_within(Duration::from_secs(90), "the call is forwarded by the volunteer", || {
            p.engine
                .call_state()
                .filter(|v| v.topology == Topology::Forwarded && v.forwarded_by.as_deref() == Some(name.as_str()))
        })
        .await;
    }
}

/// Asserts every participant's frame count still grows.
async fn all_still_hear(everyone: &[Peer], what: &str) {
    let before: Vec<u64> = everyone.iter().map(|p| p.engine.call_state().map_or(0, |v| v.frames_received)).collect();
    tokio::time::sleep(Duration::from_secs(3)).await;
    for (p, was) in everyone.iter().zip(before) {
        let now = p.engine.call_state().map_or(0, |v| v.frames_received);
        assert!(now > was, "{what}: {} heard nothing more ({was} then {now})", p.engine.me().display_name);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_volunteer_outside_the_call_is_elected_and_carries_its_media() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    let (everyone, volunteer, _, _) = volunteer_call(&[]).await;
    all_forwarded_by(&everyone, &volunteer).await;

    // The volunteer is in no call and is listed by nobody. Every participant is a leaf of it.
    assert!(volunteer.engine.call_state().is_none(), "the volunteer is not in the call");
    for p in &everyone {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.participants.len(), 6, "the volunteer is not listed as a participant");
        assert!(view.participants.iter().all(|m| !m.forwarder), "no participant forwards");
        assert!(view.participants.iter().all(|m| m.root_id != root(&volunteer)));
        assert_eq!(p.engine.call_role(), Some("leaf"));
        assert!(view.route.detail.contains("not in this call"), "the route names the volunteer: {}", view.route.detail);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while relay_downlinks(&volunteer.engine).await != everyone.len() {
        assert!(Instant::now() < deadline, "the volunteer's relay carries every participant");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    all_still_hear(&everyone, "audio through the volunteer").await;

    // Video goes through it too, at the layer each viewer's tile needs.
    let (sender, viewer) = (&everyone[1], &everyone[2]);
    only_senders_speak(&everyone, &[1]).await;
    sender.engine.set_camera(true).expect("camera on");
    viewer.engine.set_video_tile(&root(sender), false, 1920);
    everyone[3].engine.set_video_tile(&root(sender), false, 200);
    eventually_within(Duration::from_secs(60), "video reaches both viewers at their layers", || {
        let (wide, narrow) = (layer_of(viewer, sender), layer_of(&everyone[3], sender));
        (wide.is_some_and(|l| l > 0) && narrow == Some(0)).then_some(())
    })
    .await;
    sender.engine.set_camera(false).expect("camera off");

    let mut all = everyone;
    all.push(volunteer);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_volunteer_who_leaves_hands_the_call_to_a_participant() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    let (everyone, volunteer, _, _) = volunteer_call(&[]).await;
    all_forwarded_by(&everyone, &volunteer).await;

    // The volunteer's app closes. Shutting down withdraws its offer, so the call fails over now.
    let (_, dir) = stop(volunteer).await;
    let _ = std::fs::remove_dir_all(dir);
    let left = Instant::now();
    for p in &everyone {
        eventually_within(Duration::from_secs(60), "a participant forwards the call", || {
            p.engine.call_state().filter(|v| {
                v.topology == Topology::Forwarded && v.forwarded_by.is_none() && v.participants.len() == 6
            })
        })
        .await;
    }
    let forwarder = everyone.iter().find_map(|p| forwarder_root(&p.engine)).expect("a participant is named the forwarder");
    assert!(everyone.iter().any(|p| root(p) == forwarder), "the new forwarder is in the call");
    eprintln!("failover to a participant took {:?}", left.elapsed());
    all_still_hear(&everyone, "audio after the failover").await;
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_banned_or_outside_volunteer_is_refused() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    let (mut everyone, community, channel) = boosted_call(8, 0, 5_000).await;
    let banned = everyone.pop().expect("eight members");
    banned.engine.set_capacity_override(None);
    banned.engine.set_forward_calls(community, true).expect("volunteer");
    everyone[0]
        .engine
        .moderate(community, ModAction::Ban, banned.identity.root_id())
        .await
        .expect("ban");
    for p in &everyone {
        eventually("every member sees the ban", || {
            p.engine
                .community_view(community)
                .ok()
                .filter(|v| v.members.iter().any(|m| m.root_id == root(&banned) && m.banned))
        })
        .await;
    }
    // Someone outside the server, who knows a participant's device as a contact.
    let outsider = peer("outsider").await;
    make_contacts_ready(&outsider, &everyone[1]).await;
    outsider.engine.set_capacity_override(None);

    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;

    let target = everyone[1].identity.device_id();
    let reply = offer_to_forward(&banned.engine, community, channel, target).await;
    // Both hear what anyone hears for a call the device is not in, so neither learns it is in one.
    assert_eq!(reply, "not in this call", "a banned member's offer is refused");
    let reply = offer_to_forward(&outsider.engine, community, channel, target).await;
    assert_eq!(reply, "not in this call", "an outsider's offer is refused");

    // Long past an offer interval and the re-election sustain, a participant still forwards.
    tokio::time::sleep(Duration::from_secs(8)).await;
    for p in &everyone {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.forwarded_by, None, "nobody outside the call forwards it");
        assert_eq!(forward_offers(&p.engine), 0, "no offer was kept");
    }
    assert!(everyone.iter().find_map(|p| forwarder_root(&p.engine)).is_some(), "a participant forwards");

    everyone.push(banned);
    everyone.push(outsider);
    finish(everyone).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_old_client_in_the_call_keeps_volunteers_out() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    // Device 6 runs a released build, which cannot decode an offer or link to a volunteer.
    let (everyone, volunteer, _, _) = volunteer_call(&[6]).await;
    // The new devices hold the volunteer's offer...
    eventually("the new devices hold the volunteer's offer", || {
        everyone[..6].iter().all(|p| forward_offers(&p.engine) == 1).then_some(())
    })
    .await;
    assert_eq!(forward_offers(&everyone[6].engine), 0, "the released device takes no offer");
    // ...but well past the re-election sustain, a participant still forwards, as today.
    tokio::time::sleep(Duration::from_secs(8)).await;
    for p in &everyone {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.topology, Topology::Forwarded);
        assert_eq!(view.forwarded_by, None, "no volunteer forwards a call with a released device in it");
    }
    assert!(everyone.iter().find_map(|p| forwarder_root(&p.engine)).is_some(), "a participant forwards");
    assert_eq!(relay_downlinks(&volunteer.engine).await, 0, "the volunteer relays nothing");
    all_still_hear(&everyone, "audio with the released device").await;

    let mut all = everyone;
    all.push(volunteer);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_volunteer_that_drops_everything_is_excluded_and_the_call_fails_over() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    let (everyone, volunteer, _, _) = volunteer_call(&[]).await;
    all_forwarded_by(&everyone, &volunteer).await;

    // A quiet call is not a stall: with everyone muted, nobody hears anything, and nobody spoke.
    for p in &everyone {
        p.engine.set_mute(true).await.expect("mute");
    }
    tokio::time::sleep(Duration::from_secs(9)).await;
    let name = volunteer.engine.me().display_name;
    for p in &everyone {
        let view = p.engine.call_state().expect("in call");
        assert_eq!(view.forwarded_by.as_deref(), Some(name.as_str()), "a quiet call keeps its volunteer");
    }

    // The volunteer keeps its links and its offer but forwards nothing. The leaves that speak hear
    // nothing, nor do the ones listening, so they report it and the call fails over.
    only_senders_speak(&everyone, &[]).await;
    for p in &everyone[..2] {
        p.engine.set_mute(false).await.expect("unmute");
    }
    dissent_core::test_support::drop_relayed(&volunteer.engine, true).await;
    let stalled = Instant::now();
    for p in &everyone {
        eventually_within(Duration::from_secs(60), "a participant forwards the call", || {
            p.engine.call_state().filter(|v| v.topology == Topology::Forwarded && v.forwarded_by.is_none())
        })
        .await;
    }
    eprintln!("stall to failover took {:?}", stalled.elapsed());
    assert!(everyone.iter().find_map(|p| forwarder_root(&p.engine)).is_some(), "a participant forwards");
    for p in &everyone {
        p.engine.set_mute(false).await.expect("unmute");
    }
    all_still_hear(&everyone, "audio after the stalled volunteer is replaced").await;

    // The volunteer still offers, but stays excluded for the rest of the call.
    assert!(everyone.iter().any(|p| forward_offers(&p.engine) > 0), "the volunteer's offer is still live");
    tokio::time::sleep(Duration::from_secs(8)).await;
    for p in &everyone {
        assert_eq!(p.engine.call_state().expect("in call").forwarded_by, None, "the stalled volunteer is not elected again");
    }

    let mut all = everyone;
    all.push(volunteer);
    finish(all).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_seeder_forwards_calls_only_while_the_owner_allows_it() {
    let _serial = crate::common::heavy_test_lock().await;
    bounded(async {
    let (everyone, community, channel) = boosted_call(7, 0, 5_000).await;
    let invite = everyone[0].engine.create_invite(community).await.expect("invite");
    let seeder = peer("seeder").await;
    seeder.engine.join_as_storage(&invite).await.expect("the seeder joins as storage");
    let seeder_root = root(&seeder);
    for p in &everyone {
        eventually("every member sees the seeder", || {
            p.engine
                .community_view(community)
                .ok()
                .filter(|v| v.members.iter().any(|m| m.root_id == seeder_root))
        })
        .await;
    }
    for p in &everyone {
        p.engine.set_test_capture(dissent_core::TestCapture::Pattern);
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Forwarded, "a forwarded call of seven").await;

    // Off by default: the seeder does not offer, and an offer it sends anyway is refused.
    assert!(!everyone[0].engine.seeder_forwards_calls(community));
    let reply = offer_to_forward(&seeder.engine, community, channel, everyone[1].identity.device_id()).await;
    assert_eq!(reply, "not in this call", "a seeder's offer is refused until the owner allows it");
    tokio::time::sleep(Duration::from_secs(6)).await;
    for p in &everyone {
        assert_eq!(p.engine.call_state().expect("in call").forwarded_by, None, "no seeder forwards yet");
    }

    // Only the owner may allow it.
    assert!(everyone[1].engine.set_seeder_forwards_calls(community, true).await.is_err() || !everyone[1].engine.seeder_forwards_calls(community));
    everyone[0].engine.set_seeder_forwards_calls(community, true).await.expect("the owner allows it");
    for p in everyone.iter().chain(std::iter::once(&seeder)) {
        eventually("the setting reaches every device", || p.engine.seeder_forwards_calls(community).then_some(())).await;
    }
    let name = seeder.engine.me().display_name;
    for p in &everyone {
        eventually_within(Duration::from_secs(90), "the seeder forwards the call", || {
            p.engine.call_state().filter(|v| v.forwarded_by.as_deref() == Some(name.as_str()) && v.participants.len() == 6)
        })
        .await;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while relay_downlinks(&seeder.engine).await != everyone.len() {
        assert!(Instant::now() < deadline, "the seeder's relay carries every participant");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    all_still_hear(&everyone, "audio through the seeder").await;
    // Video goes through it as well, at each viewer's layer.
    only_senders_speak(&everyone, &[1]).await;
    everyone[1].engine.set_camera(true).expect("camera on");
    everyone[2].engine.set_video_tile(&root(&everyone[1]), false, 1920);
    eventually_within(Duration::from_secs(60), "video through the seeder", || {
        layer_of(&everyone[2], &everyone[1]).is_some_and(|l| l > 0).then_some(())
    })
    .await;
    everyone[1].engine.set_camera(false).expect("camera off");
    for p in &everyone {
        p.engine.set_mute(false).await.expect("unmute");
    }

    // The owner turns it off: the call fails over to a participant at once.
    everyone[0].engine.set_seeder_forwards_calls(community, false).await.expect("the owner stops it");
    for p in &everyone {
        eventually_within(Duration::from_secs(60), "a participant forwards again", || {
            p.engine.call_state().filter(|v| v.topology == Topology::Forwarded && v.forwarded_by.is_none())
        })
        .await;
    }
    assert!(everyone.iter().find_map(|p| forwarder_root(&p.engine)).is_some(), "a participant forwards");
    all_still_hear(&everyone, "audio after the seeder stops").await;

    let mut all = everyone;
    all.push(seeder);
    finish(all).await;
    }).await;
}
