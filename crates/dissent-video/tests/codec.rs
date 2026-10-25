//! Encoder and decoder behaviour on both codecs. The frames are small so the default run stays fast.
//! The realistic 720p timing lives in `tests/bench.rs`.

use dissent_video::decode;
use dissent_video::encode::{self, EncodedChunk, EncoderConfig, VideoEncoder};
use dissent_video::metrics::psnr_luma;
use dissent_video::pattern::PatternSource;
use dissent_video::{CaptureFormat, Codec, Frame, VideoSource};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const FPS: u32 = 30;

fn frames(count: usize) -> Vec<Frame> {
    let mut src = PatternSource::new(
        CaptureFormat {
            width: WIDTH,
            height: HEIGHT,
            fps: FPS,
        },
        0x5eed,
    );
    (0..count).map(|_| src.next_frame().unwrap()).collect()
}

fn config(kbps: u32) -> EncoderConfig {
    EncoderConfig {
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        bitrate_kbps: kbps,
        keyframe_interval: 60,
    }
}

fn encode_all(enc: &mut dyn VideoEncoder, frames: &[Frame]) -> Vec<EncodedChunk> {
    let mut chunks = Vec::new();
    for frame in frames {
        chunks.extend(enc.encode(frame).unwrap());
    }
    chunks
}

#[test]
fn both_codecs_round_trip_with_usable_quality() {
    let input = frames(8);
    for codec in Codec::ENCODABLE.iter().copied() {
        let mut enc = encode::open(codec, config(600)).unwrap();
        let chunks = encode_all(enc.as_mut(), &input);
        assert!(!chunks.is_empty(), "{codec:?} produced no output");
        assert!(chunks[0].keyframe, "{codec:?} must start on a keyframe");

        let mut dec = decode::open(codec).unwrap();
        let mut decoded = Vec::new();
        for chunk in &chunks {
            decoded.extend(dec.decode(&chunk.data, chunk.pts_us).unwrap());
        }
        assert_eq!(
            decoded.len(),
            input.len(),
            "{codec:?}: every frame must come back"
        );

        let worst = input
            .iter()
            .zip(&decoded)
            .inspect(|(reference, picture)| assert_eq!(reference.pts_us(), picture.pts_us()))
            .map(|(reference, picture)| psnr_luma(reference, picture))
            .fold(f64::INFINITY, f64::min);
        assert!(
            worst > 28.0,
            "{codec:?}: worst luma PSNR {worst:.1} dB is below 28 dB"
        );
    }
}

#[test]
fn keyframe_request_produces_a_keyframe_on_that_frame() {
    let input = frames(12);
    for codec in Codec::ENCODABLE.iter().copied() {
        let mut enc = encode::open(codec, config(400)).unwrap();
        let mut chunks = encode_all(enc.as_mut(), &input[..6]);
        enc.request_keyframe();
        let requested_pts = input[6].pts_us();
        chunks.extend(encode_all(enc.as_mut(), &input[6..]));

        let at_request = chunks
            .iter()
            .find(|c| c.pts_us == requested_pts)
            .unwrap_or_else(|| panic!("{codec:?}: the requested frame must be emitted"));
        assert!(
            at_request.keyframe,
            "{codec:?}: the requested frame must be a keyframe"
        );
    }
}

#[cfg(feature = "svt-av1")]
#[test]
fn av1_bitrate_change_applies_within_a_second() {
    // One source, so the timestamps keep counting across the switch.
    let mut all = frames(45);
    let after = all.split_off(15);
    let before = all;

    let mut enc = encode::open(Codec::Av1, config(400)).unwrap();
    let mut chunks = encode_all(enc.as_mut(), &before);
    enc.set_bitrate(40).unwrap();
    chunks.extend(encode_all(enc.as_mut(), &after));

    let kbps_between = |from_us: u64, to_us: u64| {
        let bytes: usize = chunks
            .iter()
            .filter(|c| c.pts_us >= from_us && c.pts_us < to_us)
            .map(|c| c.data.len())
            .sum();
        bytes as f64 * 8.0 / ((to_us - from_us) as f64 / 1e6) / 1_000.0
    };
    let switch_us = after[0].pts_us();
    let high = kbps_between(0, switch_us);
    let one_second = kbps_between(switch_us, switch_us + 1_000_000);
    eprintln!("AV1 160x90: {high:.0} kbps before the change, {one_second:.0} kbps in the first second after");
    // At 160x90 the rate controller does not spend the whole target, so compare the two rates.
    assert!(high > 0.0, "the stream before the change must carry data");
    assert!(
        one_second < high / 2.0,
        "the lower target must apply within a second: {one_second:.0} vs {high:.0} kbps"
    );
}

#[test]
fn encoders_reject_zero_bitrate_and_wrong_sizes() {
    for codec in Codec::ENCODABLE.iter().copied() {
        let mut enc = encode::open(codec, config(400)).unwrap();
        assert!(
            enc.set_bitrate(0).is_err(),
            "{codec:?} must reject zero bitrate"
        );

        let wrong = PatternSource::new(
            CaptureFormat {
                width: 320,
                height: 180,
                fps: FPS,
            },
            1,
        )
        .next_frame()
        .unwrap();
        assert!(
            enc.encode(&wrong).is_err(),
            "{codec:?} must reject a frame of the wrong size"
        );
    }
}
