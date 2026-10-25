//! Channel settings, deletion, relays and invites. Who may change a channel, what slowmode does to
//! a member's posts, what an expired or revoked invite does to a join, and what the invite list
//! shows. Networked, so they run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_core::{
    parse_invite_id, ChannelId, ChannelPatch, ChannelVisibility, ChannelInfo, CommunityId, Engine,
};
use std::time::Duration;

/// Makes a text channel in `community` and returns its id.
async fn text_channel(owner: &Peer, community: CommunityId, name: &str) -> ChannelId {
    let info = owner
        .engine
        .create_channel(community, name.into())
        .await
        .expect("create the channel");
    info.id.parse().expect("channel id")
}

/// The channel as `engine` sees it now.
fn channel_of(engine: &Engine, community: CommunityId, channel: ChannelId) -> Option<ChannelInfo> {
    engine
        .community_view(community)
        .ok()?
        .channels
        .into_iter()
        .find(|c| c.id == channel.to_string())
}

/// A community that only `alice` is in so far, for invite tests that need their own list.
async fn fresh_community(alice: &Peer, name: &str) -> CommunityId {
    alice
        .engine
        .create_community(name.into())
        .await
        .expect("create community")
        .id
        .parse()
        .expect("community id")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn owner_changes_a_channel_and_a_member_cannot() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let chat = text_channel(&alice, community, "chat").await;
        eventually("bob sees the channel", || channel_of(&bob.engine, community, chat)).await;

        let patch = ChannelPatch {
            name: Some("talk-here".into()),
            slowmode_s: Some(30),
            visibility: Some(ChannelVisibility::Spoiler),
            ..Default::default()
        };
        alice
            .engine
            .update_channel(community, chat, patch)
            .await
            .expect("the owner changes the channel");
        eventually("the change reaches bob", || {
            channel_of(&bob.engine, community, chat)
                .filter(|c| c.name == "talk-here" && c.slowmode_s == 30 && c.visibility == "spoiler")
        })
        .await;

        let refused = bob
            .engine
            .update_channel(
                community,
                chat,
                ChannelPatch {
                    name: Some("hijacked".into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(refused.is_err(), "a member cannot change a channel");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(channel_of(&alice.engine, community, chat).unwrap().name, "talk-here");
        assert_eq!(channel_of(&bob.engine, community, chat).unwrap().name, "talk-here");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn slowmode_limits_a_member_but_not_the_owner() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let chat = text_channel(&alice, community, "chat").await;
        eventually("bob sees the channel", || channel_of(&bob.engine, community, chat)).await;
        alice
            .engine
            .update_channel(
                community,
                chat,
                ChannelPatch {
                    slowmode_s: Some(5),
                    ..Default::default()
                },
            )
            .await
            .expect("slowmode");
        eventually("bob sees slowmode", || {
            channel_of(&bob.engine, community, chat).filter(|c| c.slowmode_s == 5)
        })
        .await;

        bob.engine.send_message(community, chat, "one".into()).await.expect("send");
        // Replay would refuse the second post, so the engine refuses it before it is published.
        let second = bob.engine.send_message(community, chat, "two".into()).await;
        assert!(second.is_err_and(|e| e.to_string().contains("Slowmode")), "the second post waits out slowmode");
        wait_seen(&alice, community, chat, "one").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(!has_message(&alice.engine, community, chat, "two"), "no second post reaches alice");

        alice.engine.send_message(community, chat, "owner-a".into()).await.expect("send");
        alice.engine.send_message(community, chat, "owner-b".into()).await.expect("send");
        wait_seen(&bob, community, chat, "owner-b").await;

        // Once the slowmode has passed since bob's last accepted post, he may post again.
        tokio::time::sleep(Duration::from_secs(5)).await;
        bob.engine.send_message(community, chat, "three".into()).await.expect("send");
        wait_seen(&alice, community, chat, "three").await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn deleting_a_channel_removes_it_for_everyone() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let doomed = text_channel(&alice, community, "doomed").await;
        eventually("bob sees the channel", || channel_of(&bob.engine, community, doomed)).await;

        assert!(
            bob.engine.delete_channel(community, doomed).await.is_err(),
            "a member cannot delete a channel"
        );
        alice
            .engine
            .delete_channel(community, doomed)
            .await
            .expect("the owner deletes it");
        eventually("bob no longer sees the channel", || {
            channel_of(&bob.engine, community, doomed).is_none().then_some(())
        })
        .await;
        assert!(channel_of(&alice.engine, community, doomed).is_none());
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn revoked_invite_refuses_new_joins() {
    bounded(async {
        let alice = peer("alice").await;
        let carol = peer("carol").await;
        let community = fresh_community(&alice, "Revocable").await;
        let link = alice.engine.create_invite(community).await.expect("invite");
        let id = alice.engine.list_invites(community).expect("list")[0].id.clone();
        alice
            .engine
            .revoke_invite(community, parse_invite_id(&id).expect("id"))
            .await
            .expect("owner revokes");
        eventually("the revocation is in alice's list", || {
            alice
                .engine
                .list_invites(community)
                .ok()?
                .into_iter()
                .find(|i| i.id == id)
                .filter(|i| i.revoked)
        })
        .await;

        let err = carol
            .engine
            .join_community(&link)
            .await
            .expect_err("a revoked invite does not join");
        assert!(err.to_string().contains("revoked"), "{err}");
        finish(vec![alice, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn expired_invite_refuses_joins() {
    bounded(async {
        let alice = peer("alice").await;
        let carol = peer("carol").await;
        let community = fresh_community(&alice, "Expiring").await;
        let expires = now() + 1_500;
        let link = alice
            .engine
            .create_invite_until(community, Some(expires))
            .await
            .expect("invite");
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        let err = carol
            .engine
            .join_community(&link)
            .await
            .expect_err("an expired invite does not join");
        assert!(err.to_string().contains("expired"), "{err}");
        finish(vec![alice, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn invite_list_counts_uses_and_shows_revocations() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = fresh_community(&alice, "Counted").await;
        let used = alice.engine.create_invite(community).await.expect("invite");
        let unused = alice.engine.create_invite(community).await.expect("invite");
        assert_eq!(alice.engine.list_invites(community).expect("list").len(), 2);

        join_ready(&alice, &bob, &used).await;
        let used_id = {
            let list = alice.engine.list_invites(community).expect("list");
            list.iter().find(|i| i.link == used).expect("the used invite").id.clone()
        };
        eventually("the join counts as a use", || {
            alice
                .engine
                .list_invites(community)
                .ok()?
                .into_iter()
                .find(|i| i.id == used_id)
                .filter(|i| i.uses == 1)
        })
        .await;

        let unused_id = alice
            .engine
            .list_invites(community)
            .expect("list")
            .into_iter()
            .find(|i| i.link == unused)
            .expect("the unused invite")
            .id;
        alice
            .engine
            .revoke_invite(community, parse_invite_id(&unused_id).expect("id"))
            .await
            .expect("revoke");
        eventually("the revoked invite shows as revoked", || {
            alice
                .engine
                .list_invites(community)
                .ok()?
                .into_iter()
                .find(|i| i.id == unused_id)
                .filter(|i| i.revoked)
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn relays_reach_the_community_view() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let relay = "https://relay.example.org".to_string();
        alice
            .engine
            .set_community_relays(community, vec![relay.clone()])
            .await
            .expect("set relays");
        eventually("bob's view names the relay", || {
            bob.engine
                .community_view(community)
                .ok()
                .filter(|v| v.relays == vec![relay.clone()])
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}
