//! Voice channel presence, seen from outside the call: who is in each voice channel, how joins and
//! leaves reach everyone, deafen, the user limit, and a banned member being refused. Networked, so
//! they run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_core::{ChannelId, ChannelPatch, CommunityId, CommunityView, Engine};

/// A community of `owner` and `others` with one voice channel, which every member can see.
async fn voice_community(owner: &Peer, others: &[&Peer]) -> (CommunityId, ChannelId) {
    let community = rustaceans(owner, others[0]).await;
    for other in &others[1..] {
        let invite = owner.engine.create_invite(community).await.expect("invite");
        join_ready(owner, other, &invite).await;
    }
    let info = owner
        .engine
        .create_voice_channel(community, "Lounge".into())
        .await
        .expect("voice channel");
    let channel: ChannelId = info.id.parse().expect("channel id");
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

/// The people `engine` hears in `channel`, as root ids.
fn heard_in(engine: &Engine, community: CommunityId, channel: ChannelId) -> Option<Vec<String>> {
    let view: CommunityView = engine.community_view(community).ok()?;
    let voice = view.voice_channels.iter().find(|v| v.id == channel.to_string())?;
    Some(voice.participants.iter().map(|p| p.root_id.clone()).collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_member_outside_a_voice_channel_sees_who_is_in_it() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;
        let bob_root = bob.identity.root_id().to_string();

        // Carol is not in the call. Bob joins it, and carol hears him through his presence.
        bob.engine.join_call(community, channel).await.expect("bob joins");
        eventually("carol sees bob in the voice channel", || {
            heard_in(&carol.engine, community, channel).filter(|people| people.contains(&bob_root))
        })
        .await;
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn join_and_leave_update_everyone() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;
        let bob_root = bob.identity.root_id().to_string();

        bob.engine.join_call(community, channel).await.expect("bob joins");
        for (name, engine) in [("alice", &alice.engine), ("carol", &carol.engine)] {
            eventually(&format!("{name} sees bob join"), || {
                heard_in(engine, community, channel).filter(|p| p.contains(&bob_root))
            })
            .await;
        }

        bob.engine.leave_call().await.expect("bob leaves");
        for (name, engine) in [("alice", &alice.engine), ("carol", &carol.engine)] {
            eventually(&format!("{name} sees bob leave"), || {
                heard_in(engine, community, channel).filter(|p| !p.contains(&bob_root))
            })
            .await;
        }
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn deafen_is_reported_to_everyone_in_the_channel() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let (community, channel) = voice_community(&alice, &[&bob]).await;
        let bob_root = bob.identity.root_id().to_string();
        bob.engine.join_call(community, channel).await.expect("bob joins");
        eventually("alice sees bob join", || heard_in(&alice.engine, community, channel).filter(|p| !p.is_empty())).await;

        bob.engine.set_deafened(true).expect("deafen");
        eventually("alice sees bob deafened", || {
            alice
                .engine
                .community_view(community)
                .ok()?
                .voice_channels
                .iter()
                .find(|v| v.id == channel.to_string())?
                .participants
                .iter()
                .find(|p| p.root_id == bob_root)
                .filter(|p| p.deafened)
                .map(|_| ())
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn user_limit_refuses_the_next_join() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;
        alice
            .engine
            .update_channel(
                community,
                channel,
                ChannelPatch {
                    user_limit: Some(1),
                    ..Default::default()
                },
            )
            .await
            .expect("set the limit");
        eventually("carol sees the limit", || {
            carol
                .engine
                .community_view(community)
                .ok()?
                .channels
                .into_iter()
                .find(|c| c.id == channel.to_string())
                .filter(|c| c.user_limit == 1)
        })
        .await;

        bob.engine.join_call(community, channel).await.expect("the first person fits");
        let bob_root = bob.identity.root_id().to_string();
        eventually("carol hears bob in the channel", || {
            heard_in(&carol.engine, community, channel).filter(|p| p.contains(&bob_root))
        })
        .await;
        let refused = carol.engine.join_call(community, channel).await;
        assert!(refused.is_err_and(|e| e.to_string().contains("full")), "the limit holds");
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn banned_member_cannot_join_a_voice_channel() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let (community, channel) = voice_community(&alice, &[&bob]).await;
        let bob_root = bob.identity.root_id();
        alice
            .engine
            .moderate(community, dissent_core::ModAction::Ban, bob_root)
            .await
            .expect("ban bob");
        eventually("bob sees his own ban", || {
            bob.engine
                .community_view(community)
                .ok()?
                .members
                .into_iter()
                .find(|m| m.root_id == bob_root.to_string())
                .filter(|m| m.banned)
                .map(|_| ())
        })
        .await;
        let refused = bob.engine.join_call(community, channel).await;
        assert!(refused.is_err(), "a banned member cannot join");
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// Speaking state of `root` in `engine`'s view of `channel`, if listed.
fn speaking_in(engine: &Engine, community: CommunityId, channel: ChannelId, root: &str) -> Option<bool> {
    let view: CommunityView = engine.community_view(community).ok()?;
    let voice = view.voice_channels.iter().find(|v| v.id == channel.to_string())?;
    voice.participants.iter().find(|p| p.root_id == root).map(|p| p.speaking)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn the_roster_lists_each_person_once_and_shows_who_speaks() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let (community, channel) = voice_community(&alice, &[&bob]).await;
        let (alice_root, bob_root) = (alice.identity.root_id().to_string(), bob.identity.root_id().to_string());
        let mut events = alice.engine.subscribe();
        alice.engine.join_call(community, channel).await.expect("alice joins");
        bob.engine.join_call(community, channel).await.expect("bob joins");

        // Alice's roster lists each of them once, her own person included.
        let people = eventually("alice lists both of them", || {
            heard_in(&alice.engine, community, channel).filter(|p| p.contains(&alice_root) && p.contains(&bob_root))
        })
        .await;
        assert_eq!(people.len(), 2, "each person once: {people:?}");

        // A voice channel of two is not called a direct call, and it names the channel.
        let view = eventually("alice is connected to bob", || {
            alice.engine.call_state().filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;
        assert_eq!(view.route.title, "Voice connected", "{:?}", view.route);
        assert_eq!(view.channel_name.as_deref(), Some("Lounge"));

        // Both send audio, so both speak: in alice's call view, on her own tile, and in her roster.
        eventually("alice speaks and hears bob speak", || {
            alice
                .engine
                .call_state()
                .filter(|v| v.self_speaking && v.participants.iter().any(|p| p.root_id == bob_root && p.speaking))
        })
        .await;
        eventually("the roster shows both speaking", || {
            (speaking_in(&alice.engine, community, channel, &alice_root) == Some(true)
                && speaking_in(&alice.engine, community, channel, &bob_root) == Some(true))
            .then_some(())
        })
        .await;

        // Bob mutes: he sends nothing, so alice stops seeing him speak, after the hold.
        bob.engine.set_mute(true).await.expect("bob mutes");
        eventually("bob no longer speaks for alice", || {
            alice
                .engine
                .call_state()
                .filter(|v| v.participants.iter().any(|p| p.root_id == bob_root && !p.speaking))
        })
        .await;
        alice.engine.set_mute(true).await.expect("alice mutes");
        eventually("alice no longer speaks", || alice.engine.call_state().filter(|v| !v.self_speaking)).await;

        // The window heard of each change through `CallSpeaking`, within a check or two.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let mut speaking_events = 0;
        loop {
            match events.try_recv() {
                Ok(dissent_core::EngineEvent::CallSpeaking) => speaking_events += 1,
                Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        assert!(speaking_events >= 2, "speaking changes are announced: {speaking_events}");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_seeder_hears_no_call_it_cannot_forward() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let (community, channel) = voice_community(&alice, &[&bob, &carol]).await;
        let invite = alice.engine.create_invite(community).await.expect("invite");
        let seeder = peer("seeder").await;
        seeder.engine.join_as_storage(&invite).await.expect("the seeder joins as storage");
        let seeder_root = seeder.identity.root_id().to_string();
        eventually("bob sees the seeder", || {
            bob.engine
                .community_view(community)
                .ok()
                .filter(|v| v.members.iter().any(|m| m.root_id == seeder_root))
        })
        .await;
        let bob_root = bob.identity.root_id().to_string();
        let seeder_hears_nobody = |what: &str| {
            let heard = heard_in(&seeder.engine, community, channel).unwrap_or_default();
            assert!(heard.is_empty(), "{what}: the seeder heard {heard:?}");
        };

        // Bob joins. Carol, a member, hears him; the seeder, with forwarding off, hears nothing.
        bob.engine.join_call(community, channel).await.expect("bob joins");
        eventually("carol sees bob in the voice channel", || {
            heard_in(&carol.engine, community, channel).filter(|people| people.contains(&bob_root))
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        seeder_hears_nobody("forwarding off");

        // With forwarding allowed, a call of two still needs no forwarder, so the seeder still hears nothing.
        alice.engine.set_seeder_forwards_calls(community, true).await.expect("the owner allows it");
        eventually("the setting reaches the seeder", || seeder.engine.seeder_forwards_calls(community).then_some(())).await;
        alice.engine.join_call(community, channel).await.expect("alice joins");
        eventually("carol sees alice join", || {
            heard_in(&carol.engine, community, channel).filter(|people| people.len() == 2)
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        seeder_hears_nobody("a call too small to forward");
        finish(vec![alice, bob, carol, seeder]).await;
    })
    .await;
}
