//! Creating a private channel in one call: a failed access step leaves no public channel behind, posts are
//! refused while a channel is being set up, files are refused in a private channel, and a key request wakes
//! on a change of access. See `private_keys` and `docs/design/11-private-channels.md`.

use std::time::Duration;

use dissent_events::ChannelId;

use crate::membership::tests::{eventually, Node};
use crate::private_keys::private_retry_after;
use crate::CommunityId;

fn listed(view: &crate::CommunityView, name: &str) -> bool {
    view.channels.iter().any(|c| c.name == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_channel_is_made_in_one_event_and_is_private_from_the_start() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("OneEvent".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let made = alice
            .engine
            .create_private_channel(id, "vault".into(), vec![], vec![])
            .await
            .expect("the channel is made");
        assert!(made.private, "the channel is private as soon as it exists");
        let view = alice.engine.community_view(id).expect("view");
        assert!(listed(&view, "vault"));
    })
    .await;
}

#[test]
fn the_key_retry_waits_one_two_four_then_up_to_a_minute() {
    let mut wait = Duration::from_secs(1);
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.push(wait.as_secs());
        wait = private_retry_after(wait);
    }
    assert_eq!(seen, vec![1, 2, 4, 8, 16, 32, 60, 60]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_of_access_wakes_a_key_request_that_is_backing_off() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Wake".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let made = alice
            .engine
            .create_private_channel(id, "vault".into(), vec![], vec![])
            .await
            .expect("create");
        let channel: ChannelId = made.id.parse().expect("id");

        // Lose the key, so the request has nothing to find and backs off. Without a key the request is
        // retried at 1, 2, 4 and 8 s. The key comes back at about 5 s, during the 8 s wait.
        alice
            .engine
            .write()
            .communities
            .get_mut(&id)
            .expect("community")
            .private_keys
            .clear();
        let engine = alice.engine.clone();
        let task = tokio::spawn(async move { engine.retry_private_keys(id, channel, 0).await });
        tokio::time::sleep(Duration::from_secs(5)).await;
        alice
            .engine
            .remember_private_keys(id, vec![(channel, 0, [7u8; 32])]);
        // A change of access, which a reader's wait is woken by.
        alice
            .engine
            .set_channel_access(id, channel, true, vec![], vec![])
            .await
            .expect("access change");

        // The wait is cut short, so the loop ends well before the 8 s wait would have run out.
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the key request wakes on the change of access")
            .expect("task");
        eventually("the key is held", || alice.engine.private_key(id, channel, 0).is_some()).await;
    })
    .await;
}
