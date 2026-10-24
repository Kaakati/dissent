//! Listen and dial in one process on loopback. Relays and DHT are off, so nothing leaves the
//! machine and no firewall prompt appears.

use dissent_probe::{DialOptions, NetOptions, dial, node_config, start_listener};
use dissent_net::Node;
use dissent_identity::DeviceKey;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn loopback_dial_is_direct_and_delivers_media() {
    let opts = NetOptions {
        loopback: true,
        ..NetOptions::default()
    };
    let listener = start_listener(&opts).await.expect("listener starts");
    let dialer = Node::spawn(
        &DeviceKey::generate(),
        node_config(&opts, None).expect("config"),
    )
    .await
    .expect("dialer starts");

    let dial_opts = DialOptions {
        duration: Duration::from_secs(3),
        ..DialOptions::default()
    };
    let report = dial(&dialer, &listener.addr, &dial_opts, &opts)
        .await
        .expect("dial succeeds");

    assert_eq!(report.connect.path, "direct", "{report:?}");
    assert!(report.upgrade.time_to_direct_ms.is_some(), "no direct sample: {report:?}");
    assert_eq!(report.media.path_end, "direct", "{report:?}");
    assert!(
        report.media.delivered_pct > 95.0,
        "delivered {:.1}%: {report:?}",
        report.media.delivered_pct
    );

    dialer.shutdown().await.expect("dialer shuts down");
    listener.node.shutdown().await.expect("listener shuts down");
}
