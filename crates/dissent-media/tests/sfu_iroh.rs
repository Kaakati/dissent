//! A forwarded two-participant call over real iroh media links on loopback, relays disabled.
//! Two participants send through one forwarder, which runs on its own dissent-net node.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::run_participant;
use dissent_identity::{DeviceId, DeviceKey};
use dissent_media::{Candidate, LinkTransport, MediaKey, Mode, Participant, Sfu, Side};
use dissent_net::{MediaHandler, MediaLink, Node, NodeConfig, PathKind, RelayChoice};
use tokio::sync::mpsc;
use tokio::time::timeout;

const FORWARDER: u64 = 100;
const SPEAKER_LEVEL: u8 = 10;

struct Inbound(mpsc::UnboundedSender<(DeviceId, MediaLink)>);

impl MediaHandler for Inbound {
    fn incoming(&self, from: DeviceId, link: MediaLink) {
        let _ = self.0.send((from, link));
    }
}

fn cfg(media: Option<Arc<dyn MediaHandler>>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        media,
        ..Default::default()
    }
}

fn forwarder_candidate() -> Candidate {
    Candidate {
        peer: FORWARDER,
        upload_kbps: 10_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 5,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn two_participants_talk_through_a_forwarder_over_iroh() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let forwarder_node = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Inbound(tx)))))
        .await
        .expect("spawn forwarder");
    let alice_node = Node::spawn(&DeviceKey::generate(), cfg(None))
        .await
        .expect("spawn alice");
    let bob_node = Node::spawn(&DeviceKey::generate(), cfg(None))
        .await
        .expect("spawn bob");
    alice_node
        .remember(&forwarder_node.addr().await)
        .expect("remember forwarder");
    bob_node
        .remember(&forwarder_node.addr().await)
        .expect("remember forwarder");

    // Each participant dials the forwarder, and the forwarder accepts both links.
    let link_alice = alice_node
        .open_media(forwarder_node.id())
        .await
        .expect("alice dials");
    let link_bob = bob_node
        .open_media(forwarder_node.id())
        .await
        .expect("bob dials");
    let mut accepted: Vec<(DeviceId, MediaLink)> = Vec::new();
    for _ in 0..2 {
        accepted.push(
            timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("forwarder did not accept a link")
                .expect("handler closed"),
        );
    }
    let link_at = |who: DeviceId| {
        accepted
            .iter()
            .position(|(from, _)| *from == who)
            .expect("no link from this participant")
    };
    let (alice_side, bob_side) = (link_at(alice_node.id()), link_at(bob_node.id()));
    let mut accepted: Vec<Option<MediaLink>> = accepted.into_iter().map(|(_, l)| Some(l)).collect();
    let link_from_alice = accepted[alice_side].take().unwrap();
    let link_from_bob = accepted[bob_side].take().unwrap();
    assert_eq!(link_alice.path(), PathKind::Direct);

    let key = MediaKey::new([8; 32]);
    let candidates = vec![forwarder_candidate()];
    let mut sfu: Sfu<LinkTransport> = Sfu::new(Mode::AudioTop3);
    let mut alice =
        Participant::new(key.clone(), 1, candidates.clone(), 2, Mode::AudioTop3).unwrap();
    let mut bob = Participant::new(key, 2, candidates, 2, Mode::AudioTop3).unwrap();

    // Signalling runs in process here. The packets themselves cross iroh.
    let offer = alice
        .offer_sfu(FORWARDER, LinkTransport::new(link_alice, Side::Caller))
        .await
        .expect("alice offers");
    let answer = sfu
        .accept(1, LinkTransport::new(link_from_alice, Side::Callee), offer)
        .await
        .expect("forwarder accepts alice");
    alice.apply_sfu(FORWARDER, answer).unwrap();

    let offer = bob
        .offer_sfu(FORWARDER, LinkTransport::new(link_bob, Side::Caller))
        .await
        .expect("bob offers");
    let answer = sfu
        .accept(2, LinkTransport::new(link_from_bob, Side::Callee), offer)
        .await
        .expect("forwarder accepts bob");
    bob.apply_sfu(FORWARDER, answer).unwrap();
    alice.elect().unwrap();
    bob.elect().unwrap();
    assert_eq!(alice.active(), Some(FORWARDER));

    let forwarder = tokio::spawn(async move { sfu.run().await });
    let (alice_done, bob_done) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_participant(alice, 1, SPEAKER_LEVEL, Duration::from_millis(1500)),
            run_participant(bob, 2, SPEAKER_LEVEL, Duration::from_millis(1500)),
        )
    })
    .await
    .expect("the call did not finish in time");
    forwarder.abort();

    let (alice, from_bob) = alice_done;
    let (bob, from_alice) = bob_done;
    for (name, arrivals, from) in [("alice", &from_bob, 2u64), ("bob", &from_alice, 1u64)] {
        assert!(!arrivals.is_empty(), "{name} heard nothing");
        assert!(
            arrivals.iter().all(|a| a.sender == from),
            "{name} heard a stranger"
        );
        assert!(
            arrivals.iter().all(|a| a.intact),
            "{name} got an altered frame"
        );
        println!(
            "iroh smoke: {name} heard {} frames from peer {from}",
            arrivals.len()
        );
    }
    assert!(
        alice.failovers().is_empty() && bob.failovers().is_empty(),
        "no failover expected"
    );
    for stats in alice.link_stats().values().chain(bob.link_stats().values()) {
        assert_eq!(stats.frames_undecryptable, 0);
    }
}
