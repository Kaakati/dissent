//! Community relays on loopback, through a relay server started in this process with a
//! self-signed certificate that the nodes are told to trust.


use crate::common::*;
use dissent_core::{CommunityId, Engine, Identity, NodeConfig, RelayChoice};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{
    CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig,
};
use noq_proto::rustls::pki_types::CertificateDer;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
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
    TestRelay {
        _server: server,
        url,
        cert: certs[0].clone(),
    }
}

/// A node that trusts the test relay's certificate and uses no relay of its own.
async fn start_trusting(relay: &TestRelay, name: &str) -> Peer {
    let dir = scratch(name);
    let identity = Identity::create(now());
    let net = NodeConfig {
        relay: RelayChoice::Custom(Vec::new()),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![relay.cert.clone()])),
        loopback_only: true,
        ..NodeConfig::default()
    };
    let (engine, events) = Engine::start(identity.clone(), dir.clone(), net, dissent_core::test_support::vault_for(&dir))
        .await
        .expect("engine starts");
    engine
        .set_display_name(name.to_string())
        .await
        .expect("name");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

async fn wait_for_relay(engine: &Engine, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if engine.net_status().await.relay.is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never reached the community relay");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn community_relays_reach_members_online_and_joiners() {
    bounded(async {
        let relay = spawn_relay().await;
        let alice = start_trusting(&relay, "alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Relayed".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();

        // Refused: not https, and too many.
        assert!(alice
            .engine
            .set_community_relays(id, vec!["http://relay.example.org".into()])
            .await
            .is_err());
        let five: Vec<String> = (0..5).map(|i| format!("https://r{i}.example.org")).collect();
        assert!(alice.engine.set_community_relays(id, five).await.is_err());

        // Carol joins before any relay is named.
        let early = alice.engine.create_invite(id).await.expect("invite");
        let carol = start_trusting(&relay, "carol").await;
        carol
            .engine
            .join_community(&early)
            .await
            .expect("carol joins");

        // A plain member cannot name relays.
        assert!(carol
            .engine
            .set_community_relays(id, vec![relay.url.to_string()])
            .await
            .is_err());

        // Alice names the relay. Carol, already online, must pick it up from the event.
        alice
            .engine
            .set_community_relays(id, vec![relay.url.to_string()])
            .await
            .expect("alice sets the relay");
        assert_eq!(alice.engine.community_relays(id), vec![relay.url.to_string()]);
        wait_for_relay(&carol.engine, "carol").await;

        // Bob joins with an invite, which carries the relay. His first connection can use it.
        let with_relay = alice.engine.create_invite(id).await.expect("invite");
        let bob = start_trusting(&relay, "bob").await;
        bob.engine
            .join_community(&with_relay)
            .await
            .expect("bob joins");
        wait_for_relay(&bob.engine, "bob").await;

        finish(vec![alice, carol, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_changes_its_relays_many_times_keeps_only_the_current_ones() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Restless".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let adopted = |p: &Peer| dissent_core::test_support::adopted_relays(&p.engine);
        let before = adopted(&alice).len();
        // Ten lists of four over one run: forty adoptions, more than the node's cap of 30.
        for round in 0..10 {
            let relays: Vec<String> = (0..4).map(|i| format!("https://r{round}-{i}.restless.example/")).collect();
            alice.engine.set_community_relays(id, relays.clone()).await.expect("set relays");
            let now = adopted(&alice);
            for relay in &relays {
                assert!(now.contains(relay), "round {round}: {relay} is adopted");
            }
            assert!(
                !now.iter().any(|r| r.contains(".restless.example") && !relays.contains(r)),
                "round {round}: relays the server no longer names are gone: {now:?}"
            );
            assert_eq!(now.len(), before + 4);
        }
        alice.engine.set_community_relays(id, Vec::new()).await.expect("clear");
        assert_eq!(adopted(&alice).len(), before, "with no relays named, only the configured ones stay");
        finish(vec![alice]).await;
    })
    .await;
}
