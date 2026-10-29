//! A direct call between two engines homed on a relay server started in this process. The engines
//! are `relay_only`: without it, two engines on one machine hole-punch to a direct path before the
//! first audio frame, and the relay forwards nothing. The relay listens on loopback only, and the
//! engines bind no IP socket. The relay's own packet counter shows whether the call's media travels
//! through it, independent of what the call view reports.

use crate::common::*;
use dissent_core::{Engine, Identity, NodeConfig, PathView, RelayChoice, Topology};
use iroh::tls::CaTlsConfig;
use iroh_relay::server::{CertConfig, RelayConfig as RelayServerConfig, Server, ServerConfig, TlsConfig};
use noq_proto::rustls::pki_types::CertificateDer;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use url::Url;

/// How long the call is watched once audio flows both ways.
const WATCH: Duration = Duration::from_secs(20);
/// Relayed packets per second below which the media is taken to have left the relay. Audio alone is
/// about 50 packets a second each way, so a relayed call carries about 100.
const RELAYED_FLOOR: u64 = 30;

struct TestRelay {
    server: Server,
    url: Url,
    cert: CertificateDer<'static>,
}

impl TestRelay {
    /// Packets the relay has forwarded so far, in all directions.
    fn forwarded(&self) -> u64 {
        self.server.metrics().server.send_packets_sent.get()
    }
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
    TestRelay { server, url, cert: certs[0].clone() }
}

/// An engine that reaches peers only through `relay`, trusting its certificate. `relay_only` gives
/// the node no IP transport. `loopback_only` is still set, because the engine reads it too: it keeps
/// the engine's own DHT client off (so contact links carry the relay address instead of a DHT
/// short address) and lets it accept a relay on 127.0.0.1. DHT lookup in the node is off, so no
/// UDP socket opens at all.
async fn start_on_relay(relay: &TestRelay, name: &str) -> Peer {
    let dir = scratch(name);
    let identity = Identity::create(now());
    let net = NodeConfig {
        relay: RelayChoice::Custom(vec![relay.url.clone()]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![relay.cert.clone()])),
        relay_only: true,
        loopback_only: true,
        dht_lookup: false,
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

/// One second of the call.
#[derive(Debug)]
struct Sample {
    t_s: u64,
    relayed_per_s: u64,
    alice_frames: u64,
    bob_frames: u64,
    alice_sees: Option<PathView>,
    bob_sees: Option<PathView>,
}

/// Makes `a` and `b` contacts. A relay-only engine has no address until it reaches the relay, so
/// its first contact links carry nothing to dial. Each try takes a fresh link.
async fn contacts_once_online(a: &Peer, b: &Peer) {
    wait_online(&[a, b]).await;
    for (from, to) in [(a, b), (b, a)] {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match from.engine.add_contact(&to.engine.contact_link()).await {
                Ok(_) => break,
                Err(e) if Instant::now() < deadline => {
                    eprintln!("RELAYCALL add_contact not ready yet: {e}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => panic!("add_contact through the relay: {e}"),
            }
        }
    }
    for (who, other) in [(a, b), (b, a)] {
        let other = other.identity.root_id().to_string();
        eventually("each side lists the other as a contact", || {
            who.engine.list_contacts().contacts.iter().any(|c| c.root_id == other).then_some(())
        })
        .await;
    }
}

/// Waits until every engine has a home relay, so the addresses it hands out can be dialled.
async fn wait_online(peers: &[&Peer]) {
    for p in peers {
        let deadline = Instant::now() + Duration::from_secs(30);
        while p.engine.net_status().await.relay.is_none() {
            assert!(Instant::now() < deadline, "no home relay within 30 s");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// Every path each engine shows for the others in its call, by engine name.
fn paths_shown(everyone: &[(&str, &Peer)]) -> Vec<(String, Vec<PathView>)> {
    everyone
        .iter()
        .map(|(name, p)| {
            let paths = p.engine.call_state().map(|v| v.participants.iter().map(|x| x.path).collect()).unwrap_or_default();
            (name.to_string(), paths)
        })
        .collect()
}

/// Seven engines in a voice channel make a forwarded call. Joins the channel on all of them and
/// waits until each sees six others in a forwarded call and hears audio.
async fn forwarded_call(owner: &Peer, others: &[Peer]) {
    let refs: Vec<&Peer> = others.iter().collect();
    let (community, channel) = crate::calls::voice_community(owner, &refs).await;
    let everyone: Vec<&Peer> = std::iter::once(owner).chain(others.iter()).collect();
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join the voice channel");
    }
    for p in &everyone {
        eventually_within(Duration::from_secs(90), "a forwarded call with six others that is heard", || {
            p.engine
                .call_state()
                .filter(|v| v.participants.len() == 6 && v.topology == Topology::Forwarded && v.frames_received > 0)
        })
        .await;
    }
}

/// The path each participant shows comes from the live link, also in a forwarded call, where most
/// participants reach this device through the forwarder. With every engine relay-only, no path can
/// be direct, so every participant on every engine must show relayed. Before the fix, a forwarded
/// call listed participants as direct whatever their link was.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarded_call_through_a_relay_shows_every_participant_relayed() {
    let _serial = heavy_test_lock().await;
    bounded(async {
        let relay = spawn_relay().await;
        let owner = start_on_relay(&relay, "owner-relayed").await;
        let mut others = Vec::new();
        for n in 1..7 {
            others.push(start_on_relay(&relay, &format!("member-relayed-{n}")).await);
        }
        let mut all: Vec<&Peer> = vec![&owner];
        all.extend(others.iter());
        wait_online(&all).await;
        forwarded_call(&owner, &others).await;

        let names: Vec<String> = (0..all.len()).map(|i| format!("engine {i}")).collect();
        let everyone: Vec<(&str, &Peer)> = names.iter().map(String::as_str).zip(all.iter().copied()).collect();
        for _ in 0..5 {
            let shown = paths_shown(&everyone);
            assert!(
                shown.iter().all(|(_, paths)| paths.len() == 6 && paths.iter().all(|p| *p == PathView::Relayed)),
                "every participant must show relayed: {shown:?}"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        let mut peers = vec![owner];
        peers.extend(others);
        finish(peers).await;
    })
    .await;
}

/// The same forwarded call on loopback with relays off, where every link is direct. A participant
/// reached through the forwarder shows the forwarder link's path, so all six show direct.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarded_call_on_direct_links_shows_every_participant_direct() {
    let _serial = heavy_test_lock().await;
    bounded(async {
        let owner = peer("owner-direct").await;
        let mut others = Vec::new();
        for n in 1..7 {
            others.push(peer(&format!("member-direct-{n}")).await);
        }
        forwarded_call(&owner, &others).await;

        let all: Vec<&Peer> = std::iter::once(&owner).chain(others.iter()).collect();
        let names: Vec<String> = (0..all.len()).map(|i| format!("engine {i}")).collect();
        let everyone: Vec<(&str, &Peer)> = names.iter().map(String::as_str).zip(all.iter().copied()).collect();
        let shown = paths_shown(&everyone);
        assert!(
            shown.iter().all(|(_, paths)| paths.len() == 6 && paths.iter().all(|p| *p == PathView::Direct)),
            "every participant must show direct: {shown:?}"
        );

        let mut peers = vec![owner];
        peers.extend(others);
        finish(peers).await;
    })
    .await;
}

fn path_seen(engine: &Engine) -> Option<PathView> {
    engine.call_state().and_then(|v| v.participants.first().map(|p| p.path))
}

fn frames(engine: &Engine) -> u64 {
    engine.call_state().map_or(0, |v| v.frames_received)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_dm_call_through_a_relay_carries_audio_both_ways_on_the_relay() {
    bounded(async {
        let relay = spawn_relay().await;
        let alice = start_on_relay(&relay, "alice-relayed").await;
        let bob = start_on_relay(&relay, "bob-relayed").await;
        contacts_once_online(&alice, &bob).await;

        let mut bob_events = bob.engine.subscribe();
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        let started = Instant::now();
        eventually("audio flows both ways", || {
            (frames(&alice.engine) > 0 && frames(&bob.engine) > 0).then_some(())
        })
        .await;
        let first_audio = started.elapsed();

        let mut samples = Vec::new();
        let mut last = relay.forwarded();
        for t_s in 1..=WATCH.as_secs() {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = relay.forwarded();
            samples.push(Sample {
                t_s,
                relayed_per_s: now - last,
                alice_frames: frames(&alice.engine),
                bob_frames: frames(&bob.engine),
                alice_sees: path_seen(&alice.engine),
                bob_sees: path_seen(&bob.engine),
            });
            last = now;
        }
        eprintln!("RELAYCALL first audio both ways after {first_audio:?}");
        for s in &samples {
            eprintln!("RELAYCALL {s:?}");
        }

        // Audio keeps arriving on both sides for the whole watch.
        let (first, end) = (&samples[0], &samples[samples.len() - 1]);
        assert!(end.alice_frames > first.alice_frames, "alice stopped hearing bob: {samples:#?}");
        assert!(end.bob_frames > first.bob_frames, "bob stopped hearing alice: {samples:#?}");
        // The call view reports the link as relayed on both sides.
        assert!(
            samples.iter().all(|s| s.alice_sees == Some(PathView::Relayed) && s.bob_sees == Some(PathView::Relayed)),
            "the call view must show a relayed path throughout: {samples:#?}"
        );
        // The relay carries the media every second. If the link went direct, this count would fall
        // to keepalives, so the test cannot quietly become a direct-path test.
        let off_relay: Vec<u64> = samples.iter().filter(|s| s.relayed_per_s < RELAYED_FLOOR).map(|s| s.t_s).collect();
        assert!(off_relay.is_empty(), "media left the relay in seconds {off_relay:?}: {samples:#?}");

        finish(vec![alice, bob]).await;
    })
    .await;
}
