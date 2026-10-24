//! Network status for the status bar. With relays disabled there is no DHT and no relay, but the
//! gossip neighbour count still follows the topic.

mod common;

use common::*;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn net_status_counts_gossip_neighbours_without_relays() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;

    let idle = alice.engine.net_status().await;
    assert!(idle.dht.is_none(), "no DHT when relays are disabled");
    assert!(idle.relay.is_none(), "no home relay when relays are disabled");
    assert_eq!(idle.peers, 0, "no neighbours before any community");

    rustaceans(&alice, &bob).await;

    for engine in [&alice.engine, &bob.engine] {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = engine.net_status().await;
            if status.peers == 1 {
                assert!(status.dht.is_none());
                break;
            }
            assert!(Instant::now() < deadline, "peer count never reached one: {status:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    finish(vec![alice, bob]).await;
    }).await;
}
