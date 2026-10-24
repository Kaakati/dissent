//! Sequence numbers and sample timestamps through a mesh, across a rekey. Forwarder, cascade and
//! failover runs check every arrival's stamp in their own test files. Links are in-memory, so no
//! sockets are involved.

mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use common::{memory_pair, MemoryTransport};
use dissent_media::{MediaKey, Mesh, ReceivedFrame, FRAME_SAMPLES};
use tokio::sync::Barrier;
use tokio::time::{interval, timeout};

type Peer = Mesh<MemoryTransport>;

const SENDER: u64 = 1;
/// Frames sent before the rekey. They carry explicit sample timestamps.
const BEFORE_REKEY: u32 = 20;
/// Frames sent after the rekey. They carry timestamps derived from the frame count.
const AFTER_REKEY: u32 = 20;
/// The sender's explicit sample clock at its first frame, so the clock does not start at zero.
const CLOCK_START: u32 = 5_000_000;
/// The level `Mesh::send_audio_frame` sets: RFC 6464 silence.
const LEVEL: u8 = 127;

fn key(byte: u8) -> MediaKey {
    MediaKey::new([byte; 32])
}

fn payload(index: u32) -> Vec<u8> {
    let mut frame = vec![0u8; 160];
    frame[..4].copy_from_slice(&index.to_le_bytes());
    frame
}

/// Connects every pair of `ids` with an offer and an answer over in-memory links.
async fn full_mesh(ids: &[u64], media_key: MediaKey) -> Vec<Peer> {
    let mut meshes: Vec<Peer> = ids
        .iter()
        .map(|id| Mesh::new(media_key.clone(), *id).unwrap())
        .collect();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            let (caller_end, callee_end) = memory_pair();
            let (lower, upper) = meshes.split_at_mut(j);
            let offer = lower[i].offer(ids[j], caller_end).await.unwrap();
            let answer = upper[0].accept(ids[i], callee_end, offer).await.unwrap();
            lower[i].apply(ids[j], answer).unwrap();
        }
    }
    meshes
}

/// Waits for `fut` while keeping the mesh pumped. Queued packets leave only while the mesh is
/// polled, so a peer that is waiting must still drive its links. Frames from the sender that
/// arrive meanwhile go into `got`.
async fn until<F: Future>(mesh: &mut Peer, got: &mut Vec<ReceivedFrame>, fut: F) -> F::Output {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return out,
            frame = mesh.next_received_frame() => {
                if let Ok(Some(frame)) = frame {
                    if frame.sender == SENDER {
                        got.push(frame);
                    }
                }
            }
        }
    }
}

/// Sends the sender's frames: explicit timestamps before the rekey, derived ones after.
async fn send_side(mut mesh: Peer, barrier: Arc<Barrier>) -> Peer {
    mesh.wait_connected().await.unwrap();
    let mut scratch = Vec::new();
    let mut ticker = interval(Duration::from_millis(20));
    for index in 0..BEFORE_REKEY {
        until(&mut mesh, &mut scratch, ticker.tick()).await;
        let clock = CLOCK_START + index * FRAME_SAMPLES;
        mesh.send_audio_frame_at(&payload(index), clock).await.unwrap();
    }
    until(&mut mesh, &mut scratch, barrier.wait()).await;
    mesh.set_key(key(2), 1).unwrap();
    until(&mut mesh, &mut scratch, barrier.wait()).await;
    for index in BEFORE_REKEY..BEFORE_REKEY + AFTER_REKEY {
        until(&mut mesh, &mut scratch, ticker.tick()).await;
        mesh.send_audio_frame(&payload(index)).await.unwrap();
    }
    // Keep the links pumped until the receivers have the last frames.
    let mut tail = Vec::new();
    until(&mut mesh, &mut tail, tokio::time::sleep(Duration::from_millis(500))).await;
    mesh
}

/// Receives the sender's frames, rekeying at the same point the sender does.
async fn receive_side(mut mesh: Peer, barrier: Arc<Barrier>) -> (Peer, Vec<ReceivedFrame>) {
    mesh.wait_connected().await.unwrap();
    let mut got = Vec::new();
    collect(&mut mesh, &mut got, BEFORE_REKEY as usize).await;
    until(&mut mesh, &mut got, barrier.wait()).await;
    mesh.set_key(key(2), 1).unwrap();
    until(&mut mesh, &mut got, barrier.wait()).await;
    collect(&mut mesh, &mut got, (BEFORE_REKEY + AFTER_REKEY) as usize).await;
    (mesh, got)
}

async fn collect(mesh: &mut Peer, got: &mut Vec<ReceivedFrame>, total: usize) {
    while got.len() < total {
        let frame = timeout(Duration::from_secs(10), mesh.next_received_frame())
            .await
            .expect("no frame within ten seconds")
            .unwrap()
            .expect("the mesh emptied before the frames arrived");
        if frame.sender == SENDER {
            got.push(frame);
        }
    }
}

fn assert_stream(got: &[ReceivedFrame]) {
    assert_eq!(got.len(), (BEFORE_REKEY + AFTER_REKEY) as usize);
    for (index, frame) in got.iter().enumerate() {
        let index = index as u32;
        assert_eq!(frame.sender, SENDER);
        assert_eq!(frame.seq, index, "sequence numbers count on through the rekey");
        let clock = if index < BEFORE_REKEY {
            CLOCK_START + index * FRAME_SAMPLES
        } else {
            index * FRAME_SAMPLES
        };
        assert_eq!(frame.timestamp, clock, "frame {index} has the wrong timestamp");
        assert_eq!(frame.audio_level, LEVEL);
        assert_eq!(frame.payload, payload(index));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn sequence_numbers_and_timestamps_survive_a_mesh_rekey() {
    timeout(Duration::from_secs(60), run_rekey_test())
        .await
        .expect("the mesh rekey run hung");
}

async fn run_rekey_test() {
    let mut meshes = full_mesh(&[SENDER, 2, 3], key(1)).await.into_iter();
    let barrier = Arc::new(Barrier::new(3));
    let sender = tokio::spawn(send_side(meshes.next().unwrap(), barrier.clone()));
    let first = tokio::spawn(receive_side(meshes.next().unwrap(), barrier.clone()));
    let second = tokio::spawn(receive_side(meshes.next().unwrap(), barrier.clone()));

    let sender_mesh = sender.await.unwrap();
    for (peer, stats) in sender_mesh.stats() {
        assert!(stats.target_kbps > 0, "the link to {peer} reports no bitrate target");
    }
    let (_, got_first) = first.await.unwrap();
    let (_, got_second) = second.await.unwrap();
    assert_stream(&got_first);
    assert_stream(&got_second);
}
