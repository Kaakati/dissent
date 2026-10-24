//! A forwarded call in which one member is removed by a rekey. The remaining members rekey and
//! keep talking. The removed member keeps the old key, and it decrypts nothing after the rekey.
//! Forwarders hold no key and relay the ciphertext unchanged. Links are in-memory.

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use common::{memory_pair, MemoryTransport};
use dissent_media::{
    ranked, Candidate, CallStats, MediaKey, Mode, Participant, PeerId, ReceivedFrame, Sfu,
};
use tokio::sync::Barrier;
use tokio::time::{interval, sleep, timeout};

const MEMBERS: u64 = 8;
const BANNED: PeerId = 8;
/// A forwarder relays only the top three speakers, so only these members talk.
const SPEAKERS: [PeerId; 3] = [2, 3, 4];
const POOL: usize = 2;
const FRAMES_BEFORE: u32 = 10;
const FRAMES_AFTER: u32 = 25;
/// Long enough for the grace window to close, with a margin.
const LISTEN_AFTER: Duration = Duration::from_millis(3_000);
const LEVEL: u8 = 40;

fn old_key() -> MediaKey {
    MediaKey::new([5u8; 32])
}

fn new_key() -> MediaKey {
    MediaKey::new([6u8; 32])
}

/// The frame's index, then the phase it was sent in: 0 under the old key, 1 under the new one.
fn payload(index: u32, phase: u8) -> Vec<u8> {
    let mut frame = vec![0u8; 160];
    frame[..4].copy_from_slice(&index.to_le_bytes());
    frame[4] = phase;
    frame
}

/// The phase a received frame was sent in.
fn phase_of(frame: &ReceivedFrame) -> u8 {
    frame.payload[4]
}

fn candidates() -> Vec<Candidate> {
    (1..=MEMBERS)
        .map(|peer| Candidate {
            peer,
            upload_kbps: if peer <= POOL as u64 { 30_000 } else { 4_000 },
            on_battery: false,
            wired: peer <= POOL as u64,
            cpu_ok: true,
            rtt_ms_median: 20 + peer as u32,
        })
        .collect()
}

/// Waits for `fut` while the participant keeps its links pumped. Frames that arrive meanwhile go
/// into `got`. Queued packets leave only while the links are polled.
async fn until<F: Future>(
    participant: &mut Participant<MemoryTransport>,
    got: &mut Vec<ReceivedFrame>,
    fut: F,
) -> F::Output {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return out,
            frame = participant.next_received_frame() => {
                if let Ok(Some(frame)) = frame {
                    got.push(frame);
                }
            }
        }
    }
}

struct Member {
    participant: Participant<MemoryTransport>,
    before: Vec<ReceivedFrame>,
    after: Vec<ReceivedFrame>,
}

async fn run_member(mut participant: Participant<MemoryTransport>, barrier: Arc<Barrier>) -> Member {
    let id = participant.id();
    let banned = id == BANNED;
    let speaks = SPEAKERS.contains(&id);
    participant.wait_connected().await.unwrap();
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut ticker = interval(Duration::from_millis(20));

    for index in 0..FRAMES_BEFORE {
        until(&mut participant, &mut before, ticker.tick()).await;
        if speaks {
            participant
                .send_audio_frame(&payload(index, 0), LEVEL)
                .await
                .unwrap();
        }
    }
    until(&mut participant, &mut before, barrier.wait()).await;

    // Everyone but the banned member rekeys. The banned member has no way to learn the new key.
    if !banned {
        participant.set_key(new_key(), 1).unwrap();
    }
    until(&mut participant, &mut after, barrier.wait()).await;

    for index in 0..FRAMES_AFTER {
        until(&mut participant, &mut after, ticker.tick()).await;
        if speaks && !banned {
            participant
                .send_audio_frame(&payload(index, 1), LEVEL)
                .await
                .unwrap();
        }
    }
    until(&mut participant, &mut after, sleep(LISTEN_AFTER)).await;
    Member {
        participant,
        before,
        after,
    }
}

fn undecryptable(stats: &BTreeMap<PeerId, CallStats>) -> u64 {
    stats.values().map(|s| s.frames_undecryptable).sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rekey_removes_a_member_from_a_forwarded_call() {
    timeout(Duration::from_secs(90), run_rekey())
        .await
        .expect("the rekey run hung");
}

async fn run_rekey() {
    let cands = candidates();
    let order = ranked(&cands, MEMBERS as usize, Mode::AudioTop3);
    let pool: Vec<PeerId> = order[..POOL].to_vec();

    let mut forwarders: BTreeMap<PeerId, Sfu<MemoryTransport>> = pool
        .iter()
        .map(|id| (*id, Sfu::new(Mode::AudioTop3)))
        .collect();
    let mut participants: Vec<Participant<MemoryTransport>> = (1..=MEMBERS)
        .map(|id| {
            Participant::new(old_key(), id, cands.clone(), MEMBERS as usize, Mode::AudioTop3)
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
    }

    let forwarder_tasks: Vec<_> = forwarders
        .into_values()
        .map(|mut sfu| tokio::spawn(async move { let _ = sfu.run().await; }))
        .collect();

    let barrier = Arc::new(Barrier::new(MEMBERS as usize));
    let tasks: Vec<_> = participants
        .into_iter()
        .map(|p| tokio::spawn(run_member(p, barrier.clone())))
        .collect();
    let mut members = Vec::new();
    for task in tasks {
        members.push(task.await.unwrap());
    }
    for task in forwarder_tasks {
        task.abort();
    }

    for member in &members {
        let id = member.participant.id();
        if id == BANNED {
            // The removed member heard the call before the rekey, and nothing after it.
            assert!(!member.before.is_empty(), "the removed member never heard the call");
            let new_key_frames = member.after.iter().filter(|f| phase_of(f) == 1).count();
            assert_eq!(
                new_key_frames, 0,
                "the removed member decrypted {new_key_frames} frames sent under the new key"
            );
            println!(
                "removed member: {} frames from before the rekey arrived after it",
                member.after.len()
            );
            // The new-key frames reach the removed member's links and fail to decrypt. Failing
            // frames do not count as activity, so it treats the forwarder as silent and fails over.
            assert!(
                !member.participant.failovers().is_empty(),
                "the removed member kept a live link to the call after the rekey"
            );
        } else {
            assert_eq!(
                undecryptable(&member.participant.link_stats()),
                0,
                "member {id} failed to decrypt a frame"
            );
            for other in SPEAKERS.iter().copied().filter(|o| *o != id) {
                let counts: BTreeMap<u64, usize> = (1..=MEMBERS)
                    .map(|s| (s, member.after.iter().filter(|f| f.sender == s).count()))
                    .collect();
                assert!(
                    member.after.iter().any(|f| f.sender == other),
                    "member {id} heard nothing from member {other} after the rekey: {counts:?},                      failovers {:?}, undecryptable {}",
                    member.participant.failovers(),
                    undecryptable(&member.participant.link_stats())
                );
            }
        }
    }
}
