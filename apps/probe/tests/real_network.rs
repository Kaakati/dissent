//! Real-network measurement with default settings: number0's relays and the DHT, bound to all
//! interfaces. Both sides run in this process, so it measures this machine's own path to the
//! relays and any direct path between its two endpoints. It does not measure two separate
//! networks. Run it through the firewall-safe wrapper:
//!
//!     scripts/fwtest.sh dissent-probe real_network --ignored --nocapture

use dissent_probe::{DialOptions, NetOptions, dial, node_config, start_listener, summary_table};
use dissent_net::Node;
use dissent_identity::DeviceKey;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn default_network_probe_on_this_machine() {
    let opts = NetOptions::default();
    let listener = start_listener(&opts).await.expect("listener starts");
    let dialer = Node::spawn(
        &DeviceKey::generate(),
        node_config(&opts, None).expect("config"),
    )
    .await
    .expect("dialer starts");

    let dial_opts = DialOptions {
        duration: Duration::from_secs(20),
        ..DialOptions::default()
    };
    let report = dial(&dialer, &listener.addr, &dial_opts, &opts)
        .await
        .expect("dial succeeds");
    eprintln!("{}", summary_table(&report));
    eprintln!("{}", serde_json::to_string(&report).unwrap());

    dialer.shutdown().await.expect("dialer shuts down");
    listener.node.shutdown().await.expect("listener shuts down");
}
