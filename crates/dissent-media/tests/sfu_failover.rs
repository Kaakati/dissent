//! A 20-participant audio call through peer-SFU forwarders, with the primary forwarder lost
//! mid-call. Every link is an in-memory channel, so no sockets are involved.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{memory_pair, run_participant, seq_unique, Arrival, MemoryTransport, SENTINEL};
use dissent_media::{
    elect, ranked, Candidate, FailoverReason, MediaKey, Mode, Participant, PeerId, Sfu,
    SILENCE_TIMEOUT,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::sleep;

const PARTICIPANTS: usize = 20;
const POOL: usize = 4;
const SPEAKERS: [PeerId; 3] = [1, 2, 3];
const SPEAKER_LEVEL: u8 = 10;
const LISTENER_LEVEL: u8 = 100;

fn level_of(sender: PeerId) -> u8 {
    if SPEAKERS.contains(&sender) {
        SPEAKER_LEVEL
    } else {
        LISTENER_LEVEL
    }
}
/// How long the call runs, from the moment everyone is connected.
const RUN_FOR: Duration = Duration::from_secs(6);
/// How long the call runs before the primary is lost.
const FAULT_AT: Duration = Duration::from_secs(2);
/// The exit criterion from the architecture spec.
const RECOVERY_LIMIT: Duration = Duration::from_secs(2);

/// Upload and wiring for the 20 participants. Peers 1 to 4 are wired with the most upload, so
/// the election puts them in the pool.
fn candidates() -> Vec<Candidate> {
    let wired_hosts = [
        (1u64, 30_000u32, 20u32),
        (2, 25_000, 25),
        (3, 20_000, 30),
        (4, 18_000, 35),
    ];
    let mut out: Vec<Candidate> = wired_hosts
        .iter()
        .map(|&(peer, upload_kbps, rtt_ms_median)| Candidate {
            peer,
            upload_kbps,
            on_battery: false,
            wired: true,
            cpu_ok: true,
            rtt_ms_median,
        })
        .collect();
    for peer in 5..=PARTICIPANTS as u64 {
        out.push(Candidate {
            peer,
            upload_kbps: 4_000 + peer as u32 * 500,
            on_battery: peer % 5 == 0,
            wired: peer % 2 == 0,
            cpu_ok: true,
            rtt_ms_median: 40 + peer as u32,
        });
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    /// The forwarder process dies. Its links close at once.
    Kill,
    /// The forwarder stops responding but keeps its links open.
    Freeze,
}

/// Holds a forwarder until `freeze` fires. A frozen forwarder keeps its links but processes
/// nothing.
async fn run_or_freeze(mut sfu: Sfu<MemoryTransport>, mut freeze: oneshot::Receiver<()>) {
    tokio::select! {
        _ = sfu.run() => {}
        _ = &mut freeze => {}
    }
    let _held = sfu;
    std::future::pending::<()>().await;
}

struct Outcome {
    fault_at: Instant,
    /// Per participant: the time from the fault until frames from every other speaker are back.
    recovery: Vec<(PeerId, Duration)>,
    /// Per participant: the longest gap in frames from any speaker that spans the fault.
    worst_gap: Vec<(PeerId, Duration)>,
    failovers: BTreeMap<PeerId, Vec<dissent_media::Failover>>,
    arrivals: BTreeMap<PeerId, Vec<Arrival>>,
    undecryptable: u64,
    intact: bool,
    /// Every frame's sequence number, timestamp and level were right, and went up per sender.
    stamped: bool,
    forwarder_observed: u64,
    plaintext_leaks: u64,
    backups_after: BTreeMap<PeerId, Option<PeerId>>,
    primary: PeerId,
    backup: PeerId,
    next_backup: PeerId,
}

async fn run_forwarded_call(fault: Fault) -> Outcome {
    let cands = candidates();
    let order = ranked(&cands, PARTICIPANTS, Mode::AudioTop3);
    let pool: Vec<PeerId> = order[..POOL].to_vec();
    let (primary, backup, next_backup) = (pool[0], pool[1], pool[2]);

    let key = MediaKey::new([3; 32]);
    let leaks = Arc::new(AtomicU64::new(0));
    let observed = Arc::new(AtomicU64::new(0));

    // Forwarders. They hold no key, and the observer is the only view of what they receive.
    let mut forwarders: BTreeMap<PeerId, Sfu<MemoryTransport>> = pool
        .iter()
        .map(|sfu| {
            let (leak_counter, seen_counter) = (leaks.clone(), observed.clone());
            let observer = Box::new(move |_from: PeerId, wire: &[u8]| {
                seen_counter.fetch_add(1, Ordering::Relaxed);
                if wire.windows(SENTINEL.len()).any(|w| w == SENTINEL) {
                    leak_counter.fetch_add(1, Ordering::Relaxed);
                }
            });
            (*sfu, Sfu::new(Mode::AudioTop3).with_observer(observer))
        })
        .collect();

    // Participants, each linked to every pool forwarder, so a backup is ready.
    let mut participants: Vec<Participant<MemoryTransport>> = (1..=PARTICIPANTS as u64)
        .map(|id| {
            Participant::new(
                key.clone(),
                id,
                cands.clone(),
                PARTICIPANTS,
                Mode::AudioTop3,
            )
            .unwrap()
        })
        .collect();
    for participant in participants.iter_mut() {
        for sfu in &pool {
            let (participant_end, forwarder_end) = memory_pair();
            let offer = participant.offer_sfu(*sfu, participant_end).await.unwrap();
            let answer = forwarders
                .get_mut(sfu)
                .unwrap()
                .accept(participant.id(), forwarder_end, offer)
                .await
                .unwrap();
            participant.apply_sfu(*sfu, answer).unwrap();
        }
        participant.elect().unwrap();
        assert_eq!(participant.active(), Some(primary));
        assert_eq!(participant.backup(), Some(backup));
    }

    // Start the forwarders, then the call.
    let mut freeze_senders = Vec::new();
    let mut primary_handle: Option<JoinHandle<()>> = None;
    let mut primary_freeze: Option<oneshot::Sender<()>> = None;
    for (sfu_id, sfu) in forwarders {
        let (freeze_tx, freeze_rx) = oneshot::channel();
        let handle = tokio::spawn(run_or_freeze(sfu, freeze_rx));
        if sfu_id == primary {
            primary_handle = Some(handle);
            primary_freeze = Some(freeze_tx);
        } else {
            // The sender stays alive, so these forwarders never see a freeze signal.
            freeze_senders.push(freeze_tx);
        }
    }

    let mut calls = Vec::new();
    for participant in participants {
        let id = participant.id();
        let level = if SPEAKERS.contains(&id) {
            SPEAKER_LEVEL
        } else {
            LISTENER_LEVEL
        };
        calls.push(tokio::spawn(run_participant(
            participant,
            id,
            level,
            RUN_FOR,
        )));
    }

    // Lose the primary forwarder partway through the call.
    sleep(FAULT_AT).await;
    let fault_at = Instant::now();
    match fault {
        Fault::Kill => primary_handle.as_ref().unwrap().abort(),
        Fault::Freeze => {
            let _ = primary_freeze.take().unwrap().send(());
        }
    }

    let mut results = Vec::new();
    for call in calls {
        results.push(call.await.unwrap());
    }
    drop(freeze_senders);

    let mut out = Outcome {
        fault_at,
        recovery: Vec::new(),
        worst_gap: Vec::new(),
        failovers: BTreeMap::new(),
        arrivals: BTreeMap::new(),
        undecryptable: 0,
        intact: true,
        stamped: true,
        forwarder_observed: observed.load(Ordering::Relaxed),
        plaintext_leaks: leaks.load(Ordering::Relaxed),
        backups_after: BTreeMap::new(),
        primary,
        backup,
        next_backup,
    };

    for (participant, arrivals) in results {
        let id = participant.id();
        out.undecryptable += participant
            .link_stats()
            .values()
            .map(|s| s.frames_undecryptable)
            .sum::<u64>();
        out.intact &= arrivals.iter().all(|a| a.intact);
        out.stamped &= arrivals
            .iter()
            .all(|a| a.stamp_ok && a.audio_level == level_of(a.sender));
        out.stamped &= seq_unique(&arrivals);
        out.failovers.insert(id, participant.failovers().to_vec());
        out.backups_after.insert(id, participant.backup());

        let others: Vec<PeerId> = SPEAKERS.iter().copied().filter(|s| *s != id).collect();
        let mut worst_recovery = Duration::ZERO;
        let mut worst_gap = Duration::ZERO;
        for speaker in &others {
            let from_speaker: Vec<Instant> = arrivals
                .iter()
                .filter(|a| a.sender == *speaker)
                .map(|a| a.at)
                .collect();
            let first_after = from_speaker
                .iter()
                .find(|at| **at >= fault_at)
                .unwrap_or_else(|| {
                    panic!("participant {id} never heard speaker {speaker} after the fault")
                });
            worst_recovery = worst_recovery.max(first_after.duration_since(fault_at));

            // The longest silence from this speaker that spans the fault.
            for pair in from_speaker.windows(2) {
                if pair[1] >= fault_at && pair[0] <= fault_at + RUN_FOR {
                    worst_gap = worst_gap.max(pair[1].duration_since(pair[0]));
                }
            }
        }
        out.recovery.push((id, worst_recovery));
        out.worst_gap.push((id, worst_gap));

        // Only the top speakers are ever forwarded, so nothing else may arrive.
        let senders: Vec<PeerId> = arrivals.iter().map(|a| a.sender).collect();
        assert!(
            senders.iter().all(|s| SPEAKERS.contains(s) && *s != id),
            "participant {id} received a frame from a non-speaker or from itself: {:?}",
            senders
                .iter()
                .filter(|s| !SPEAKERS.contains(s) || **s == id)
                .collect::<Vec<_>>()
        );
        out.arrivals.insert(id, arrivals);
    }
    out
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort();
    values[values.len() / 2]
}

fn report(label: &str, out: &Outcome) {
    let mut recovery: Vec<Duration> = out.recovery.iter().map(|(_, d)| *d).collect();
    let max = *recovery.iter().max().unwrap();
    let med = median(&mut recovery);
    let worst_gap = out.worst_gap.iter().map(|(_, d)| *d).max().unwrap();
    println!(
        "{label}: recovery max {:.1} ms, median {:.1} ms; longest gap {:.1} ms; \
         forwarder frames seen {}, plaintext leaks {}, undecryptable {}",
        max.as_secs_f64() * 1e3,
        med.as_secs_f64() * 1e3,
        worst_gap.as_secs_f64() * 1e3,
        out.forwarder_observed,
        out.plaintext_leaks,
        out.undecryptable,
    );
    let _ = out.fault_at;
}

fn assert_failed_over(out: &Outcome, reasons: &[FailoverReason]) {
    for (id, switches) in &out.failovers {
        assert_eq!(
            switches.len(),
            1,
            "participant {id} switched {} times",
            switches.len()
        );
        let switch = switches[0];
        assert_eq!(
            switch.from, out.primary,
            "participant {id} left the wrong forwarder"
        );
        assert_eq!(
            switch.to,
            Some(out.backup),
            "participant {id} did not promote the backup"
        );
        assert!(
            reasons.contains(&switch.reason),
            "participant {id}: reason {:?}",
            switch.reason
        );
        assert_eq!(
            out.backups_after[id],
            Some(out.next_backup),
            "participant {id} did not elect the next backup"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_person_call_recovers_within_two_seconds_when_the_forwarder_is_killed() {
    let out = run_forwarded_call(Fault::Kill).await;
    report("kill", &out);

    assert!(out.intact, "a frame arrived altered");
    assert!(
        out.stamped,
        "a frame arrived with the wrong sequence number, timestamp or level"
    );
    assert_eq!(out.undecryptable, 0, "a frame failed to decrypt");
    assert_eq!(out.plaintext_leaks, 0, "a forwarder saw plaintext");
    assert!(
        out.forwarder_observed > 0,
        "the forwarder saw no frames, so the leak check is void"
    );
    assert_failed_over(
        &out,
        &[FailoverReason::LinkError, FailoverReason::SendFailed],
    );

    let worst = out.recovery.iter().map(|(_, d)| *d).max().unwrap();
    if !cfg!(debug_assertions) {
        assert!(worst < RECOVERY_LIMIT, "recovery took {worst:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_person_call_recovers_within_two_seconds_when_the_forwarder_freezes() {
    let out = run_forwarded_call(Fault::Freeze).await;
    report("freeze", &out);

    assert!(out.intact, "a frame arrived altered");
    assert!(
        out.stamped,
        "a frame arrived with the wrong sequence number, timestamp or level"
    );
    assert_eq!(out.undecryptable, 0, "a frame failed to decrypt");
    assert_eq!(out.plaintext_leaks, 0, "a forwarder saw plaintext");
    assert_failed_over(&out, &[FailoverReason::Silence]);

    let worst = out.recovery.iter().map(|(_, d)| *d).max().unwrap();
    if !cfg!(debug_assertions) {
        assert!(worst < RECOVERY_LIMIT, "recovery took {worst:?}");
    }
    // Silence is the only signal, so the wait is at least the silence timeout.
    assert!(
        worst >= SILENCE_TIMEOUT,
        "recovery {worst:?} is faster than the silence timeout"
    );
}

#[test]
fn the_pool_is_the_top_of_the_election() {
    let cands = candidates();
    let e = elect(&cands, PARTICIPANTS, Mode::AudioTop3).unwrap();
    assert_eq!(e.primary, 1);
    assert_eq!(e.backup, Some(2));
}
