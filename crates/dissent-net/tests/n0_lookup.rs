//! With number0's DNS directory switched off (`NodeConfig.n0_address_lookup = false`), peers still connect:
//! through a relay, and on the LAN by the direct addresses they already know. Everything runs on loopback,
//! with a relay of its own, so nothing reaches number0. The DHT cannot be exercised on loopback.

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{
    CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig,
};
use noq_proto::rustls::pki_types::CertificateDer;
use url::Url;

const WAIT: Duration = Duration::from_secs(20);

struct TestRelay {
    _server: Server,
    url: Url,
    cert: CertificateDer<'static>,
}

async fn spawn_relay() -> TestRelay {
    let (certs, server_config) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut relay = RelayServerConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual { server_config },
    ));
    relay.key_cache_capacity = Some(1024);
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.expect("spawn relay");
    let addr: SocketAddr = server.https_addr().expect("https addr");
    TestRelay {
        _server: server,
        url: format!("https://{addr}").parse().expect("relay url"),
        cert: certs[0].clone(),
    }
}

/// A node on `relay` only, with number0's directory switched off.
fn config(relay: &TestRelay) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(vec![relay.url.clone()]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![relay.cert.clone()])),
        loopback_only: true,
        n0_address_lookup: false,
        ..Default::default()
    }
}

/// Waits until `node` has `relay` as its home, so peers can reach it through that relay.
async fn wait_home(node: &Node, relay: &Url) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while node.relay_report().home.as_ref() != Some(relay) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the relay did not become home"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    node.online().await;
}

#[ignore = "networked: run with scripts/fwtest.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_is_reached_through_a_relay_with_number0_off() {
    let relay = spawn_relay().await;
    let a = Node::spawn(&DeviceKey::generate(), config(&relay))
        .await
        .expect("a");
    let b = Node::spawn(&DeviceKey::generate(), config(&relay))
        .await
        .expect("b");
    wait_home(&a, &relay.url).await;
    wait_home(&b, &relay.url).await;
    // b learns a from its address record alone. No directory lookup is involved.
    let addr_a = a.addr().await;
    b.remember(&addr_a).expect("remember a");
    b.ping(&addr_a)
        .await
        .expect("the ping reaches a through the relay");
    a.shutdown().await.expect("shut a down");
    b.shutdown().await.expect("shut b down");
}

#[ignore = "networked: run with scripts/fwtest.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_is_reached_on_the_lan_with_number0_off() {
    // No relay at all: the direct loopback addresses stand in for the LAN.
    let cfg = NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        n0_address_lookup: false,
        ..Default::default()
    };
    let a = Node::spawn(&DeviceKey::generate(), cfg.clone())
        .await
        .expect("a");
    let b = Node::spawn(&DeviceKey::generate(), cfg).await.expect("b");
    let addr_a = a.addr().await;
    b.remember(&addr_a).expect("remember a");
    b.ping(&addr_a)
        .await
        .expect("the ping reaches a over the direct addresses");
    a.shutdown().await.expect("shut a down");
    b.shutdown().await.expect("shut b down");
}
