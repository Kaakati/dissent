//! Measures what a native-size screen share costs on this machine. Run in release with
//! `cargo test --release -p dissent-video --features svt-av1 --test screen_cost -- --ignored --nocapture`.
//! It uses synthetic pictures only, so no real screen is opened.

use std::time::Instant;

use dissent_video::encode::{self, EncoderConfig};
use dissent_video::frame::Frame;
use dissent_video::pattern::{scrolling_document, PatternSource};
use dissent_video::{CaptureFormat, Codec, VideoSource};

const SIZES: [(&str, u32, u32); 2] = [("1080p", 1920, 1080), ("1440p", 2560, 1440)];
const FRAMES: u64 = 90;

struct Cost {
    mean_ms: f64,
    max_ms: f64,
    kbps: f64,
    key_bytes: usize,
    delta_bytes: usize,
    /// Frames the encoder really coded. Rate control skips the rest.
    coded: usize,
}

fn run(mut encode: impl FnMut(&Frame) -> Vec<(bool, usize)>, frames: &[Frame], fps: u32) -> Cost {
    let (mut total_ms, mut max_ms, mut bytes, mut key_bytes, mut delta_bytes, mut deltas, mut coded) = (0.0f64, 0.0f64, 0usize, 0usize, 0usize, 0usize, 0usize);
    for index in 0..FRAMES as usize {
        let started = Instant::now();
        let out = encode(&frames[index % frames.len()]);
        let ms = started.elapsed().as_secs_f64() * 1_000.0;
        total_ms += ms;
        max_ms = max_ms.max(ms);
        for (key, len) in out {
            if len == 0 {
                continue;
            }
            coded += 1;
            bytes += len;
            if key {
                key_bytes = key_bytes.max(len);
            } else {
                delta_bytes += len;
                deltas += 1;
            }
        }
    }
    Cost {
        mean_ms: total_ms / FRAMES as f64,
        max_ms,
        kbps: bytes as f64 * 8.0 / (FRAMES as f64 / f64::from(fps)) / 1_000.0,
        key_bytes,
        delta_bytes: delta_bytes / deltas.max(1),
        coded,
    }
}

fn report(label: &str, cost: Cost) {
    println!(
        "COST {label}: {:.1} ms mean, {:.1} ms worst, {}/{FRAMES} coded, {:.0} kbps, keyframe {} KB, delta {} KB",
        cost.mean_ms,
        cost.max_ms,
        cost.coded,
        cost.kbps,
        cost.key_bytes / 1_000,
        cost.delta_bytes / 1_000
    );
}

fn pictures(width: u32, height: u32, noisy: bool) -> Vec<Frame> {
    if noisy {
        let mut source = PatternSource::new(CaptureFormat { width, height, fps: 30 }, 7);
        (0..30).map(|_| source.next_frame().expect("a frame")).collect()
    } else {
        (0..30).map(|i| scrolling_document(width, height, i)).collect()
    }
}

#[test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
fn encode_cost_at_native_sizes() {
    for (name, width, height) in SIZES {
        for (scene, noisy) in [("document", false), ("noise", true)] {
            let frames = pictures(width, height, noisy);
            for kbps in [1_500u32, 6_000] {
                for codec in Codec::ENCODABLE.iter().copied() {
                    let config = EncoderConfig { width, height, fps: 30, bitrate_kbps: kbps, keyframe_interval: 300, screen: true };
                    let mut encoder = encode::open(codec, config).expect("an encoder");
                    let cost = run(
                        |f| encoder.encode(f).expect("encodes").into_iter().map(|c| (c.keyframe, c.data.len())).collect(),
                        &frames,
                        30,
                    );
                    report(&format!("{name} {scene} {codec:?} {kbps} kbps"), cost);
                }
            }
        }
    }
}

/// The settings OpenH264 offers, to choose how screens run.
#[test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
fn openh264_settings_at_native_sizes() {
    use openh264::encoder::{BitRate, Complexity, Encoder, EncoderConfig as H264Config, FrameRate, FrameType, RateControlMode, UsageType};
    use openh264::formats::YUVSlices;
    use openh264::OpenH264API;
    for (name, width, height) in SIZES {
        let frames = pictures(width, height, false);
        let (w, h) = (width as usize, height as usize);
        for (usage_name, usage) in [("camera", UsageType::CameraVideoRealTime), ("screen", UsageType::ScreenContentRealTime)] {
            for threads in [1u16, 4, 8] {
                for (complexity_name, complexity) in [("low", Complexity::Low), ("medium", Complexity::Medium)] {
                    let config = H264Config::new()
                        .bitrate(BitRate::from_bps(3_000_000))
                        .max_frame_rate(FrameRate::from_hz(30.0))
                        .rate_control_mode(RateControlMode::Bitrate)
                        .usage_type(usage)
                        .complexity(complexity)
                        .num_threads(threads);
                    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).expect("an encoder");
                    let cost = run(
                        |f| {
                            let i420 = f.to_i420();
                            let (y, rest) = i420.data().split_at(w * h);
                            let (u, v) = rest.split_at(w * h / 4);
                            let stream = encoder.encode(&YUVSlices::new((y, u, v), (w, h), (w, w / 2, w / 2))).expect("encodes");
                            let key = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
                            vec![(key, stream.to_vec().len())]
                        },
                        &frames,
                        30,
                    );
                    report(&format!("{name} {usage_name} {threads} threads {complexity_name}"), cost);
                }
            }
        }
    }
}
