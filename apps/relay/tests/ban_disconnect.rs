//! A ban ends a member's open connection through the hosted relay, and the banned member cannot reconnect,
//! while the other member stays connected throughout. Real clients on loopback, a real relay, and the same
//! access hook and disconnect path the relay for my servers uses.

use std::{net::Ipv4Addr, time::Duration};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};
use dissent_relay::hosted::{apply, Caps, ConfigBuilder, HostedAccess, RelayHost};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{CertConfig, RelayConfig as RelayServerConfig, ServerConfig, TlsConfig};
use url::Url;

const WAIT: Duration = Duration::from_secs(20);
const REFUSED: Duration = Duration::from_secs(5);

fn node_config(url: &Url, cert: &rustls::pki_types::CertificateDer<'static>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Custom(vec![url.clone()]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![cert.clone()])),
        loopback_only: true,
        ..Default::default()
    }
}

/// Waits until `node` has `url` as its home relay, so peers can reach it through the relay.
async fn wait_home(node: &Node, url: &Url) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while node.relay_report().home.as_ref() != Some(url) {
        assert!(tokio::time::Instant::now() < deadline, "the relay did not become home");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    node.online().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_ban_ends_the_connection_and_the_banned_member_cannot_reconnect() {
    let (a_key, b_key) = (DeviceKey::generate(), DeviceKey::generate());
    let caps = Caps::default();
    let access = HostedAccess::new(&caps, [a_key.id().0, b_key.id().0]);

    // The relay's certificate is known to the clients, so the test can run without a public name.
    let (certs, server_config) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let cert = certs[0].clone();
    // The builder runs once, at start, so the TLS config is handed over through a slot.
    let slot = std::sync::Mutex::new(Some(server_config));
    let build: ConfigBuilder = Box::new(move |access| {
        let tls = slot.lock().unwrap().take();
        Box::pin(async move {
            let server_config = tls.ok_or("the test relay is built once")?;
            let mut relay = RelayServerConfig::new((Ipv4Addr::LOCALHOST, 0));
            relay.tls = Some(TlsConfig::new((Ipv4Addr::LOCALHOST, 0), CertConfig::Manual { server_config }));
            relay.key_cache_capacity = Some(1024);
            let mut config = ServerConfig::default();
            config.relay = Some(relay);
            apply(&caps, access, &mut config);
            Ok(config)
        })
    });
    let mut host = RelayHost::start(build, access.clone()).await.expect("start the relay");
    let url: Url = format!("https://{}", host.addr().expect("the relay listens")).parse().unwrap();

    let a = Node::spawn(&a_key, node_config(&url, &cert)).await.expect("member a");
    let b = Node::spawn(&b_key, node_config(&url, &cert)).await.expect("member b");
    wait_home(&a, &url).await;
    wait_home(&b, &url).await;
    assert_eq!(access.live(), 2, "both members are connected");

    // Ban a: the list is replaced without its key. Its open connection is closed on the server.
    assert_eq!(host.set_members([b_key.id().0]), 1, "a's connection is closed");
    assert_eq!(access.live(), 1, "only b is still connected");

    // While a is banned, b reaches the relay and a cannot reach b through it, and a cannot reconnect:
    // the relay refuses a's key, so a's probe through the relay fails.
    assert!(
        b.ping_via_relay(&url, a_key.id(), REFUSED).await.is_err(),
        "a is no longer reachable through the relay"
    );
    assert!(
        a.ping_via_relay(&url, b_key.id(), REFUSED).await.is_err(),
        "the banned member cannot reconnect through the relay"
    );

    // b stays connected throughout: its home relay is still the relay, and it is still admitted.
    assert_eq!(b.relay_report().home.as_ref(), Some(&url), "b stays connected");
    assert_eq!(access.live(), 1, "the relay still holds b, and only b");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    host.stop().await;
}
