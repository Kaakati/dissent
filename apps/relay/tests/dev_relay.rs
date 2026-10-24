//! Two clients reach each other through a relay started in `--dev` mode. Loopback only, so no
//! firewall prompts and no DHT.

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, PeerAddr, RelayChoice};
use std::time::{Duration, Instant};
use url::Url;

fn client_config(relay: &Url) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(vec![relay.clone()]),
        loopback_only: true,
        dht_lookup: false,
        ..NodeConfig::default()
    }
}

/// Waits until the node has a home relay, then returns its address.
async fn addr_with_relay(node: &Node) -> PeerAddr {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let addr = node.addr().await;
        if addr.relay.is_some() {
            return addr;
        }
        assert!(Instant::now() < deadline, "node never reached the relay");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_connect_through_dev_relay() {
    let relay = dissent_relay::Server::spawn(dissent_relay::dev_config(
        "127.0.0.1:0".parse().unwrap(),
    ))
    .await
    .expect("relay starts");
    let http = relay.http_addr().expect("relay bound an HTTP port");
    let relay_url: Url = format!("http://{http}").parse().unwrap();

    let a = Node::spawn(&DeviceKey::generate(), client_config(&relay_url))
        .await
        .expect("client a");
    let b = Node::spawn(&DeviceKey::generate(), client_config(&relay_url))
        .await
        .expect("client b");
    let addr_a = addr_with_relay(&a).await;
    addr_with_relay(&b).await;

    // Dial a through the relay only, dropping its loopback addresses. The ping must succeed.
    let relay_only = PeerAddr {
        id: addr_a.id,
        relay: addr_a.relay.clone(),
        direct: Vec::new(),
    };
    let report = b.ping(&relay_only).await.expect("ping through the relay");
    eprintln!("ping through dev relay: {report:?}");

    // The relay accepted both clients.
    let metrics = &relay.metrics().server;
    assert!(
        metrics.unique_client_keys.get() >= 2,
        "relay saw {} unique clients, expected 2",
        metrics.unique_client_keys.get()
    );
    assert!(metrics.accepts.get() >= 2, "relay accepted {} connections", metrics.accepts.get());

    a.shutdown().await.expect("a shuts down");
    b.shutdown().await.expect("b shuts down");
    relay.shutdown().await.expect("relay shuts down");
}
