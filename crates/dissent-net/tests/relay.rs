//! Relay behaviour on loopback, using relay servers started in-process with self-signed certificates.

use std::{net::Ipv4Addr, time::Duration};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, PathKind, PeerAddr, RelayChoice};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{
    CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig,
};
use noq_proto::rustls::pki_types::CertificateDer;
use url::Url;

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
    let url: Url = format!("https://{}", server.https_addr().expect("https addr"))
        .parse()
        .expect("relay url");
    TestRelay { _server: server, url, cert: certs[0].clone() }
}

fn node_config(relays: Vec<Url>, trust: Vec<CertificateDer<'static>>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(relays),
        relay_ca: Some(CaTlsConfig::custom_roots(trust)),
        loopback_only: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn relays_can_be_added_at_runtime_and_are_advertised() {
    let relay = spawn_relay().await;
    let a = Node::spawn(&DeviceKey::generate(), node_config(vec![], vec![relay.cert.clone()]))
        .await
        .expect("spawn a");
    assert!(a.relays().is_empty());

    a.add_relays(vec![relay.url.clone()]).await;
    a.add_relays(vec![relay.url.clone()]).await; // duplicate is skipped
    assert_eq!(a.relays(), vec![relay.url.clone()]);

    tokio::time::timeout(Duration::from_secs(20), a.online())
        .await
        .expect("node did not reach the relay within 20 s");
    let addr = a.addr().await;
    assert_eq!(addr.relay.as_ref(), Some(&relay.url), "address should advertise the relay");

    a.shutdown().await.expect("shutdown a");
}

/// Question: when B dials A by a `PeerAddr` whose relay is not in B's own relay map, does iroh
/// still connect through that relay? A is reachable only through relay R1. B's map holds only
/// R2, and B trusts both certificates.
#[tokio::test]
async fn dial_through_relay_missing_from_own_map() {
    let r1 = spawn_relay().await;
    let r2 = spawn_relay().await;
    let a = Node::spawn(&DeviceKey::generate(), node_config(vec![r1.url.clone()], vec![r1.cert.clone()]))
        .await
        .expect("spawn a");
    let b = Node::spawn(
        &DeviceKey::generate(),
        node_config(vec![r2.url.clone()], vec![r1.cert.clone(), r2.cert.clone()]),
    )
    .await
    .expect("spawn b");
    tokio::time::timeout(Duration::from_secs(20), a.online()).await.expect("a online");
    tokio::time::timeout(Duration::from_secs(20), b.online()).await.expect("b online");
    assert!(!b.relays().contains(&r1.url), "R1 must be absent from B's map");

    // Relay-only address: no direct IPs, so the connection can only use R1.
    let via_r1 = PeerAddr { id: a.id(), relay: Some(r1.url.clone()), direct: vec![] };
    let report = tokio::time::timeout(Duration::from_secs(30), b.ping(&via_r1))
        .await
        .expect("ping timed out")
        .expect("ping through a relay outside B's map");
    eprintln!("dial via relay outside own map: rtt {} ms, path {:?}", report.rtt_ms, report.path);
    assert_ne!(report.path, PathKind::Direct, "the path must go through the relay");

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

/// Same setup as above, but B does not trust R1's certificate. The dial must fail, so trust roots
/// gate relays that are not in the dialler's own map.
#[tokio::test]
async fn dial_through_unknown_relay_needs_its_certificate() {
    let r1 = spawn_relay().await;
    let r2 = spawn_relay().await;
    let a = Node::spawn(&DeviceKey::generate(), node_config(vec![r1.url.clone()], vec![r1.cert.clone()]))
        .await
        .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), node_config(vec![r2.url.clone()], vec![r2.cert.clone()]))
        .await
        .expect("spawn b");
    tokio::time::timeout(Duration::from_secs(20), a.online()).await.expect("a online");

    let via_r1 = PeerAddr { id: a.id(), relay: Some(r1.url.clone()), direct: vec![] };
    let result = tokio::time::timeout(Duration::from_secs(15), b.ping(&via_r1)).await;
    assert!(!matches!(result, Ok(Ok(_))), "dial succeeded without trusting R1: {result:?}");

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}
