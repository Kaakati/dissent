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
