//! A member impersonates another member. The forger runs a mesh with the victim's sender id, but
//! signs its digests with its own device key. The receiver must flag and mute the forgery within
//! 500 ms, while the real sender keeps playing.

mod common;

use std::time::{Duration, Instant};

use common::{auth_set, frame, memory_pair, run_mesh_peer, MemoryTransport};
use dissent_media::{Auth, MediaKey, Mesh};
use tokio::time::timeout;

const VICTIM: u64 = 2;
const RECEIVER: u64 = 3;
const FORGER: u64 = 1;
const FORGED_MARK: u8 = b'F';

/// A payload that cannot be mistaken for the victim's sentinel frames.
fn forged_frame() -> Vec<u8> {
    vec![FORGED_MARK; 160]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forged_sender_id_is_flagged_and_muted_while_the_real_sender_plays() {
    let media_key = MediaKey::new([9; 32]);
    let members = auth_set(&[VICTIM, RECEIVER, FORGER]);
    // The forger signs with its own device key, but claims the victim's sender id.
    let forger_auth: Auth = members[&FORGER].clone();

    let mut victim: Mesh<MemoryTransport> =
        Mesh::new_authenticated(media_key.clone(), VICTIM, members[&VICTIM].clone()).unwrap();
    let mut receiver: Mesh<MemoryTransport> =
        Mesh::new_authenticated(media_key.clone(), RECEIVER, members[&RECEIVER].clone()).unwrap();
    let mut forger: Mesh<MemoryTransport> =
        Mesh::new_authenticated(media_key.clone(), VICTIM, forger_auth).unwrap();

    // The real link from the victim to the receiver.
    let (victim_end, receiver_end_victim) = memory_pair();
    let offer = victim.offer(RECEIVER, victim_end).await.unwrap();
    let answer = receiver
        .accept(VICTIM, receiver_end_victim, offer)
        .await
        .unwrap();
    victim.apply(RECEIVER, answer).unwrap();

    // The forged link: the forger offers as its id 2 and the receiver files it under key 1.
    let (forger_end, receiver_end_forger) = memory_pair();
    let offer = forger.offer(RECEIVER, forger_end).await.unwrap();
    let answer = receiver
        .accept(FORGER, receiver_end_forger, offer)
        .await
        .unwrap();
    forger.apply(RECEIVER, answer).unwrap();

    let real_frames = 50u64;
    let forged_frames = 60u64;
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<Instant>();

    let victim_task = tokio::spawn(async move {
        let (victim, _) = run_mesh_peer(victim, real_frames, 0).await;
        victim
    });

    // The forger sends forged frames at the victim's cadence.
    let forger_task = tokio::spawn(async move {
        forger.wait_connected().await.unwrap();
        let _ = start_tx.send(Instant::now());
        // Like an honest sender, the forger keeps reading its link while it sends. str0m
        // only moves queued packets when it is driven.
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        let mut sent = 0u64;
        while sent < forged_frames {
            tokio::select! {
                _ = ticker.tick() => {
                    forger.send_audio_frame(&forged_frame()).await.unwrap();
                    sent += 1;
                }
                _ = forger.next_frame() => {}
            }
        }
        let _ = timeout(Duration::from_millis(300), async {
            while let Ok(Some(_)) = forger.next_frame().await {}
        })
        .await;
        forger
    });

    // The receiver plays everything, and polls its stats so it can time the detection.
    receiver.wait_connected().await.unwrap();
    let mut delivered: Vec<(Instant, Vec<u8>, u64)> = Vec::new();
    let mut detected_at: Option<Instant> = None;
    let run = Duration::from_millis(2500);
    let end = tokio::time::Instant::now() + run;
    while tokio::time::Instant::now() < end {
        match timeout(Duration::from_millis(20), receiver.next_frame()).await {
            Ok(Ok(Some((sender, payload)))) => {
                delivered.push((Instant::now(), payload, sender));
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("receiver failed: {e}"),
            Err(_) => {}
        }
        if detected_at.is_none() && impersonations(&receiver) > 0 {
            detected_at = Some(Instant::now());
        }
    }

    let victim = victim_task.await.unwrap();
    let _forger = forger_task.await.unwrap();
    drop(victim);

    // Detection happens within 500 ms of the forged traffic starting.
    let forged_start = start_rx.await.expect("the forger never started");
    let detected = detected_at.expect("the forgery was never flagged");
    let latency = detected.duration_since(forged_start);
    assert!(
        latency <= Duration::from_millis(500),
        "detected {latency:?} after the forged traffic started, which is more than 500 ms"
    );

    // The real sender keeps playing: every one of its frames arrives, intact.
    let real: Vec<&(Instant, Vec<u8>, u64)> = delivered
        .iter()
        .filter(|(_, payload, _)| payload.first() != Some(&FORGED_MARK))
        .collect();
    let mut seqs: Vec<u64> = real
        .iter()
        .map(|(_, payload, sender)| {
            assert_eq!(*sender, VICTIM);
            let seq = u64::from_le_bytes(payload[..8].try_into().unwrap());
            assert_eq!(payload, &frame(seq), "a real frame arrived altered");
            seq
        })
        .collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(
        seqs,
        (0..real_frames).collect::<Vec<_>>(),
        "the real sender lost frames"
    );

    // After detection, the forgery plays no more.
    let forged_after_detection = delivered
        .iter()
        .filter(|(at, payload, _)| payload.first() == Some(&FORGED_MARK) && *at > detected)
        .count();
    assert_eq!(
        forged_after_detection, 0,
        "forged audio played after the sender was muted"
    );

    let forged_before_detection = delivered
        .iter()
        .filter(|(at, payload, _)| payload.first() == Some(&FORGED_MARK) && *at <= detected)
        .count();
    println!(
        "forgery: flagged after {:.0} ms; real frames played {}; forged frames played before the flag {}",
        latency.as_secs_f64() * 1e3,
        real.len(),
        forged_before_detection,
    );
    assert!(impersonations(&receiver) >= 1);
}

/// The impersonations detected on every link of the receiver.
fn impersonations(mesh: &Mesh<MemoryTransport>) -> u64 {
    mesh.stats()
        .values()
        .map(|s| s.impersonation_detected)
        .sum()
}
