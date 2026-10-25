//! A deterministic test source. It needs no hardware and is used in tests and CI.

use std::time::Duration;

use crate::error::Result;
use crate::frame::{from_packed_rgb, Frame};
use crate::source::{CaptureFormat, VideoSource};

/// Moving colour bars with a seeded noise layer. The noise keeps the bitrate honest:
/// a flat image would compress to almost nothing and hide rate-control bugs.
pub struct PatternSource {
    format: CaptureFormat,
    index: u64,
    seed: u32,
}

impl PatternSource {
    pub fn new(format: CaptureFormat, seed: u32) -> Self {
        Self {
            format,
            index: 0,
            seed,
        }
    }

    /// The gap between frame timestamps at the configured rate.
    pub fn frame_period(&self) -> Duration {
        Duration::from_micros(1_000_000 / self.format.fps.max(1) as u64)
    }
}

impl VideoSource for PatternSource {
    fn format(&self) -> CaptureFormat {
        self.format
    }

    fn next_frame(&mut self) -> Result<Frame> {
        let CaptureFormat { width, height, .. } = self.format;
        let (w, h) = (width as usize, height as usize);
        let shift = self.index as usize * 3;
        let mut rgb = Vec::with_capacity(w * h * 3);
        let mut noise = self.seed;
        for y in 0..h {
            for x in 0..w {
                noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let jitter = (noise >> 28) as u8;
                let bar = ((x + shift) / (w / 8).max(1)) % 8;
                let (r, g, b) = BARS[bar];
                rgb.push(r.saturating_add(jitter));
                rgb.push(g.saturating_add(y as u8 / 4));
                rgb.push(b);
            }
        }
        let pts_us = self.index * self.frame_period().as_micros() as u64;
        self.index += 1;
        from_packed_rgb(width, height, 3, pts_us, &rgb)
    }
}

const BARS: [(u8, u8, u8); 8] = [
    (235, 235, 235),
    (235, 235, 16),
    (16, 235, 235),
    (16, 235, 16),
    (235, 16, 235),
    (235, 16, 16),
    (16, 16, 235),
    (16, 16, 16),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_frames_at_the_frame_rate() {
        let mut src = PatternSource::new(
            CaptureFormat {
                width: 16,
                height: 16,
                fps: 25,
            },
            1,
        );
        let a = src.next_frame().unwrap();
        let b = src.next_frame().unwrap();
        assert_eq!(a.pts_us(), 0);
        assert_eq!(b.pts_us(), 40_000);
        assert_ne!(a.data(), b.data(), "the bars must move between frames");
    }
}
