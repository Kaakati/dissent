//! The server's channel order: an admin reorders the text and voice channels and every member sees the new
//! order. A member cannot reorder. Networked, so it runs through `scripts/fwtest.sh dissent-core it --ignored
//! channel_order::`.

use crate::common::*;
use dissent_core::{ChannelId, CommunityId, Engine};
use std::time::Duration;

/// The channel names `engine` lists, text then voice, in the order the sidebar shows them.
fn names(engine: &Engine, community: CommunityId) -> Option<(Vec<String>, Vec<String>)> {
    let view = engine.community_view(community).ok()?;
    let of = |voice: bool| view.channels.iter().filter(|c| c.voice == voice).map(|c| c.name.clone()).collect();
    Some((of(false), of(true)))
}

/// The id of the channel named `name` in `engine`'s view.
fn id_of(engine: &Engine, community: CommunityId, name: &str) -> ChannelId {
    let view = engine.community_view(community).expect("view");
    view.channels.iter().find(|c| c.name == name).expect("channel").id.parse().expect("channel id")
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_admins_order_reaches_a_member_and_a_member_cannot_reorder() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        for name in ["chat", "random"] {
            alice.engine.create_channel(community, name.into()).await.expect("text channel");
        }
        for name in ["lounge", "stage"] {
            alice.engine.create_voice_channel(community, name.into()).await.expect("voice channel");
        }
        let created = (strings(&["general", "chat", "random"]), strings(&["lounge", "stage"]));
        eventually("bob sees the channels in creation order", || {
            names(&bob.engine, community).filter(|n| *n == created)
        })
        .await;

        let text = ["random", "general", "chat"].map(|n| id_of(&alice.engine, community, n)).to_vec();
        let voice = ["stage", "lounge"].map(|n| id_of(&alice.engine, community, n)).to_vec();
        alice.engine.reorder_channels(community, text, voice).await.expect("the owner reorders");
        let ordered = (strings(&["random", "general", "chat"]), strings(&["stage", "lounge"]));
        assert_eq!(names(&alice.engine, community), Some(ordered.clone()), "applied at once for the author");
        eventually("bob sees the new order", || names(&bob.engine, community).filter(|n| *n == ordered)).await;

        let refused = bob
            .engine
            .reorder_channels(community, vec![id_of(&bob.engine, community, "chat")], Vec::new())
            .await;
        assert!(refused.is_err(), "a member cannot reorder");

        // A channel made after the order goes at the end of its kind.
        alice.engine.create_channel(community, "late".into()).await.expect("text channel");
        let with_late = (strings(&["random", "general", "chat", "late"]), strings(&["stage", "lounge"]));
        eventually("bob sees the new channel at the end", || {
            names(&bob.engine, community).filter(|n| *n == with_late)
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(names(&alice.engine, community), Some(with_late));
        finish(vec![alice, bob]).await;
    })
    .await;
}
