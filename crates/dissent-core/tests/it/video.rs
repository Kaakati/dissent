
//! Video in a call, end to end: one engine turns its camera or its screen on, and the other engine
//! receives decoded previews of it. The camera is a deterministic test picture, since CI has no
//! camera. Networked, so run with `scripts/fwtest.sh dissent-core it --ignored video::`.

use crate::common::*;
use dissent_core::{EncodedVideo, EngineEvent, TestCapture};
use std::sync::Arc;
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
        alice.engine.set_test_capture(TestCapture::Pattern);
        let before = bob.engine.call_state().map_or(0, |v| v.frames_received);
        alice.engine.set_camera(true).expect("camera on");
        let alice_root = alice.identity.root_id().to_string();
        let (count, bytes) =
            previews_for(&mut bob_events, &alice_root, false, Duration::from_secs(4)).await;
        assert!(count >= 20, "Bob reads Alice's camera at about 15 fps: {count} previews");
        // Every video frame Bob takes in counts, not only the ones he draws as previews.
        let received = bob.engine.call_state().map_or(0, |v| v.frames_received);
        assert!(received >= before + count as u64, "frames_received counts the video: {received} after {before} + {count} previews");
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
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_encoded_sink_gets_the_camera_frames_undecoded_and_no_previews() {
    let _serial = SERIAL.lock().await;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;

        // Bob's window decodes the frames itself, so he takes them encoded.
        let (tx, rx) = std::sync::mpsc::channel::<EncodedVideo>();
        bob.engine.set_encoded_video_sink(Some(Arc::new(move |frame| {
            let _ = tx.send(frame);
            true
        })));

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

        alice.engine.set_test_capture(TestCapture::Pattern);
        alice.engine.set_camera(true).expect("camera on");
        tokio::time::sleep(Duration::from_secs(3)).await;

        let alice_root = alice.identity.root_id().to_string();
        let frames: Vec<EncodedVideo> = rx.try_iter().filter(|f| f.root_id == alice_root).collect();
        assert!(frames.len() >= 15, "Bob's window gets about 15 frames of Alice's camera a second: {}", frames.len());
        assert!(frames.iter().any(|f| f.keyframe), "a keyframe arrives first, so a decoder can start");
        assert!(frames.iter().all(|f| !f.screen && !f.av1 && !f.payload.is_empty()), "H.264 camera frames, still encoded");
        let (previews, _) = previews_for(&mut bob_events, &alice_root, false, Duration::from_secs(1)).await;
        assert_eq!(previews, 0, "no JPEG previews while the window decodes the frames itself");

        alice.engine.set_camera(false).expect("camera off");
        bob.engine.set_encoded_video_sink(None);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked, and needs a display: run with scripts/fwtest.sh"]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
async fn a_shared_screen_in_a_dm_call_reaches_the_other_engine() {
    // CI's Linux runner has no display to capture.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("skipped: no display");
        return;
    }
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

        alice.engine.set_test_capture(TestCapture::Pattern);
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

        alice.engine.set_test_capture(TestCapture::Pattern);
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

/// What Bob received of a share over a span.
#[derive(Debug)]
struct Received {
    fps: f64,
    kbps: f64,
    largest_key: usize,
}

fn measure(frames: &[EncodedVideo], seconds: f64) -> Received {
    let bytes: usize = frames.iter().map(|f| f.payload.len()).sum();
    Received {
        fps: frames.len() as f64 / seconds,
        kbps: bytes as f64 * 8.0 / seconds / 1_000.0,
        largest_key: frames.iter().filter(|f| f.keyframe).map(|f| f.payload.len()).max().unwrap_or(0),
    }
}

/// Decodes `frame`, a keyframe, and returns the size of its picture.
fn keyframe_size(frame: &EncodedVideo) -> Option<(u32, u32)> {
    let codec = if frame.av1 { dissent_video::Codec::Av1 } else { dissent_video::Codec::H264 };
    let mut decoder = dissent_video::decode::open(codec).ok()?;
    let pictures = decoder.decode(&frame.payload, 0).ok()?;
    pictures.last().map(|p| (p.width(), p.height()))
}

/// A share of a screen of `size` in a DM call: Bob gets it at that size, with the whole stream, and the
/// bitrate slider moves it during the share.
async fn share_reaches_its_own_size(size: (u32, u32)) {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    make_contacts_ready(&alice, &bob).await;

    let (tx, rx) = std::sync::mpsc::channel::<EncodedVideo>();
    bob.engine.set_encoded_video_sink(Some(Arc::new(move |frame| {
        tx.send(frame).is_ok()
    })));
    let mut bob_events = bob.engine.subscribe();
    alice.engine.call_dm(bob.identity.root_id()).await.expect("call starts");
    accept_incoming(&bob.engine, &mut bob_events).await;
    eventually("the call is up on both sides", || {
        alice.engine.call_state().filter(|v| v.participants.len() == 1 && v.frames_received > 0)
    })
    .await;

    alice.engine.set_test_capture(TestCapture::Document { width: size.0, height: size.1 });
    assert_eq!(alice.engine.set_screen_bitrate(3_000), 3_000);
    alice.engine.set_screen_share(true, None).expect("screen share on");

    let started = Instant::now();
    let mut native_at = None;
    while started.elapsed() < Duration::from_secs(45) {
        let Ok(frame) = rx.recv_timeout(Duration::from_secs(1)) else {
            continue;
        };
        if frame.keyframe && keyframe_size(&frame) == Some(size) {
            native_at = Some(started.elapsed());
            println!("SHARE keyframe of {} bytes, AV1 {}", frame.payload.len(), frame.av1);
            break;
        }
    }
    let sharing = eventually("Bob sees that Alice shares", || {
        bob.engine.call_state().filter(|c| c.participants.iter().any(|p| p.screen))
    })
    .await;
    assert_eq!(sharing.participants.len(), 1);
    let native_at = native_at.unwrap_or_else(|| panic!("Bob never got a {size:?} keyframe"));
    println!("SHARE {size:?}: at its own size after {:.1} s", native_at.as_secs_f64());

    let span = |seconds: u64| {
        let until = Instant::now() + Duration::from_secs(seconds);
        let mut frames = Vec::new();
        while Instant::now() < until {
            frames.extend(rx.recv_timeout(Duration::from_millis(200)));
        }
        measure(&frames, seconds as f64)
    };
    let plan = || dissent_core::test_support::screen_plan(&alice.engine).expect("a share runs");
    // The share climbs off its start, on the rate the link is known to carry.
    span(10);
    let at_3000 = span(10);
    println!("SHARE {size:?} at 3000 kbps: {:?} {at_3000:?}", plan());
    assert_eq!((plan().0, plan().1), size, "the share stays at its own size");
    assert!(plan().3 <= 3_000, "and at its bitrate: {:?}", plan());
    assert!(at_3000.fps >= 3.0, "and keeps moving: {at_3000:?}");
    assert!(at_3000.largest_key < 2_000_000, "and a keyframe fits what str0m keeps to send: {at_3000:?}");
    assert!(at_3000.kbps <= 3_000.0 * 1.5, "and sends near its bitrate at most: {at_3000:?}");

    alice.engine.set_screen_bitrate(6_000);
    span(5);
    let at_6000 = span(10);
    println!("SHARE {size:?} at 6000 kbps: {:?} {at_6000:?}", plan());
    let target = || dissent_core::test_support::screen_target(&alice.engine);
    assert_eq!(target(), Some(6_000), "the slider raises the share's rate during a share");

    alice.engine.set_screen_bitrate(1_500);
    span(5);
    let at_1500 = span(10);
    println!("SHARE {size:?} at 1500 kbps: {:?} {at_1500:?}", plan());
    assert_eq!(target(), Some(1_500), "and lowers it");
    assert!(plan().3 <= 1_500, "and the encoder follows: {:?}", plan());
    assert_eq!((plan().0, plan().1), size, "without touching the size");
    assert!(at_1500.kbps <= 1_500.0 * 1.5, "the share stays near a lowered bitrate: {at_1500:?}");

    alice.engine.set_screen_share(false, None).expect("screen share off");
    eventually_within(Duration::from_secs(20), "Bob sees that the share ended", || {
        bob.engine.call_state().filter(|c| c.participants.iter().all(|p| !p.screen))
    })
    .await;
    bob.engine.set_encoded_video_sink(None);
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_1080p_screen_is_shared_at_1080p() {
    // SERIAL first, as the other tests here take it: they hold it while they wait for the run lock.
    let _serial = SERIAL.lock().await;
    // Encoding a large picture takes the CPU of many engines, so it does not run beside them.
    heavy(7, share_reaches_its_own_size((1920, 1080))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_1440p_screen_is_shared_at_1440p() {
    // SERIAL first, as the other tests here take it: they hold it while they wait for the run lock.
    let _serial = SERIAL.lock().await;
    // Encoding a large picture takes the CPU of many engines, so it does not run beside them.
    heavy(7, share_reaches_its_own_size((2560, 1440))).await;
}

/// The sound of a share: off unless turned on, encrypted on the same path as the picture, in Bob's playout at his
/// volume. The source is a test tone, so nothing captures the machine's sound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_share_can_carry_the_computers_sound_at_each_viewers_volume() {
    let _serial = SERIAL.lock().await;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;
        let mut bob_events = bob.engine.subscribe();
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up on both sides", || {
            alice.engine.call_state().filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;
        alice.engine.set_test_capture(TestCapture::Pattern);
        alice.engine.set_screen_share(true, None).expect("screen share on");
        let sound_on = |on: bool| bob.engine.call_state().is_some_and(|c| c.participants.iter().any(|p| p.screen_audio) == on);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(sound_on(false), "a share has no sound until the sharer adds it");

        alice.engine.set_screen_audio(true).expect("sound on");
        eventually("Bob's view says the share has sound", || sound_on(true).then_some(())).await;

        let energy = |engine: &dissent_core::Engine| {
            let mut most = 0f32;
            for _ in 0..50 {
                let mut out = vec![0f32; 960];
                engine.pull_playout(&mut out);
                most = most.max(out.iter().map(|s| s * s).sum::<f32>() / 960.0);
                std::thread::sleep(Duration::from_millis(20));
            }
            most
        };
        let heard = energy(&bob.engine);
        assert!(heard > 0.01, "Bob's playout has the tone: {heard}");
        bob.engine.set_screen_volume(&alice.identity.root_id().to_string(), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let muted = energy(&bob.engine);
        assert!(muted < heard / 100.0, "Bob's volume mutes it: {muted} from {heard}");
        assert!(alice.engine.call_state().is_some_and(|c| c.participants.iter().all(|p| !p.screen_audio)), "Alice does not get her own sound back");

        alice.engine.set_screen_audio(false).expect("sound off");
        eventually_within(Duration::from_secs(15), "the sound ends and the share goes on", || {
            bob.engine.call_state().filter(|c| c.participants.iter().all(|p| p.screen && !p.screen_audio))
        })
        .await;
        alice.engine.set_screen_share(false, None).expect("screen share off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// A 1440p screen decoded into previews on the receiver must not hold up that device's call loop, which also
/// carries its audio. Bob has no window decoding for him, so every frame goes through the engine's own decoder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_received_1440p_screen_does_not_hold_up_the_call_loop() {
    let _serial = SERIAL.lock().await;
    heavy(5, async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;
        let mut bob_events = bob.engine.subscribe();
        alice.engine.call_dm(bob.identity.root_id()).await.expect("call starts");
        accept_incoming(&bob.engine, &mut bob_events).await;
        eventually("the call is up on both sides", || {
            alice.engine.call_state().filter(|v| v.participants.len() == 1 && v.frames_received > 0)
        })
        .await;
        let alice_root = alice.identity.root_id().to_string();
        let measure = |label: &str| {
            let stats = dissent_core::test_support::take_loop_stats(&bob.engine).expect("in a call");
            println!("LOOP {label}: {} ticks, {} over 40 ms late, worst {:.0} ms, {} audio frames from Alice in 10 s", stats.0, stats.1, stats.2, stats.3);
            stats
        };
        dissent_core::test_support::take_loop_stats(&bob.engine);
        tokio::time::sleep(Duration::from_secs(10)).await;
        let quiet = measure("without a share");

        alice.engine.set_test_capture(TestCapture::Document { width: 2560, height: 1440 });
        alice.engine.set_screen_bitrate(6_000);
        alice.engine.set_screen_share(true, None).expect("screen share on");
        bob.engine.set_video_tile(&alice_root, true, 2560);
        let (count, _) = previews_for(&mut bob_events, &alice_root, true, Duration::from_secs(20)).await;
        assert!(count > 0, "Bob's engine makes previews of the share");
        dissent_core::test_support::take_loop_stats(&bob.engine);
        tokio::time::sleep(Duration::from_secs(10)).await;
        let sharing = measure("receiving a 1440p share as previews");
        assert!(quiet.3 >= 400, "audio flows without a share: {quiet:?}");
        assert!(sharing.3 >= quiet.3 * 9 / 10, "the share does not cost Bob his audio: {sharing:?} against {quiet:?}");
        // Decoding in the loop held it up about 380 ms at a time; 100 ms (doubled on a slower runner) still catches that.
        assert!(sharing.2 < 100.0 * scale(), "the call loop is never held up for long: {sharing:?}");

        alice.engine.set_screen_share(false, None).expect("screen share off");
        finish(vec![alice, bob]).await;
    })
    .await;
}

