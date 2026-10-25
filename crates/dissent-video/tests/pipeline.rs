//! The encoder, the CPU governor and the adapter working together, with real encode timings.

use std::thread;
use std::time::{Duration, Instant};

use dissent_video::adapt::{Content, CpuGovernor, CpuVerdict, VideoAdapter};
use dissent_video::encode::{self, EncodedChunk, EncoderConfig, VideoEncoder};
use dissent_video::pattern::PatternSource;
use dissent_video::{CaptureFormat, Codec, Frame, Result, VideoSource};

/// A real encoder that also sleeps for `delay` on every frame, to stand in for a slow CPU.
struct SlowEncoder {
    inner: Box<dyn VideoEncoder>,
    delay: Duration,
}

impl VideoEncoder for SlowEncoder {
    fn config(&self) -> EncoderConfig {
        self.inner.config()
    }

    fn encode(&mut self, frame: &Frame) -> Result<Vec<EncodedChunk>> {
        thread::sleep(self.delay);
        self.inner.encode(frame)
    }

    fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }

    fn set_bitrate(&mut self, kbps: u32) -> Result<()> {
        self.inner.set_bitrate(kbps)
    }
}

fn frames(count: usize) -> Vec<Frame> {
    let mut src = PatternSource::new(
        CaptureFormat {
            width: 160,
            height: 90,
            fps: 30,
        },
        7,
    );
    (0..count).map(|_| src.next_frame().unwrap()).collect()
}

/// Runs the governor over real encode timings and returns the frame index of the first downgrade.
fn first_downgrade(
    enc: &mut dyn VideoEncoder,
    adapter: &VideoAdapter,
    count: usize,
) -> Option<usize> {
    let mut governor = CpuGovernor::new();
    let rungs = adapter.rungs().to_vec();
    for (index, frame) in frames(count).iter().enumerate() {
        let start = Instant::now();
        enc.encode(frame).unwrap();
        let ms = start.elapsed().as_secs_f64() * 1_000.0;
        if governor.observe(ms, &rungs, 0) == CpuVerdict::Downgrade {
            return Some(index);
        }
    }
    None
}

#[test]
fn a_slow_encoder_triggers_the_governor_downgrade() {
    let mut enc = SlowEncoder {
        inner: encode::open(
            Codec::H264,
            EncoderConfig {
                width: 160,
                height: 90,
                fps: 30,
                bitrate_kbps: 800,
                keyframe_interval: 60,
            },
        )
        .unwrap(),
        delay: Duration::from_millis(60),
    };
    // The governor's rungs are the same for every codec, so use the full ladder here.
    let adapter = VideoAdapter::with_codecs(Content::Camera, &Codec::ALL).unwrap();
    // Ten slow frames in a row are the trigger, so the verdict arrives on the tenth.
    assert_eq!(first_downgrade(&mut enc, &adapter, 20), Some(9));
}

#[test]
fn repeated_downgrades_reach_the_h264_rung() {
    let mut adapter = VideoAdapter::new(Content::Camera);
    for _ in 0..10 {
        for _ in 0..3 {
            adapter.update(3_000, 32);
        }
        adapter.apply_cpu(CpuVerdict::Downgrade);
    }
    let plan = adapter.update(3_000, 32);
    assert_eq!(
        plan.codec,
        Some(Codec::H264),
        "the lowest camera rung is H.264 at 360p"
    );
    assert_eq!(plan.height, 360);
}
