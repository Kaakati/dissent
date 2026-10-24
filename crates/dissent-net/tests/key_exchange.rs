use std::time::Duration;

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, PingReport, PathKind, RelayChoice};
use iroh::{Endpoint, RelayMode, endpoint::presets};
use noq_proto::{crypto::rustls::HandshakeData, rustls::NamedGroup};

const PING_ALPN: &[u8] = b"dissent/ping/0";

fn loopback() -> NodeConfig {
    NodeConfig { relay: RelayChoice::Disabled, loopback_only: true, ..Default::default() }
}

/// Two nodes both prefer the hybrid group, so the ping must report it.
#[tokio::test]
async fn nodes_negotiate_hybrid_key_exchange() {
    let a = Node::spawn(&DeviceKey::generate(), loopback()).await.expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), loopback()).await.expect("spawn b");

    let report: PingReport = b.ping(&a.addr().await).await.expect("ping");
    assert_eq!(report.pq, Some(true));
    assert_eq!(report.path, PathKind::Direct);

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

/// A classical-only peer (ring provider, no ML-KEM) must still connect and get X25519.
#[tokio::test]
async fn classical_peer_falls_back_to_x25519() {
    let a = Node::spawn(&DeviceKey::generate(), loopback()).await.expect("spawn a");
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .expect("bind classical client");

    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        client.connect(a.addr().await.to_iroh().expect("addr"), PING_ALPN),
    )
    .await
    .expect("connect timed out")
    .expect("classical peer must still connect");

    let group = conn
        .handshake_data()
        .and_then(|d| d.downcast::<HandshakeData>().ok())
        .and_then(|d| d.negotiated_key_exchange_group);
    assert_eq!(group, Some(NamedGroup::X25519));

    conn.close(0u32.into(), b"done");
    client.close().await;
    a.shutdown().await.expect("shutdown a");
}
