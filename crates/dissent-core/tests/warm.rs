//! Warm connections on loopback: warming a community pings its other members, and a second warm
//! right after is skipped.

mod common;

use common::*;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn warm_reaches_the_other_members_and_is_rate_limited() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;

    // Bob's address may take a moment to reach alice. Warming with nobody to ping does not count.
    let deadline = Instant::now() + Duration::from_secs(20);
    let answered = loop {
        let n = alice.engine.warm(id).await.expect("warm");
        if n > 0 {
            break n;
        }
        assert!(Instant::now() < deadline, "nobody answered a warm-up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(answered, 1, "one other member, and it answered");

    // The same community is not pinged again straight away.
    assert_eq!(alice.engine.warm(id).await.expect("warm again"), 0);

    finish(vec![alice, bob]).await;
    }).await;
}
