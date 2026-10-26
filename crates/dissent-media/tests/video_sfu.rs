//! A forwarder routes video without a key: a listener gets a sender's frames through it, and a
//! gap in a sender's frame ids drops the deltas after the gap until the next keyframe.
//!
//! Runs over in-memory transports, so no socket is bound.

mod common;

use std::time::Duration;

use common::memory_pair;
use dissent_media::{Call, MediaKey, Mesh, Mode, Sfu, VideoCodec, VideoFrame, VideoMeta};
use tokio::sync::oneshot;
use tokio::time::{interval, timeout};

const FRAMES: u32 = 40;
const GOP: u32 = 10;

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

fn body(frame_id: u32) -> Vec<u8> {
    let len = if is_key(frame_id) { 12_000 } else { 2_000 };
    (0..len)
        .map(|i| ((i * 5 + frame_id as usize * 11) % 251) as u8)
        .collect()
}

/// Connects `sender` to the forwarder as a downlink, and returns the sender's call.
async fn join_as_sender(sfu: &mut Sfu<common::MemoryTransport>, id: u64, key: &MediaKey) -> Call<common::MemoryTransport> {
    let (end, forwarder_end) = memory_pair();
    let (mut call, offer) = Call::offer_over(end, key.clone(), id).await.unwrap();
    let answer = sfu.accept(id, forwarder_end, offer).await.unwrap();
    call.apply(answer).unwrap();
    call
}

/// Connects a listener to the forwarder, as a downlink that offers.
async fn join_as_listener(sfu: &mut Sfu<common::MemoryTransport>, id: u64, key: &MediaKey) -> Call<common::MemoryTransport> {
    join_as_sender(sfu, id, key).await
}

/// Sends frames from `sender`, skipping the ids in `skip`. The sender keeps driving its link
/// until `done` fires, since str0m's pacer only releases packets while the call is pumped.
async fn send_frames(
    mut sender: Call<common::MemoryTransport>,
    skip: Vec<u32>,
    done: oneshot::Receiver<()>,
) {
    sender.wait_connected().await.unwrap();
    let mut ticker = interval(Duration::from_millis(33));
    for id in 0..FRAMES {
        ticker.tick().await;
        if skip.contains(&id) {
            continue;
        }
        sender
            .send_video(&body(id), meta(id))
            .await
            .unwrap();
    }
    tokio::select! {
        _ = done => {}
        _ = async {
            while let Ok(Some(_)) = sender.next_video_frame().await {}
        } => {}
    }
}

/// Reads frames until `expect` have arrived or ten seconds pass.
async fn collect(listener: &mut Call<common::MemoryTransport>, expect: usize) -> Vec<VideoFrame> {
    listener.wait_connected().await.unwrap();
    let mut got = Vec::new();
    let _ = timeout(Duration::from_secs(10), async {
        while got.len() < expect {
            match listener.next_video_frame().await {
                Ok(Some(frame)) => got.push(frame),
                _ => break,
            }
        }
    })
    .await;
    got
}

#[tokio::test]
async fn a_listener_gets_every_frame_through_a_forwarder() {
    let key = MediaKey::new([21; 32]);
    let mut sfu = Sfu::new(Mode::Video720p);
    let sender = join_as_sender(&mut sfu, 1, &key).await;
    let mut listener = join_as_listener(&mut sfu, 2, &key).await;
    tokio::spawn(async move {
        let _ = sfu.run().await;
    });

    let (done, done_rx) = oneshot::channel();
    let sending = tokio::spawn(send_frames(sender, Vec::new(), done_rx));
    let got = collect(&mut listener, FRAMES as usize).await;
    let _ = done.send(());
    let _ = sending.await;

    assert_eq!(got.len(), FRAMES as usize, "every frame reaches the listener");
    for (i, frame) in got.iter().enumerate() {
        let id = i as u32;
        assert_eq!(frame.sender, 1);
        assert_eq!(frame.frame_id, id);
        assert_eq!(frame.keyframe, is_key(id));
        assert_eq!(frame.payload, body(id), "frame {id} is unchanged by the forwarder");
    }
}

#[tokio::test]
async fn a_gap_drops_deltas_until_the_next_keyframe() {
    let key = MediaKey::new([22; 32]);
    let mut sfu = Sfu::new(Mode::Video720p);
    let sender = join_as_sender(&mut sfu, 1, &key).await;
    let mut listener = join_as_listener(&mut sfu, 2, &key).await;
    tokio::spawn(async move {
        let _ = sfu.run().await;
    });

    // Frame 5 never leaves the sender, so frames 6 to 9 are deltas after a gap.
    let (done, done_rx) = oneshot::channel();
    let sending = tokio::spawn(send_frames(sender, vec![5], done_rx));
    let got = collect(&mut listener, 5 + (FRAMES - GOP) as usize).await;
    let _ = done.send(());
    let _ = sending.await;

    let ids: Vec<u32> = got.iter().map(|f| f.frame_id).collect();
    let expected: Vec<u32> = (0..5).chain(10..FRAMES).collect();
    assert_eq!(ids, expected, "deltas 6 to 9 are dropped, and decoding resumes at keyframe 10");
    for frame in &got {
        assert_eq!(frame.payload, body(frame.frame_id));
    }
}

#[tokio::test]
async fn a_mesh_sends_one_video_frame_to_every_peer() {
    let key = MediaKey::new([23; 32]);
    let mut one = Mesh::new(key.clone(), 1).unwrap();
    let mut two = Mesh::new(key.clone(), 2).unwrap();
    let (end_one, end_two) = memory_pair();
    let offer = one.offer(2, end_one).await.unwrap();
    let answer = two.accept(1, end_two, offer).await.unwrap();
    one.apply(2, answer).unwrap();
    let (a, b) = tokio::join!(one.wait_connected(), two.wait_connected());
    a.unwrap();
    b.unwrap();

    let sending = tokio::spawn(async move {
        let mut ticker = interval(Duration::from_millis(33));
        for id in 0..FRAMES_MESH {
            ticker.tick().await;
            one.send_video(&body(id), meta(id)).await.unwrap();
        }
        // Keep the mesh driven while the peer reads, so its packets are paced out.
        let _ = timeout(Duration::from_secs(5), async {
            while let Ok(Some(_)) = one.next_video_frame().await {}
        })
        .await;
    });

    let mut got = Vec::new();
    let _ = timeout(Duration::from_secs(10), async {
        while got.len() < FRAMES_MESH as usize {
            match two.next_video_frame().await {
                Ok(Some(frame)) => got.push(frame),
                _ => break,
            }
        }
    })
    .await;
    sending.abort();
    assert_eq!(got.len(), FRAMES_MESH as usize, "every frame reaches the peer through the mesh");
    for (i, frame) in got.iter().enumerate() {
        assert_eq!(frame.frame_id as usize, i);
        assert_eq!(frame.sender, 1);
        assert_eq!(frame.payload, body(i as u32));
    }
}

const FRAMES_MESH: u32 = 12;
