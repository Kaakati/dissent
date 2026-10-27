//! Server boosts: a member who publishes a relay this app has verified is a host, and the level
//! follows the hosts. One root counts once, a relay that stays unverified for over 72 hours stops
//! counting, and the ceilings follow the level. See `docs/design/12-boosts.md`.

use crate::common::*;
use dissent_core::boosts::HOST_GRACE_MS;
use dissent_core::CommunityId;

#[tokio::test(flavor = "multi_thread")]
async fn a_verified_relay_makes_a_host_and_the_level_follows() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Hosts".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let view = alice.engine.community_view(id).unwrap();
        assert_eq!((view.boost.level, view.boost.hosts.len(), view.boost.voice_kbps), (0, 0, 96));
        assert_eq!(view.boost.next_level_hosts, Some(1));

        assert!(alice.engine.set_hosted_relays(id, vec!["http://plain.example".into()]).await.is_err(), "https only");
        let relays = vec!["https://relay-a.example/".to_string(), "https://relay-b.example/".to_string()];
        alice.engine.set_hosted_relays(id, relays.clone()).await.expect("publish hosted relays");
        // Published but not verified yet: no host.
        assert_eq!(alice.engine.community_view(id).unwrap().boost.level, 0);

        let now = now();
        alice.engine.record_relay_check(&relays[0], now);
        alice.engine.record_relay_check(&relays[1], now);
        let view = alice.engine.community_view(id).unwrap();
        assert_eq!(view.boost.level, 1);
        assert_eq!(view.boost.hosts.len(), 1, "two relays, one root, one host");
        assert_eq!(view.boost.hosts[0].relays, relays);
        assert!(view.members.iter().any(|m| m.relay_host), "the member list marks the host");
        assert_eq!((view.boost.video_height, view.boost.video_fps), (1080, 30));
        let me = alice.engine.user_profile(alice.identity.root_id());
        assert_eq!(me.hosts_for.len(), 1);

        // Both relays last passed over 72 hours ago: the host stops counting.
        let stale = now - HOST_GRACE_MS - 1000;
        alice.engine.record_relay_check(&relays[0], stale);
        alice.engine.record_relay_check(&relays[1], stale);
        let view = alice.engine.community_view(id).unwrap();
        assert_eq!((view.boost.level, view.boost.hosts.len()), (0, 0));
        assert!(!view.members.iter().any(|m| m.relay_host));

        // Withdrawing removes the host even with a fresh check.
        alice.engine.record_relay_check(&relays[0], now);
        alice.engine.set_hosted_relays(id, Vec::new()).await.expect("withdraw");
        assert_eq!(alice.engine.community_view(id).unwrap().boost.level, 0);
        finish(vec![alice]).await;
    })
    .await;
}

mod through_a_relay {
    use super::*;
    use dissent_core::{Engine, Identity, NodeConfig, RelayChoice};
    use iroh::tls::CaTlsConfig;
    use iroh_relay::server::{CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig};
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
        relay.tls = Some(TlsConfig::new((Ipv4Addr::LOCALHOST, 0), CertConfig::Manual { server_config }));
        relay.key_cache_capacity = Some(1024);
        let mut config = ServerConfig::default();
        config.relay = Some(relay);
        let server = Server::spawn(config).await.expect("spawn relay");
        let url: Url = format!("https://{}", server.https_addr().expect("https addr")).parse().expect("relay url");
        TestRelay { _server: server, url, cert: certs[0].clone() }
    }

    /// An engine homed on `relay`, trusting its certificate. `server_relays` is the "Use relays
    /// offered by my servers" setting.
    async fn start_on_relay(relay: &TestRelay, name: &str, server_relays: bool) -> Peer {
        start_on_relays(relay, &[relay], name, server_relays).await
    }

    /// An engine configured with `home` only, trusting the certificates of all of `trusted`.
    async fn start_on_relays(home: &TestRelay, trusted: &[&TestRelay], name: &str, server_relays: bool) -> Peer {
        let dir = scratch(name);
        let identity = Identity::create(now());
        let net = NodeConfig {
            relay: RelayChoice::Custom(vec![home.url.clone()]),
            relay_ca: Some(CaTlsConfig::custom_roots(trusted.iter().map(|r| r.cert.clone()).collect::<Vec<_>>())),
            loopback_only: true,
            community_relays: server_relays,
            ..NodeConfig::default()
        };
        let (engine, events) = Engine::start_with_timings(
            identity.clone(),
            dir.clone(),
            net,
            dissent_core::test_support::vault_for(&dir),
            dissent_core::test_support::fast_timings(),
        )
        .await
        .expect("engine starts");
        engine.set_display_name(name.to_string()).await.expect("name");
        Peer { engine, identity, dir, _events: events }
    }

    /// Runs relay checks until `engine` counts a host for `id`, or `limit` runs out.
    async fn level_after_checks(engine: &Engine, id: CommunityId, limit: Duration) -> u8 {
        let deadline = Instant::now() + limit;
        loop {
            dissent_core::test_support::check_relay_hosts(engine).await;
            let level = engine.community_view(id).unwrap().boost.level;
            if level > 0 || Instant::now() > deadline {
                return level;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "networked: run with scripts/fwtest.sh"]
    async fn a_member_counts_a_host_only_after_reaching_them_through_their_relay() {
        bounded(async {
            let relay = spawn_relay().await;
            let alice = start_on_relay(&relay, "alice-host", true).await;
            let bob = start_on_relay(&relay, "bob-member", true).await;
            let carol = start_on_relay(&relay, "carol-no-server-relays", false).await;
            let id = rustaceans(&alice, &bob).await;
            let invite = alice.engine.create_invite(id).await.expect("invite");
            join_ready(&alice, &carol, &invite).await;
            alice.engine.set_hosted_relays(id, vec![relay.url.to_string()]).await.expect("publish");
            eventually("bob and carol see the published relay", || {
                let b = bob.engine.community_view(id).ok()?;
                let c = carol.engine.community_view(id).ok()?;
                (b.boost.level == 0 && c.boost.level == 0).then_some(())
            })
            .await;
            // Bob reaches alice through her relay, so he counts her.
            assert_eq!(level_after_checks(&bob.engine, id, Duration::from_secs(30)).await, 1, "bob counts alice");
            let view = bob.engine.community_view(id).unwrap();
            assert_eq!(view.boost.hosts.len(), 1);
            assert_eq!(view.boost.hosts[0].root_id, alice.identity.root_id().to_string());
            // Carol has "Use relays offered by my servers" off: she probes nothing and counts no one.
            assert_eq!(level_after_checks(&carol.engine, id, Duration::from_secs(3)).await, 0, "carol counts nothing");
            finish(vec![alice, bob, carol]).await;
        })
        .await;
    }

    fn has_relay(p: &Peer, url: &str) -> bool {
        dissent_core::test_support::adopted_relays(&p.engine).iter().any(|r| r == url)
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "networked: run with scripts/fwtest.sh"]
    async fn a_hosted_relay_is_added_only_once_it_passes_and_dropped_once_withdrawn() {
        bounded(async {
            let hosted = spawn_relay().await;
            let own = spawn_relay().await;
            let alice = start_on_relays(&hosted, &[&hosted], "alice-host", true).await;
            // Bob has his own relay, and trusts the hosted one only so the check can reach it.
            let bob = start_on_relays(&own, &[&own, &hosted], "bob-member", true).await;
            let id = rustaceans(&alice, &bob).await;
            let good = hosted.url.to_string();
            // Nothing listens here, so this one never passes.
            let dead = "https://127.0.0.1:1/".to_string();
            alice.engine.set_hosted_relays(id, vec![dead.clone(), good.clone()]).await.expect("publish");
            eventually("bob sees both published relays", || {
                (bob.engine.hosted_relays().len() == 2).then_some(())
            })
            .await;
            assert!(!has_relay(&bob, &good) && !has_relay(&bob, &dead), "nothing is added before a check");
            assert_eq!(level_after_checks(&bob.engine, id, Duration::from_secs(30)).await, 1, "bob counts alice");
            assert!(has_relay(&bob, &good), "the relay that passed is added");
            // Probe the dead one too, then check it is still not added.
            for _ in 0..2 {
                dissent_core::test_support::check_relay_hosts(&bob.engine).await;
            }
            assert!(!has_relay(&bob, &dead), "a relay that never passed is never added");

            alice.engine.set_hosted_relays(id, Vec::new()).await.expect("withdraw");
            eventually("bob sees the relays withdrawn", || {
                bob.engine.hosted_relays().is_empty().then_some(())
            })
            .await;
            dissent_core::test_support::check_relay_hosts(&bob.engine).await;
            assert!(!has_relay(&bob, &good), "a relay that stops counting is dropped");
            assert!(has_relay(&bob, own.url.as_str()), "bob's configured relay stays");
            finish(vec![alice, bob]).await;
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_relay_probe_runs_during_a_call() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Quiet".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let relay = "https://relay.quiet.example/";
        alice.engine.set_hosted_relays(id, vec![relay.into()]).await.expect("publish");

        dissent_core::test_support::pretend_in_call(&alice.engine, true);
        dissent_core::test_support::check_relay_hosts(&alice.engine).await;
        assert_eq!(dissent_core::test_support::relays_probed(&alice.engine), 0, "the tick skips during a call");
        let busy = alice.engine.probe_relay(relay, alice.identity.device_id()).await;
        assert_eq!(busy, Err(dissent_core::boosts::MEMBER_BUSY.to_string()), "a relay check answers busy");

        dissent_core::test_support::pretend_in_call(&alice.engine, false);
        dissent_core::test_support::check_relay_hosts(&alice.engine).await;
        assert_eq!(dissent_core::test_support::relays_probed(&alice.engine), 1, "after the call the tick probes");
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn relays_on_a_private_network_are_refused_and_never_adopted() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Home".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        for local in ["https://192.168.1.1", "https://10.0.0.7:443/", "https://nas.local/", "https://router.lan"] {
            assert!(alice.engine.set_hosted_relays(id, vec![local.into()]).await.is_err(), "{local} is refused as a hosted relay");
            assert!(alice.engine.set_community_relays(id, vec![local.into()]).await.is_err(), "{local} is refused as a server relay");
        }

        // Another app can still publish one, since replay takes any https URL. This app neither
        // adopts it nor counts it.
        let lan = "https://192.168.1.1/";
        dissent_core::test_support::publish_hosted_relays_unchecked(&alice.engine, id, vec![lan.into()])
            .await
            .expect("publish without checks");
        dissent_core::test_support::check_relay_hosts(&alice.engine).await;
        assert!(
            !dissent_core::test_support::adopted_relays(&alice.engine).iter().any(|r| r.contains("192.168.1.1")),
            "a private relay is never added to the node"
        );
        assert_eq!(dissent_core::test_support::relay_checks(&alice.engine), 0, "and never passes");
        assert_eq!(alice.engine.community_view(id).unwrap().boost.level, 0);
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_and_probe_times_go_when_the_relay_is_withdrawn() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Tidy".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let relay = "https://relay.tidy.example/";
        alice.engine.set_hosted_relays(id, vec![relay.into()]).await.expect("publish");
        alice.engine.record_relay_check(relay, now());
        // A check for a relay no one publishes, as one withdrawn before this run.
        alice.engine.record_relay_check("https://relay.gone.example/", now());
        dissent_core::test_support::check_relay_hosts(&alice.engine).await;
        assert_eq!(dissent_core::test_support::relays_probed(&alice.engine), 1);
        assert_eq!(dissent_core::test_support::relay_checks(&alice.engine), 1, "only the published relay's check is kept");

        alice.engine.set_hosted_relays(id, Vec::new()).await.expect("withdraw");
        dissent_core::test_support::check_relay_hosts(&alice.engine).await;
        assert_eq!(dissent_core::test_support::relays_probed(&alice.engine), 0, "the probe time goes");
        assert_eq!(dissent_core::test_support::relay_checks(&alice.engine), 0, "the check goes");
        finish(vec![alice]).await;
    })
    .await;
}
