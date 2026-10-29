//! Simulcast layers through a forwarder: each viewer gets one layer of each stream, chosen from its
//! tile width and its budget. A narrow tile gets the small layer and a wide one the large layer.
//! Under a tight budget the active speaker keeps the large layer and the others drop first. A
//! viewer that does not know layers gets layer 0 only.
//!
//! Runs over in-memory transports, so no socket is bound.

mod common;

use std::time::Duration;

use common::memory_pair;
use dissent_media::{Call, Candidate, LayerInfo, MediaKey, Mode, Participant, Sfu, VideoCodec, VideoFrame, VideoMeta};
use tokio::sync::oneshot;
use tokio::time::{interval, timeout};

/// Frames per layer in one run, and a keyframe every `GOP` of them.
const FRAMES: u32 = 60;
const GOP: u32 = 5;
const FORWARDER: u64 = 9;

/// A small layer and a large one, as a sender announces them.
fn two_layers() -> Vec<LayerInfo> {
    vec![LayerInfo { width: 320, kbps: 150 }, LayerInfo { width: 1280, kbps: 1500 }]
}

fn meta(frame_id: u32, layer: u8) -> VideoMeta {
    VideoMeta {
        frame_id,
        keyframe: frame_id.is_multiple_of(GOP),
        codec: VideoCodec::H264,
        screen: false,
        layer,
    }
}

/// A frame body that names its layer in its first byte, so a viewer can check what it got.
fn body(frame_id: u32, layer: u8) -> Vec<u8> {
    let len = if layer == 0 { 600 } else { 3_000 };
    let mut out: Vec<u8> = (0..len).map(|i| ((i * 7 + frame_id as usize * 13) % 251) as u8).collect();
    out[0] = layer;
    out
}

/// Connects a keyed call to the forwarder as a downlink, and returns it.
async fn join(sfu: &mut Sfu<common::MemoryTransport>, id: u64, key: &MediaKey) -> Call<common::MemoryTransport> {
    let (end, forwarder_end) = memory_pair();
    let (mut call, offer) = Call::offer_over(end, key.clone(), id).await.unwrap();
    let answer = sfu.accept(id, forwarder_end, offer).await.unwrap();
    call.apply(answer).unwrap();
    call
}

/// Sends both layers of every frame, interleaved, then keeps the link driven until `done`.
async fn send_layers(mut sender: Call<common::MemoryTransport>, done: oneshot::Receiver<()>) {
    sender.wait_connected().await.unwrap();
    let mut ticker = interval(Duration::from_millis(33));
    for id in 0..FRAMES {
        ticker.tick().await;
        for layer in [0u8, 1] {
            sender.send_video(&body(id, layer), meta(id, layer)).await.unwrap();
        }
    }
    tokio::select! {
        _ = done => {}
        _ = async { while let Ok(Some(_)) = sender.next_video_frame().await {} } => {}
    }
}

/// Reads frames for `for_how_long`, or until the link closes.
async fn collect(viewer: &mut Call<common::MemoryTransport>, for_how_long: Duration) -> Vec<VideoFrame> {
    viewer.wait_connected().await.unwrap();
    let mut got = Vec::new();
    let _ = timeout(for_how_long, async {
        while let Ok(Some(frame)) = viewer.next_video_frame().await {
            got.push(frame);
        }
    })
    .await;
    got
}

#[tokio::test]
async fn a_narrow_tile_gets_the_small_layer_and_a_wide_one_the_large_layer() {
    let key = MediaKey::new([41; 32]);
    let mut sfu = Sfu::new(Mode::Video720p);
    // Only the tiles set here decide the layers. A link estimate that dips on a loaded machine would
    // lower the wide tile, and the test would read a layer it did not choose.
    sfu.set_use_estimates(false);
    let sender = join(&mut sfu, 1, &key).await;
    let mut narrow = join(&mut sfu, 2, &key).await;
    let mut wide = join(&mut sfu, 3, &key).await;
    let mut released = join(&mut sfu, 4, &key).await;
    sfu.set_sender_layers(1, false, two_layers());
    sfu.set_layer_aware(2, true);
    sfu.set_layer_aware(3, true);
    sfu.set_viewer_tile(2, 1, false, 240);
    sfu.set_viewer_tile(3, 1, false, 1200);
    // Viewer 4 is a released client: it does not know layers, so its wide tile still gets layer 0.
    sfu.set_viewer_tile(4, 1, false, 1200);
    assert_eq!(sfu.wanted_layers(1, false), 0b11, "the sender is asked for both layers");
    tokio::spawn(async move {
        let _ = sfu.run().await;
    });

    let (done, done_rx) = oneshot::channel();
    let sending = tokio::spawn(send_layers(sender, done_rx));
    let (n, w, r) = tokio::join!(
        collect(&mut narrow, Duration::from_secs(4)),
        collect(&mut wide, Duration::from_secs(4)),
        collect(&mut released, Duration::from_secs(4)),
    );
    let _ = done.send(());
    let _ = sending.await;

    for (name, got, layer) in [("the narrow tile", &n, 0u8), ("the wide tile", &w, 1), ("the released viewer", &r, 0)] {
        // Frames sent before the link is up are lost, so a little less than the whole run arrives.
        assert!(got.len() >= (FRAMES / 2) as usize, "{name} gets the stream: {} frames", got.len());
        assert!(got.iter().all(|f| f.layer == layer && f.payload[0] == layer), "{name} gets layer {layer} only");
        assert!(got[0].keyframe, "{name} starts at a keyframe");
        // One layer's frames follow on, so the decoder sees one stream.
        assert!(got.windows(2).all(|p| p[1].frame_id == p[0].frame_id + 1), "{name} gets every frame of its layer in order");
    }
}

fn candidates() -> Vec<Candidate> {
    vec![Candidate { peer: FORWARDER, upload_kbps: 30_000, on_battery: false, wired: true, cpu_ok: true, rtt_ms_median: 20 }]
}

/// A participant that sends speech at `level` and both video layers, until `done`. It also reads
/// what arrives, since a link is only driven while its call is polled.
async fn speak_and_send(mut sender: Participant<common::MemoryTransport>, level: u8, mut done: oneshot::Receiver<()>) {
    sender.wait_connected().await.unwrap();
    let mut ticker = interval(Duration::from_millis(33));
    let mut id = 0u32;
    loop {
        tokio::select! {
            _ = &mut done => break,
            _ = ticker.tick() => {
                sender.send_audio_frame(&common::frame(u64::from(id)), level).await.unwrap();
                for layer in [0u8, 1] {
                    sender.send_video(&body(id, layer), meta(id, layer)).await.unwrap();
                }
                id += 1;
            }
            next = sender.next_received_frame() => if next.is_err() { break },
        }
    }
}

#[tokio::test]
async fn a_tight_budget_keeps_the_speaker_sharp_and_lowers_the_others() {
    let key = MediaKey::new([42; 32]);
    let mut sfu = Sfu::new(Mode::Video720p);
    // Only the budget set here limits the viewer, not a link estimate that dips under load.
    sfu.set_use_estimates(false);
    // The speaker has the higher id, so a tie between equal savings would lower it first: only the
    // speaker rule keeps it sharp.
    let mut quiet = Participant::new(key.clone(), 1, candidates(), 2, Mode::Video720p).unwrap();
    let mut speaker = Participant::new(key.clone(), 2, candidates(), 2, Mode::Video720p).unwrap();
    for p in [&mut speaker, &mut quiet] {
        let (end, forwarder_end) = memory_pair();
        let offer = p.offer_sfu(FORWARDER, end).await.unwrap();
        let answer = sfu.accept(p.id(), forwarder_end, offer).await.unwrap();
        p.apply_sfu(FORWARDER, answer).unwrap();
        p.elect().unwrap();
    }
    let mut viewer = join(&mut sfu, 3, &key).await;
    for sender in [1, 2] {
        sfu.set_sender_layers(sender, false, two_layers());
        sfu.set_viewer_tile(3, sender, false, 1200);
    }
    sfu.set_layer_aware(3, true);
    // Both large layers need 3000 kbps. 1700 holds one large and one small.
    sfu.set_listener_budget(3, 1_700);
    let (stop_sfu, sfu_stop) = oneshot::channel::<()>();
    let forwarding = tokio::spawn(async move {
        tokio::select! {
            _ = sfu_stop => {}
            _ = sfu.run() => {}
        }
        sfu
    });

    let (stop_a, done_a) = oneshot::channel();
    let (stop_b, done_b) = oneshot::channel();
    // A low level is loud: 10 is speech, 127 is silence.
    let a = tokio::spawn(speak_and_send(speaker, 10, done_a));
    let b = tokio::spawn(speak_and_send(quiet, 127, done_b));
    let got = collect(&mut viewer, Duration::from_secs(5)).await;
    let _ = stop_sfu.send(());
    let _ = forwarding.await;
    let _ = stop_a.send(());
    let _ = stop_b.send(());
    let (ra, rb) = tokio::join!(a, b);
    ra.expect("the speaker's task ran");
    rb.expect("the other sender's task ran");

    // The plan settles once the forwarder has heard who speaks. Judge the second half of the run.
    let late = &got[got.len() / 2..];
    let layers_of = |sender: u64| -> Vec<u8> { late.iter().filter(|f| f.sender == sender).map(|f| f.layer).collect() };
    let (loud, soft) = (layers_of(2), layers_of(1));
    assert!(!loud.is_empty() && !soft.is_empty(), "both streams reach the viewer: {} and {}", loud.len(), soft.len());
    assert!(loud.iter().all(|l| *l == 1), "the speaker keeps the large layer: {loud:?}");
    assert!(soft.iter().all(|l| *l == 0), "the other drops to the small layer: {soft:?}");
}
