//! The home relay follows measured latency. Two relays run on loopback. One sits behind a proxy that holds each
//! chunk of bytes back, so it measures slower. The node is given both, and the faster one must become home.

use std::{net::{Ipv4Addr, SocketAddr}, time::Duration};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{
    CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig,
};
use noq_proto::rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

/// How long the proxy holds each chunk. Round trips through it take about twice this.
const SLOW_DELAY: Duration = Duration::from_millis(150);
/// How long the node has to settle on the fast relay.
const SETTLE: Duration = Duration::from_secs(90);

struct TestRelay {
    _server: Server,
    addr: SocketAddr,
    cert: CertificateDer<'static>,
}

async fn spawn_relay() -> TestRelay {
    let (certs, server_config) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut relay = RelayServerConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new((Ipv4Addr::LOCALHOST, 0), CertConfig::Manual { server_config }));
    relay.key_cache_capacity = Some(1024);
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.expect("spawn relay");
    let addr = server.https_addr().expect("https addr");
    TestRelay { _server: server, addr, cert: certs[0].clone() }
}

/// Copies bytes from `reader` to `writer`, holding each chunk back by `delay`. Order is kept.
async fn forward(mut reader: impl AsyncRead + Unpin, mut writer: impl AsyncWrite + Unpin, delay: Duration) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        tokio::time::sleep(delay).await;
        if writer.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = writer.shutdown().await;
}

/// A TCP proxy in front of `target` that adds `delay` to every chunk, in both directions. The relay behind it
/// is unchanged, so its TLS certificate still checks out.
async fn slow_proxy(target: SocketAddr, delay: Duration) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("bind proxy");
    let addr = listener.local_addr().expect("proxy addr");
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(target).await else {
                    return;
                };
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                tokio::join!(
                    forward(client_read, server_write, delay),
                    forward(server_read, client_write, delay),
                );
            });
        }
    });
    addr
}

fn node_config(relays: Vec<Url>, trust: Vec<CertificateDer<'static>>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(relays),
        relay_ca: Some(CaTlsConfig::custom_roots(trust)),
        loopback_only: true,
        ..Default::default()
    }
}

fn url(addr: SocketAddr) -> Url {
    format!("https://{addr}").parse().expect("relay url")
}

#[tokio::test]
async fn the_faster_relay_becomes_home_and_latencies_are_reported() {
    let fast = spawn_relay().await;
    let slow = spawn_relay().await;
    let slow_addr = slow_proxy(slow.addr, SLOW_DELAY).await;
    let fast_url = url(fast.addr);
    let slow_url = url(slow_addr);
    // The proxy answers with the slow relay's certificate, so trusting it is enough.
    let node = Node::spawn(
        &DeviceKey::generate(),
        node_config(vec![slow_url.clone(), fast_url.clone()], vec![fast.cert.clone(), slow.cert.clone()]),
    )
    .await
    .expect("spawn node");

    let deadline = tokio::time::Instant::now() + SETTLE;
    loop {
        let report = node.relay_report();
        let fast_ms = report.latencies.iter().find(|(u, _)| *u == fast_url).and_then(|(_, l)| *l);
        let slow_ms = report.latencies.iter().find(|(u, _)| *u == slow_url).and_then(|(_, l)| *l);
        if report.home.as_ref() == Some(&fast_url) && fast_ms.is_some() && slow_ms.is_some() {
            assert!(fast_ms < slow_ms, "fast {fast_ms:?} should measure below slow {slow_ms:?}");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "fast relay did not become home: {report:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    node.shutdown().await.expect("shutdown node");
}
