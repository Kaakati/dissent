//! A forwarder reports each downlink's stats, including the bandwidth target, after traffic.
//! Links are in-memory, so no sockets are involved.

mod common;

use std::time::Duration;

use common::{memory_pair, run_participant, MemoryTransport};
use dissent_media::{Candidate, MediaKey, Mode, Participant, Sfu, MAX_KBPS, MIN_KBPS};
use tokio::sync::oneshot;
use tokio::time::timeout;

const FORWARDER: u64 = 10;
const PARTICIPANTS: [u64; 2] = [1, 2];
const RUN_FOR: Duration = Duration::from_secs(2);

/// Steps the forwarder until `stop` fires, then returns it so its stats can be read.
async fn run_forwarder(
    mut sfu: Sfu<MemoryTransport>,
    mut stop: oneshot::Receiver<()>,
) -> Sfu<MemoryTransport> {
    loop {
        tokio::select! {
            _ = &mut stop => break,
            stepped = sfu.step() => {
                if !stepped.unwrap() {
                    break;
                }
            }
        }
    }
    sfu
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downlink_stats_carry_the_bandwidth_target_after_traffic() {
    timeout(Duration::from_secs(60), run())
        .await
        .expect("the downlink run hung");
}

async fn run() {
    let key = MediaKey::new([13u8; 32]);
    let candidates = vec![Candidate {
        peer: FORWARDER,
        upload_kbps: 30_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 20,
    }];
    let mut sfu = Sfu::new(Mode::AudioTop3);
    let mut participants: Vec<Participant<MemoryTransport>> = PARTICIPANTS
        .iter()
        .map(|id| {
            Participant::new(key.clone(), *id, candidates.clone(), PARTICIPANTS.len(), Mode::AudioTop3)
                .unwrap()
        })
        .collect();
    for participant in participants.iter_mut() {
        let (participant_end, forwarder_end) = memory_pair();
        let offer = participant.offer_sfu(FORWARDER, participant_end).await.unwrap();
        let answer = sfu
            .accept(participant.id(), forwarder_end, offer)
            .await
            .unwrap();
        participant.apply_sfu(FORWARDER, answer).unwrap();
        participant.elect().unwrap();
    }

    let (stop_tx, stop_rx) = oneshot::channel();
    let forwarder = tokio::spawn(run_forwarder(sfu, stop_rx));
    let mut calls = Vec::new();
    for participant in participants {
        let id = participant.id();
        calls.push(tokio::spawn(run_participant(participant, id, 40, RUN_FOR)));
    }
    // The participants stay alive until the stats are read, because dropping one closes its link.
    let mut finished = Vec::new();
    for call in calls {
        finished.push(call.await.unwrap());
    }
    let _ = stop_tx.send(());
    let sfu = forwarder.await.unwrap();

    let stats = sfu.downlink_stats();
    assert_eq!(
        stats.keys().copied().collect::<Vec<_>>(),
        PARTICIPANTS.to_vec(),
        "every downlink reports its stats"
    );
    for (peer, call) in &stats {
        assert!(
            call.packets_received > 0,
            "the downlink to {peer} carried no traffic"
        );
        assert!(
            (MIN_KBPS..=MAX_KBPS).contains(&call.target_kbps),
            "the downlink to {peer} reports no bandwidth target: {}",
            call.target_kbps
        );
    }
    drop(finished);
}
