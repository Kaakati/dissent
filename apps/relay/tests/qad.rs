//! QUIC address discovery: a client whose only relay is ours learns its own address from it once the relay serves
//! QAD, and learns nothing without it. Loopback only, so the address is 127.0.0.1; this shows the mechanism, not how
//! often two devices behind real routers then connect directly.

use std::{net::Ipv4Addr, time::Duration};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};
use iroh::{Watcher, tls::CaTlsConfig};
use iroh_relay::server::{CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig};
use url::Url;

/// Starts a TLS relay on loopback, serving QAD on the port clients probe when `qad` is set.
async fn relay(qad: bool) -> (Server, Url, rustls::pki_types::CertificateDer<'static>) {
    let (certs, server_config) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut relay = RelayServerConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new((Ipv4Addr::LOCALHOST, 0), CertConfig::Manual { server_config }));
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    if qad {
        dissent_relay::enable_qad(&mut config, (Ipv4Addr::LOCALHOST, dissent_relay::QAD_PORT).into());
    }
    let server = Server::spawn(config).await.expect("spawn relay");
    let url: Url = format!("https://{}", server.https_addr().expect("https")).parse().unwrap();
    (server, url, certs[0].clone())
}

/// The public IPv4 address the node's net report found, once it has picked a home relay.
async fn learned_address(qad: bool) -> Option<std::net::SocketAddrV4> {
    let (server, url, cert) = relay(qad).await;
    let config = NodeConfig {
        relay: RelayChoice::Custom(vec![url]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![cert])),
        loopback_only: true,
        dht_lookup: false,
        ..Default::default()
    };
    let node = Node::spawn(&DeviceKey::generate(), config).await.expect("spawn node");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut found = None;
    while tokio::time::Instant::now() < deadline {
        let report = node.endpoint().net_report().get();
        if let Some(report) = report.filter(|r| r.preferred_relay.is_some()) {
            found = report.global_v4;
            if found.is_some() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    node.shutdown().await.expect("shutdown");
    let _ = server.shutdown().await;
    found
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_client_learns_its_address_only_from_a_relay_that_serves_qad() {
    assert_eq!(learned_address(false).await, None, "without QAD the client learns no address from our relay");
    let learned = learned_address(true).await.expect("with QAD the client learns its address");
    assert_eq!(*learned.ip(), Ipv4Addr::LOCALHOST);
}
