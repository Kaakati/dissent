//! "Hide my IP address": engines that reach every peer through a relay, with no IP transport and no
//! DHT, as the desktop runs them with the setting on (`NodeConfig::relay_only`). Joining a server,
//! its messages and direct messages still work, and no peer learns another's direct IP. A call over
//! the relay is covered by `relay_calls`.

use crate::common::*;
use crate::relay_calls::{contacts_once_online, spawn_relay, start_on_relay, wait_online};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn relay_only_engines_join_message_and_dm_without_seeing_an_ip() {
    bounded(async {
        let relay = spawn_relay().await;
        let alice = start_on_relay(&relay, "alice-hidden").await;
        let bob = start_on_relay(&relay, "bob-hidden").await;
        wait_online(&[&alice, &bob]).await;

        // A server: bob joins with a full invite, which carries the relay and no IP.
        let community = rustaceans(&alice, &bob).await;
        let channel = channel_id(&alice.engine, community, "general").expect("general");
        let before = relay.forwarded();
        alice.engine.send_message(community, channel, "through the relay".into()).await.expect("send");
        eventually_within(Duration::from_secs(60), "bob reads alice's message", || {
            bob.engine
                .channel_messages(community, channel, 50)
                .iter()
                .any(|m| m.text == "through the relay")
                .then_some(())
        })
        .await;

        // A direct message, between contacts.
        contacts_once_online(&alice, &bob).await;
        alice.engine.send_dm(bob.identity.root_id(), "a private note".into()).await.expect("dm");
        eventually_within(Duration::from_secs(60), "bob reads the direct message", || {
            dm_has(&bob.engine, alice.identity.root_id(), "a private note").then_some(())
        })
        .await;
        assert!(relay.forwarded() > before, "the traffic went through the relay");

        // Neither has learned a direct IP address for the other: only the relay.
        let (alice_dev, bob_dev) = (alice.identity.device_id(), bob.identity.device_id());
        assert_eq!(dissent_core::test_support::direct_addrs_known(&bob.engine, alice_dev), 0, "bob holds no IP of alice's");
        assert_eq!(dissent_core::test_support::direct_addrs_known(&alice.engine, bob_dev), 0, "alice holds no IP of bob's");
        finish(vec![alice, bob]).await;
    })
    .await;
}
