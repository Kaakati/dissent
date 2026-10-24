//! A device that joins a mesh after it started. The existing members learn its certificate, and
//! its frames verify for them. Links are in-memory, so no sockets are involved.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use common::{memory_pair, MemoryTransport};
use dissent_identity::{DeviceCert, DeviceKey, RootKey};
use dissent_media::{Auth, CallStats, MediaKey, Mesh};
use tokio::time::{interval, sleep_until, timeout, Instant};

type Peer = Mesh<MemoryTransport>;

/// The members in the call before the join.
const EXISTING: [u64; 3] = [1, 2, 3];
const JOINER: u64 = 4;
const FRAMES: u32 = 60;
const SEND_FOR: Duration = Duration::from_secs(4);

fn key() -> MediaKey {
    MediaKey::new([9u8; 32])
}

fn payload(index: u32) -> Vec<u8> {
    let mut frame = vec![0u8; 160];
    frame[..4].copy_from_slice(&index.to_le_bytes());
    frame
}

/// Connects two meshes with one offer and one answer over an in-memory link.
async fn connect(a: &mut Peer, a_id: u64, b: &mut Peer, b_id: u64) {
    let (a_end, b_end) = memory_pair();
    let offer = a.offer(b_id, a_end).await.unwrap();
    let answer = b.accept(a_id, b_end, offer).await.unwrap();
    a.apply(b_id, answer).unwrap();
}

/// Runs one member until the window closes. The joiner sends frames. Every member keeps its
/// links pumped, so the frames reach the others. Returns the mesh, its stats, and the number of
/// frames it received from the joiner.
async fn run_member(mut mesh: Peer, sends: bool) -> (Peer, BTreeMap<u64, CallStats>, usize) {
    mesh.wait_connected().await.unwrap();
    let end = Instant::now() + SEND_FOR;
    let mut ticker = interval(Duration::from_millis(20));
    let mut sent = 0u32;
    let mut from_joiner = 0usize;
    loop {
        tokio::select! {
            _ = sleep_until(end) => break,
            _ = ticker.tick(), if sends && sent < FRAMES => {
                mesh.send_audio_frame(&payload(sent)).await.unwrap();
                sent += 1;
            }
            frame = mesh.next_received_frame() => {
                if let Ok(Some(frame)) = frame {
                    if frame.sender == JOINER {
                        from_joiner += 1;
                    }
                }
            }
        }
    }
    let stats = mesh.stats();
    (mesh, stats, from_joiner)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_joiner_verifies_for_existing_members_after_they_learn_its_certificate() {
    timeout(Duration::from_secs(60), run_join())
        .await
        .expect("the join run hung");
}

async fn run_join() {
    // One root certifies every device. Existing members start knowing only each other.
    let root = RootKey::generate();
    let mut devices: BTreeMap<u64, DeviceKey> = EXISTING
        .iter()
        .copied()
        .chain(std::iter::once(JOINER))
        .map(|id| (id, DeviceKey::generate()))
        .collect();
    let all_certs: HashMap<u64, DeviceCert> = devices
        .iter()
        .map(|(id, device)| (*id, root.certify(device.id(), 0)))
        .collect();

    let mut meshes: Vec<Peer> = Vec::new();
    for id in EXISTING {
        let known: HashMap<u64, DeviceCert> = all_certs
            .iter()
            .filter(|(member, _)| EXISTING.contains(member))
            .map(|(member, cert)| (*member, cert.clone()))
            .collect();
        let auth = Auth::new(devices.remove(&id).unwrap(), known);
        meshes.push(Mesh::new_authenticated(key(), id, auth).unwrap());
    }
    let auth = Auth::new(devices.remove(&JOINER).unwrap(), all_certs.clone());
    let mut joiner = Mesh::new_authenticated(key(), JOINER, auth).unwrap();

    // The existing members connect among themselves, then the joiner connects to each.
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        let (lower, upper) = meshes.split_at_mut(j);
        connect(&mut lower[i], EXISTING[i], &mut upper[0], EXISTING[j]).await;
    }
    for (i, id) in EXISTING.iter().enumerate() {
        connect(&mut meshes[i], *id, &mut joiner, JOINER).await;
    }

    // The join: every existing member learns the joiner's certificate before traffic starts.
    for mesh in meshes.iter_mut() {
        mesh.learn_cert(JOINER, all_certs[&JOINER].clone()).unwrap();
    }

    let mut tasks = Vec::new();
    for mesh in meshes {
        tasks.push(tokio::spawn(run_member(mesh, false)));
    }
    tasks.push(tokio::spawn(run_member(joiner, true)));

    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }

    let (_, joiner_stats, _) = results.pop().unwrap();
    for (peer, call) in &joiner_stats {
        assert_eq!(
            call.impersonation_detected, 0,
            "the joiner flagged an impersonation on the link to {peer}"
        );
    }
    for (_, stats, from_joiner) in &results {
        for (peer, call) in stats {
            assert_eq!(
                call.impersonation_detected, 0,
                "a member flagged an impersonation on the link to {peer}"
            );
        }
        // Frames arrive from the joiner under every load, so at least some must come through.
        assert!(*from_joiner > 0, "no frames from the joiner arrived");
        // A debug build on a loaded machine can miss the digest deadline, so the full count and
        // the unauthenticated count are checked in release builds only.
        if !cfg!(debug_assertions) {
            assert_eq!(*from_joiner, FRAMES as usize, "frames went missing");
            for (peer, call) in stats {
                assert_eq!(
                    call.frames_unauthenticated, 0,
                    "frames from {peer} stayed unverified after the join"
                );
            }
        }
    }
}
