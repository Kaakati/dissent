//! Leaving a community stops its gossip topic and deletes its local data.

mod common;

use common::*;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn left_community_gets_no_more_gossip() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general channel");

    alice
        .engine
        .send_message(id, general, "before".into())
        .await
        .expect("alice posts");
    eventually("bob has the first message", || {
        has_message(&bob.engine, id, general, "before").then_some(())
    })
    .await;

    bob.engine
        .leave_community(id)
        .await
        .expect("bob leaves");
    assert!(bob.engine.communities().is_empty());
    let bob_dir = bob.dir.join("communities").join(id.to_string());
    assert!(!bob_dir.exists(), "bob's community data is deleted");

    alice
        .engine
        .send_message(id, general, "after".into())
        .await
        .expect("alice posts again");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        bob.engine.community_view(id).is_err(),
        "bob does not rebuild the community from gossip"
    );
    assert!(!bob_dir.exists(), "gossip did not recreate the community directory");

    finish(vec![alice, bob]).await;
}
