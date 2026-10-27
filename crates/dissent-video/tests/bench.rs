//! Realistic encode cost at 720p, for every encodable codec. Ignored by default because it takes
//! seconds in release and much longer in debug.
//!
//! Run with: `cargo test --release -p dissent-video --test bench -- --ignored --nocapture`

use std::time::Instant;

use dissent_video::decode;
use dissent_video::encode::{self, EncoderConfig};
use dissent_video::pattern::PatternSource;
use dissent_video::{CaptureFormat, Codec, VideoSource};

const FRAMES: usize = 30;

#[test]
#[ignore = "realistic 720p encode timing: run in release with --ignored --nocapture"]
fn realistic_720p_encode_ms_per_frame() {
    let (width, height, fps) = (1280, 720, 30);
    let mut src = PatternSource::new(CaptureFormat { width, height, fps }, 0x5eed);
    let frames: Vec<_> = (0..FRAMES).map(|_| src.next_frame().unwrap()).collect();

    for codec in Codec::ENCODABLE.iter().copied() {
        let mut enc = encode::open(
            codec,
            EncoderConfig {
                width,
                height,
                fps,
                bitrate_kbps: 1_000,
                keyframe_interval: 300,
            },
        )
        .unwrap();

        let start = Instant::now();
        let mut bytes = 0usize;
        for frame in &frames {
            bytes += enc
                .encode(frame)
                .unwrap()
                .iter()
                .map(|c| c.data.len())
                .sum::<usize>();
        }
        let ms = start.elapsed().as_secs_f64() * 1_000.0 / FRAMES as f64;
        println!("{codec:?} 720p @30 fps, 1000 kbps: {ms:.1} ms/frame, {bytes} bytes over {FRAMES} frames");
    }
}

#[test]
#[ignore = "realistic 720p decode timing: run in release with --ignored --nocapture"]
fn realistic_720p_decode_ms_per_frame() {
    let (width, height, fps) = (1280, 720, 30);
    let mut src = PatternSource::new(CaptureFormat { width, height, fps }, 0x5eed);
    let frames: Vec<_> = (0..FRAMES).map(|_| src.next_frame().unwrap()).collect();

    for codec in Codec::ENCODABLE.iter().copied() {
        let mut enc = encode::open(
            codec,
            EncoderConfig {
                width,
                height,
                fps,
                bitrate_kbps: 1_000,
                keyframe_interval: 300,
            },
        )
        .unwrap();
        let chunks: Vec<Vec<u8>> = frames
            .iter()
            .flat_map(|frame| enc.encode(frame).unwrap())
            .map(|chunk| chunk.data)
            .collect();

        let mut dec = decode::open(codec).unwrap();
        let start = Instant::now();
        let mut decoded = 0usize;
        for (i, chunk) in chunks.iter().enumerate() {
            decoded += dec.decode(chunk, i as u64).unwrap().len();
        }
        let ms = start.elapsed().as_secs_f64() * 1_000.0 / chunks.len() as f64;
        println!("{codec:?} 720p decode: {ms:.1} ms/frame, {decoded} of {} frames out", chunks.len());
    }
}

#[test]
#[ignore = "realistic 1080p encode timing at 30 and 60 fps: run in release with --ignored --nocapture"]
fn realistic_1080p_encode_ms_per_frame() {
    let (width, height) = (1920, 1080);
    for (fps, kbps) in [(30, 3_000), (60, 5_000)] {
        let mut src = PatternSource::new(CaptureFormat { width, height, fps }, 0x5eed);
        let frames: Vec<_> = (0..FRAMES).map(|_| src.next_frame().unwrap()).collect();

        for codec in Codec::ENCODABLE.iter().copied() {
            let mut enc = encode::open(
                codec,
                EncoderConfig {
                    width,
                    height,
                    fps,
                    bitrate_kbps: kbps,
                    keyframe_interval: 300,
                },
            )
            .unwrap();

            let start = Instant::now();
            let mut bytes = 0usize;
            for frame in &frames {
                bytes += enc
                    .encode(frame)
                    .unwrap()
                    .iter()
                    .map(|c| c.data.len())
                    .sum::<usize>();
            }
            let ms = start.elapsed().as_secs_f64() * 1_000.0 / FRAMES as f64;
            println!(
                "{codec:?} 1080p @{fps} fps, {kbps} kbps: {ms:.1} ms/frame (budget {:.1}), {bytes} bytes",
                1_000.0 / fps as f64
            );
        }
    }
}
