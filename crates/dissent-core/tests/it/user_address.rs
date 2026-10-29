//! Adding a contact by address, and the "Let people add me by address" switch.
//!
//! The first test runs on this machine: without the DHT, an address lookup says so. The rest use the
//! real network and the public Mainline DHT, so they are ignored and named `internet_*`, which the
//! release workflow skips (`--skip internet`) because the public network makes them flaky. Run them
//! through the firewall-approved script:
//!   scripts/fwtest.sh dissent-core it --ignored user_address::internet_ --nocapture

use crate::common::*;
use dissent_core::{Engine, Identity, NodeConfig};
use dissent_discovery::Discovery;
use dissent_identity::RootId;
use std::time::{Duration, Instant};

/// Each wait for the network gets this long.
const LIMIT: Duration = Duration::from_secs(120);

async fn start_real(name: &str) -> Peer {
    let identity = Identity::create(now());
    let dir = scratch(name);
    let (engine, events) = Engine::start(identity.clone(), dir.clone(), NodeConfig::default(), dissent_core::test_support::vault_for(&dir))
        .await
        .expect("engine starts");
    engine.set_display_name(name.to_string()).await.expect("name set");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

/// Polls the DHT until the user record of `root` is published (`want` true) or withdrawn (`want` false).
async fn wait_for_record(discovery: &Discovery, root: RootId, want: bool) {
    let deadline = Instant::now() + LIMIT;
    loop {
        let published = discovery
            .resolve_user(&root)
            .await
            .ok()
            .flatten()
            .is_some_and(|r| !r.is_empty());
        if published == want {
            return;
        }
        assert!(Instant::now() < deadline, "the user record never became published={want}");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_by_address_without_the_dht_says_so() {
    bounded(async {
        let alice = peer("alice-addr-local").await;
        let bob = peer("bob-addr-local").await;
        let err = bob
            .engine
            .add_contact_by_address(&alice.identity.root_id().address(), "")
            .await
            .expect_err("no DHT, so no lookup");
        assert!(err.to_string().contains("needs the internet"), "{err}");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn internet_a_person_is_added_by_their_address_over_the_dht() {
    bounded(async {
        let alice = start_real("alice-addr-net").await;
        let bob = start_real("bob-addr-net").await;
        let alice_root = alice.identity.root_id();
        let discovery = Discovery::new().expect("dht client");

        // Switching on publishes the user record now rather than at the next maintenance pass.
        alice.engine.set_address_discovery(true).expect("switch on");
        wait_for_record(&discovery, alice_root, true).await;

        bob.engine
            .add_contact_by_address(&alice_root.address(), "hi from bob")
            .await
            .expect("bob adds alice by address");

        let bob_root = bob.identity.root_id().to_string();
        let deadline = Instant::now() + LIMIT;
        while !alice.engine.contact_requests().iter().any(|r| r.user == bob_root) {
            assert!(Instant::now() < deadline, "alice never saw bob's request");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn internet_switching_address_discovery_off_withdraws_the_address_and_links_still_work() {
    bounded(async {
        let alice = start_real("alice-off-net").await;
        let bob = start_real("bob-off-net").await;
        let alice_root = alice.identity.root_id();
        let discovery = Discovery::new().expect("dht client");

        alice.engine.set_address_discovery(true).expect("switch on");
        wait_for_record(&discovery, alice_root, true).await;

        alice.engine.set_address_discovery(false).expect("switch off");
        wait_for_record(&discovery, alice_root, false).await;

        let err = bob
            .engine
            .add_contact_by_address(&alice_root.address(), "")
            .await
            .expect_err("a withdrawn address is not found");
        assert!(err.to_string().contains("don't allow adding by address"), "{err}");

        // The long contact link does not depend on the record, so it still adds.
        let link = alice.engine.contact_link();
        assert!(!link.contains("dsu1"), "with discovery off the link is the long form: {link}");
        bob.engine.add_contact(&link).await.expect("the long link still adds");
        finish(vec![alice, bob]).await;
    })
    .await;
}
