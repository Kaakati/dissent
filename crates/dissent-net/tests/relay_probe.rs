//! `Node::ping_via_relay`: a ping that may only travel through one relay. It completes when the relay
//! admits this node's key and carries its packets to a peer on that relay, and fails when the relay
//! refuses the key or cannot be reached. Everything runs on loopback, with a relay of its own.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use dissent_identity::{DeviceId, DeviceKey};
use dissent_net::{Node, NodeConfig, RelayChoice};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{
    Access, AccessControl, CertConfig, ClientRequest, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig,
};
use noq_proto::rustls::pki_types::CertificateDer;
use url::Url;

const WAIT: Duration = Duration::from_secs(20);

/// Admits only the listed keys, as a hosted "relay for my servers" admits only members.
#[derive(Debug)]
struct Members(Vec<[u8; 32]>);

impl AccessControl for Members {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(request.endpoint_id().as_bytes()) {
            Access::Allow
        } else {
            Access::Deny { reason: Some("not a member".into()) }
        }
    }
}

struct TestRelay {
    _server: Server,
    url: Url,
    cert: CertificateDer<'static>,
}

async fn spawn_relay(members: Vec<[u8; 32]>) -> TestRelay {
    let (certs, server_config) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut relay = RelayServerConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new((Ipv4Addr::LOCALHOST, 0), CertConfig::Manual { server_config }));
    relay.key_cache_capacity = Some(1024);
    relay.access = Arc::new(Members(members));
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

fn config(relay: &TestRelay) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(vec![relay.url.clone()]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![relay.cert.clone()])),
        loopback_only: true,
        ..Default::default()
    }
}

/// Waits until `node` has `relay` as its home, so peers can reach it through that relay.
async fn wait_home(node: &Node, relay: &Url) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while node.relay_report().home.as_ref() != Some(relay) {
        assert!(tokio::time::Instant::now() < deadline, "the relay did not become home");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    node.online().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ping_through_a_relay_that_admits_both_completes() {
    let (a_key, b_key) = (DeviceKey::generate(), DeviceKey::generate());
    let relay = spawn_relay(vec![a_key.id().0, b_key.id().0]).await;
    let a = Node::spawn(&a_key, config(&relay)).await.expect("a");
    let b = Node::spawn(&b_key, config(&relay)).await.expect("b");
    wait_home(&a, &relay.url).await;
    wait_home(&b, &relay.url).await;
    let rtt = a.ping_via_relay(&relay.url, b.id(), WAIT).await.expect("the ping goes through the relay");
    assert!(rtt < WAIT.as_millis() as u64);
    // A's own connection to the relay is back in charge afterwards: a normal ping still works.
    b.remember(&a.addr().await).unwrap();
    b.ping(&a.addr().await).await.expect("a is still reachable");
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_refuses_this_key_fails_the_ping() {
    let (a_key, b_key) = (DeviceKey::generate(), DeviceKey::generate());
    // Only B is a member: the relay refuses A's key.
    let relay = spawn_relay(vec![b_key.id().0]).await;
    let b = Node::spawn(&b_key, config(&relay)).await.expect("b");
    wait_home(&b, &relay.url).await;
    let a = Node::spawn(&a_key, config(&relay)).await.expect("a");
    assert!(a.ping_via_relay(&relay.url, b.id(), Duration::from_secs(5)).await.is_err());
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_relay_fails_the_ping() {
    let a_key = DeviceKey::generate();
    let relay = spawn_relay(vec![a_key.id().0]).await;
    let a = Node::spawn(&a_key, config(&relay)).await.expect("a");
    let dead: Url = "https://127.0.0.1:9/".parse().unwrap();
    // A real key, so the ping gets as far as dialling the relay. `[7; 32]` is not a point.
    let nobody = DeviceKey::generate().id();
    assert!(a.ping_via_relay(&dead, nobody, Duration::from_secs(3)).await.is_err());
    a.shutdown().await.unwrap();
}

/// While B's probe holds B's key on the relay, A dials B through that relay. Whatever the probe
/// endpoint does with A's packets, A's dial must still complete once the probe closes.
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_during_a_probe_still_arrives() {
    let (a_key, b_key) = (DeviceKey::generate(), DeviceKey::generate());
    let relay = spawn_relay(vec![a_key.id().0, b_key.id().0]).await;
    let a = Node::spawn(&a_key, config(&relay)).await.expect("a");
    let b = Node::spawn(&b_key, config(&relay)).await.expect("b");
    wait_home(&a, &relay.url).await;
    wait_home(&b, &relay.url).await;
    // B probes towards a peer that never answers, so its probe holds the relay for the whole wait.
    let probe = {
        let b = b.clone();
        let url = relay.url.clone();
        tokio::spawn(async move { b.ping_via_relay(&url, DeviceId([9; 32]), Duration::from_secs(3)).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    // A knows B only by its relay, so the dial goes through the relay while the probe holds it.
    let b_by_relay = dissent_net::PeerAddr { id: b.id(), relay: Some(relay.url.clone()), direct: Vec::new() };
    a.remember(&b_by_relay).unwrap();
    let started = std::time::Instant::now();
    let reply = tokio::time::timeout(WAIT, a.ping(&b_by_relay)).await;
    assert!(probe.await.unwrap().is_err(), "the probe towards nobody fails");
    let report = reply.expect("the dial finishes in time").expect("the dial during a probe still reaches b");
    eprintln!("dial during probe took {:?}, path {:?}", started.elapsed(), report.path);
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A key that is not a valid Ed25519 point, which `EndpointId::from_bytes` refuses.
fn invalid_peer() -> DeviceId {
    (0u8..=255)
        .map(|n| [n; 32])
        .find(|bytes| iroh::EndpointId::from_bytes(bytes).is_err())
        .map(DeviceId)
        .expect("some byte pattern is not a point")
}

#[tokio::test(flavor = "multi_thread")]
async fn every_probe_is_closed_whatever_ends_it() {
    let a_key = DeviceKey::generate();
    let relay = spawn_relay(vec![a_key.id().0]).await;
    let a = Node::spawn(&a_key, config(&relay)).await.expect("a");
    // A bad target fails before any probe is bound.
    let bad = a.ping_via_relay(&relay.url, invalid_peer(), Duration::from_secs(3)).await;
    assert!(matches!(bad, Err(dissent_net::NetError::BadPeer(_))), "{bad:?}");
    assert_eq!(a.open_relay_probes(), 0);
    // A ping to a peer that is not on the relay times out, and closes its probe.
    let absent = DeviceKey::generate().id();
    let timed_out = a.ping_via_relay(&relay.url, absent, Duration::from_millis(500)).await;
    assert!(matches!(timed_out, Err(dissent_net::NetError::Protocol(_))), "{timed_out:?}");
    assert_eq!(a.open_relay_probes(), 0);
    // A caller that gives up mid-ping drops the probe.
    let pending = {
        let a = a.clone();
        let url = relay.url.clone();
        tokio::spawn(async move { a.ping_via_relay(&url, absent, Duration::from_secs(30)).await })
    };
    let deadline = tokio::time::Instant::now() + WAIT;
    while a.open_relay_probes() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the probe never opened");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pending.abort();
    let _ = pending.await;
    assert_eq!(a.open_relay_probes(), 0, "an abandoned probe is not left open");
    a.shutdown().await.unwrap();
}

