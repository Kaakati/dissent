#![cfg(not(target_os = "linux"))]

//! Video in a call, end to end: one engine turns its camera or its screen on, and the other engine
//! receives decoded previews of it. The camera is a deterministic test picture, since CI has no
//! camera. Networked, so run with `scripts/fwtest.sh dissent-core it --ignored video::`.

use crate::common::*;
use dissent_core::EngineEvent;
use std::time::{Duration, Instant};

/// The two tests in this file start engines with the same names, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Counts the previews of `root_id`'s camera (or screen, when `screen`) that arrive within `span`,
/// and returns how many there were and how many bytes of JPEG they carried.
async fn previews_for(
    rx: &mut tokio::sync::broadcast::Receiver<EngineEvent>,
    root_id: &str,
    screen: bool,
    span: Duration,
) -> (u32, usize) {
    let deadline = Instant::now() + span;
    let mut count = 0;
    let mut bytes = 0;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(EngineEvent::VideoFrame {
                root_id: from,
                screen: is_screen,
                jpeg,
                ..
            })) if from == root_id && is_screen == screen => {
                count += 1;
                bytes += jpeg.len();
            }
            Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            _ => break,
        }
    }
    (count, bytes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
#[cfg(not(target_os = "linux"))]
async fn a_camera_in_a_dm_call_reaches_the_other_engine_as_jpeg_previews() {
    let _serial = SERIAL.lock().await;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;

        let mut bob_events = bob.engine.subscribe();
        alice
            .engine
            .call_dm(bob.identity.root_id())
            .await
            .expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up on both sides", || {
            alice
                .engine
                .call_state()
                .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;

        // Alice's test picture goes out, and Bob reads previews of it. The picture is flat colour
        // bars with noise, so each preview is a JPEG of a few kilobytes.
        alice.engine.use_test_camera_pattern(true);
        alice.engine.set_camera(true).expect("camera on");
        let alice_root = alice.identity.root_id().to_string();
        let (count, bytes) =
            previews_for(&mut bob_events, &alice_root, false, Duration::from_secs(4)).await;
        assert!(count >= 20, "Bob reads Alice's camera at about 15 fps: {count} previews");
        let per_second = bytes / 4;
        eprintln!(
            "camera previews: {count} in 4 s, {} KB of JPEG, {} KB/s per remote camera",
            bytes / 1024,
            per_second / 1024
        );

        alice.engine.set_camera(false).expect("camera off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked, and needs a display: run with scripts/fwtest.sh"]
#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn a_shared_screen_in_a_dm_call_reaches_the_other_engine() {
    let _serial = SERIAL.lock().await;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;

        let mut bob_events = bob.engine.subscribe();
        alice
            .engine
            .call_dm(bob.identity.root_id())
            .await
            .expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up on both sides", || {
            alice
                .engine
                .call_state()
                .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;

        alice.engine.set_screen_share(true, None).expect("screen share on");
        let alice_root = alice.identity.root_id().to_string();
        let (count, _) =
            previews_for(&mut bob_events, &alice_root, true, Duration::from_secs(4)).await;
        assert!(count >= 5, "Bob reads Alice's screen at about 10 fps: {count} previews");

        alice.engine.set_screen_share(false, None).expect("screen share off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// AV1 in a call: with the `svt-av1` feature built, both sides agree to AV1, so the camera is
/// encoded with it. Bob's previews say so. Needs the feature: `cargo test -p dissent-core --features svt-av1`.
#[cfg(feature = "svt-av1")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_camera_in_a_dm_call_is_encoded_with_av1_when_both_sides_agree() {
    let _serial = SERIAL.lock().await;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;

        let mut bob_events = bob.engine.subscribe();
        alice
            .engine
            .call_dm(bob.identity.root_id())
            .await
            .expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up on both sides", || {
            alice
                .engine
                .call_state()
                .filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;

        alice.engine.use_test_camera_pattern(true);
        alice.engine.set_camera(true).expect("camera on");
        let alice_root = alice.identity.root_id().to_string();
        let mut av1 = 0u32;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && av1 < 10 {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, bob_events.recv()).await {
                Ok(Ok(EngineEvent::VideoFrame { root_id, screen: false, av1: true, .. })) if root_id == alice_root => {
                    av1 += 1;
                }
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                _ => break,
            }
        }
        assert!(av1 >= 10, "Bob reads AV1 previews of Alice's camera: {av1}");

        alice.engine.set_camera(false).expect("camera off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

