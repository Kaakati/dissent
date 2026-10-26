//! Regression tests for the video carrier: encrypted video frames carried as escaped H.264 NAL
//! units over str0m's writer path (see `video.rs` and `docs/design/04-calls.md`).
//!
//! These tests send frames large enough to need FU-A fragmentation, in both directions, over
//! loopback. If a str0m upgrade starts parsing the NAL body, or the escaping breaks, they fail.
//! Every socket binds to loopback only, so Windows Firewall shows no prompt.

use std::net::SocketAddr;
use std::time::Duration;

use dissent_media::{Call, MediaKey, Signal, VideoCodec, VideoFrame, VideoMeta};
use tokio::time::{interval, timeout};

/// Keyframes are large, so they fragment into many packets.
const KEY_BYTES: usize = 25_000;
const DELTA_BYTES: usize = 5_000;
const FRAMES: u32 = 90;
const GOP: u32 = 30;

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn key(byte: u8) -> MediaKey {
    MediaKey::new([byte; 32])
}

fn is_key(frame_id: u32) -> bool {
    frame_id.is_multiple_of(GOP)
}

/// The metadata of camera frame `frame_id`.
fn meta(frame_id: u32) -> VideoMeta {
    VideoMeta {
        frame_id,
        keyframe: is_key(frame_id),
        codec: VideoCodec::H264,
        screen: false,
    }
}

/// A deterministic frame body. The byte pattern includes zero runs, so escaping is exercised.
fn body(frame_id: u32) -> Vec<u8> {
    let len = if is_key(frame_id) {
        KEY_BYTES
    } else {
        DELTA_BYTES
    };
    (0..len)
        .map(|i| {
            let b = (i * 7 + frame_id as usize * 13) % 251;
            if i % 97 < 3 {
                0
            } else {
                b as u8
            }
        })
        .collect()
}

async fn connect_pair(caller_key: MediaKey, callee_key: MediaKey) -> (Call, Call) {
    let (mut caller, offer) = Call::start_offer(loopback(), caller_key, 1).await.unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) = Call::accept(loopback(), callee_key, 2, offer)
        .await
        .unwrap();
    let answer = Signal::from_bytes(&answer.to_bytes().unwrap()).unwrap();
    caller.apply(answer).unwrap();
    (caller, callee)
}

/// Sends `FRAMES` video frames at 30 fps while collecting the peer's frames until `expect`
/// have arrived, then keeps the session alive for `settle`.
async fn run_video_side(
    mut call: Call,
    expect: usize,
    settle: Duration,
) -> (Call, Vec<VideoFrame>) {
    call.wait_connected().await.unwrap();
    assert!(call.has_video(), "the peer answered the video line");

    let mut ticker = interval(Duration::from_millis(33));
    let mut sent = 0u32;
    let mut received = Vec::new();
    while sent < FRAMES || received.len() < expect {
        tokio::select! {
            _ = ticker.tick(), if sent < FRAMES => {
                call.send_video(&body(sent), meta(sent))
                    .await
                    .unwrap();
                sent += 1;
            }
            next = call.next_video_frame() => {
                received.push(next.unwrap().expect("call closed before all frames arrived"));
            }
        }
    }
    let _ = timeout(settle, async {
        while let Ok(Some(frame)) = call.next_video_frame().await {
            received.push(frame);
        }
    })
    .await;
    (call, received)
}

fn assert_frames_intact(received: &[VideoFrame], sender: u64) {
    assert_eq!(received.len(), FRAMES as usize, "every frame arrives");
    for (i, frame) in received.iter().enumerate() {
        let id = i as u32;
        assert_eq!(frame.sender, sender);
        assert_eq!(frame.frame_id, id, "frames arrive in order");
        assert_eq!(frame.keyframe, is_key(id), "keyframe flag of frame {id}");
        assert_eq!(frame.codec, VideoCodec::H264);
        assert_eq!(
            frame.payload,
            body(id),
            "frame {id} decrypts to what was sent"
        );
    }
}

#[tokio::test]
async fn fifty_kb_keyframes_cross_a_loopback_call_in_both_directions() {
    let (caller, callee) = connect_pair(key(7), key(7)).await;
    // Both sides offer AV1, so the call can use it. Frames still ride the H.264 carrier.
    assert!(caller.peer_has_av1() && callee.peer_has_av1(), "AV1 is negotiated when both sides offer it");

    let caller_task = tokio::spawn(run_video_side(
        caller,
        FRAMES as usize,
        Duration::from_secs(1),
    ));
    let callee_task = tokio::spawn(run_video_side(
        callee,
        FRAMES as usize,
        Duration::from_secs(1),
    ));

    let (_caller, from_callee) = caller_task.await.unwrap();
    let (_callee, from_caller) = callee_task.await.unwrap();
    assert_frames_intact(&from_caller, 1);
    assert_frames_intact(&from_callee, 2);
}

#[tokio::test]
async fn video_and_audio_share_a_call_without_losing_audio() {
    let (mut caller, mut callee) = connect_pair(key(9), key(9)).await;
    let (a, b) = tokio::join!(caller.wait_connected(), callee.wait_connected());
    a.unwrap();
    b.unwrap();
    assert!(caller.has_video() && callee.has_video());

    // The caller sends 60 audio frames at 20 ms, and a video frame after every second one.
    let sender = tokio::spawn(async move {
        let mut ticker = interval(Duration::from_millis(20));
        let mut video_sent = 0u32;
        for audio_seq in 0..60u64 {
            ticker.tick().await;
            let mut frame = vec![0u8; 160];
            frame[..8].copy_from_slice(&audio_seq.to_le_bytes());
            caller.send_audio_frame(&frame).await.unwrap();
            if audio_seq % 2 == 1 && video_sent < 30 {
                caller
                    .send_video(&body(video_sent), meta(video_sent))
                    .await
                    .unwrap();
                video_sent += 1;
            }
        }
        // Keep the session driven, so the callee's NACKs for lost packets get answered, until the
        // callee has read everything.
        let _ = timeout(Duration::from_secs(5), async {
            while let Ok(Some(_)) = caller.next_video_frame().await {}
        })
        .await;
    });

    let mut audio_got = 0usize;
    timeout(Duration::from_secs(10), async {
        while audio_got < 60 {
            callee.next_frame().await.unwrap().expect("call open");
            audio_got += 1;
        }
    })
    .await
    .expect("all audio arrives");

    // Video frames that arrived while audio was being read wait in their own queue.
    let mut video_got = 0usize;
    let r = timeout(Duration::from_secs(10), async {
        while video_got < 30 {
            let frame = callee.next_video_frame().await.unwrap().expect("call open");
            assert_eq!(frame.frame_id as usize, video_got, "video arrives in order");
            video_got += 1;
        }
    })
    .await;
    r.expect("all video arrives");
    sender.await.unwrap();
    assert_eq!(audio_got, 60, "audio is untouched by the video carrier");
    assert_eq!(video_got, 30);
}
