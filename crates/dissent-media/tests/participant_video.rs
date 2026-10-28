//! A participant sends video to its forwarder, and a second participant reads it there. The
//! forwarder holds no key, so it routes the frames as they are.
//!
//! Runs over in-memory transports, so no socket is bound.

mod common;

use std::time::Duration;

use common::memory_pair;
use dissent_media::{Candidate, MediaKey, Mode, Participant, Sfu, VideoCodec, VideoFrame, VideoMeta};
use tokio::sync::oneshot;
use tokio::time::{interval, timeout};

const FRAMES: u32 = 12;
const GOP: u32 = 6;
const FORWARDER: u64 = 9;

fn is_key(frame_id: u32) -> bool {
    frame_id.is_multiple_of(GOP)
}

fn body(frame_id: u32) -> Vec<u8> {
    let len = if is_key(frame_id) { 9_000 } else { 1_800 };
    (0..len)
        .map(|i| ((i * 3 + frame_id as usize * 17) % 251) as u8)
        .collect()
}

fn meta(frame_id: u32) -> VideoMeta {
    VideoMeta {
        frame_id,
        keyframe: is_key(frame_id),
        codec: VideoCodec::H264,
        screen: false,
        layer: 0,
    }
}

/// One forwarder, and a participant that is linked to it and has elected it.
fn candidates() -> Vec<Candidate> {
    vec![Candidate {
        peer: FORWARDER,
        upload_kbps: 30_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 20,
    }]
}

#[tokio::test]
async fn a_participant_reads_another_participants_video_through_the_forwarder() {
    let key = MediaKey::new([31; 32]);
    let mut sfu = Sfu::new(Mode::Video720p);
    let mut sender = Participant::new(key.clone(), 1, candidates(), 2, Mode::Video720p).unwrap();
    let mut reader = Participant::new(key.clone(), 2, candidates(), 2, Mode::Video720p).unwrap();

    for participant in [&mut sender, &mut reader] {
        let (end, forwarder_end) = memory_pair();
        let offer = participant.offer_sfu(FORWARDER, end).await.unwrap();
        let answer = sfu.accept(participant.id(), forwarder_end, offer).await.unwrap();
        participant.apply_sfu(FORWARDER, answer).unwrap();
        participant.elect().unwrap();
        assert_eq!(participant.active(), Some(FORWARDER), "the forwarder carries the call");
    }

    let (run_done, run_stop) = oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::select! {
            _ = run_stop => {}
            _ = sfu.run() => {}
        }
    });

    let (sending_done, sending_stop) = oneshot::channel::<()>();
    let sending = tokio::spawn(async move {
        sender.wait_connected().await.unwrap();
        let mut ticker = interval(Duration::from_millis(33));
        for id in 0..FRAMES {
            ticker.tick().await;
            sender.send_video(&body(id), meta(id)).await.unwrap();
        }
        // Keep the link driven until the reader has everything, so the forwarder's packets go out.
        tokio::select! {
            _ = sending_stop => {}
            _ = async {
                while let Ok(Some(_)) = sender.next_video_frame().await {}
            } => {}
        }
        sender
    });

    reader.wait_connected().await.unwrap();
    let mut got: Vec<VideoFrame> = Vec::new();
    let _ = timeout(Duration::from_secs(10), async {
        while got.len() < FRAMES as usize {
            match reader.next_video_frame().await {
                Ok(Some(frame)) => got.push(frame),
                _ => break,
            }
        }
    })
    .await;
    let _ = sending_done.send(());
    let _ = run_done.send(());
    let _ = sending.await;

    assert_eq!(got.len(), FRAMES as usize, "every frame reaches the other participant");
    for (i, frame) in got.iter().enumerate() {
        assert_eq!(frame.sender, 1, "the frame names the sender, not the forwarder");
        assert_eq!(frame.frame_id as usize, i);
        assert_eq!(frame.keyframe, is_key(i as u32));
        assert_eq!(frame.payload, body(i as u32), "frame {i} is unchanged by the forwarder");
    }
}
