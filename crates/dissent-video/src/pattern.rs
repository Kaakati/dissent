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

/// A page of text that scrolls: dark marks in lines on white, moved up 6 pixels each frame. This is
/// the hard case of a screen, unlike noise: every sharp edge moves, and a keyframe costs the most.
pub fn scrolling_document(width: u32, height: u32, index: u64) -> Frame {
    let mut rgba = vec![255u8; (width * height * 4) as usize];
    for y in 0..height {
        let page_y = y as u64 + index * 6;
        let (line, row) = (page_y / 24, page_y % 24);
        if row >= 14 {
            continue;
        }
        for x in 0..width {
            let glyph = (x as u64 / 9).wrapping_mul(2_654_435_761).wrapping_add(line.wrapping_mul(40_503));
            if (glyph >> 7) & 3 != 0 && x % 9 < 6 && (glyph >> (row % 8)) & 1 == 1 {
                let at = ((y * width + x) * 4) as usize;
                rgba[at..at + 3].copy_from_slice(&[24, 24, 28]);
            }
        }
    }
    from_packed_rgb(width, height, 4, index * 33_333, &rgba).expect("the buffer has the size of the frame")
}

/// Stands in for a screen or window of a given size: a scrolling document drawn at the format, which
/// reports its own size as the native one. Tests change that size through `native` to resize it.
pub struct DocumentSource {
    format: CaptureFormat,
    native: std::sync::Arc<std::sync::Mutex<(u32, u32)>>,
    index: u64,
    next_due: std::time::Instant,
}

impl DocumentSource {
    pub fn new(format: CaptureFormat, native: std::sync::Arc<std::sync::Mutex<(u32, u32)>>) -> Self {
        Self { format, native, index: 0, next_due: std::time::Instant::now() }
    }
}

impl VideoSource for DocumentSource {
    fn format(&self) -> CaptureFormat {
        self.format
    }

    fn next_frame(&mut self) -> Result<Frame> {
        // On a clock of its own, as a screen is, so a slow encoder does not lower the rate by more.
        let now = std::time::Instant::now();
        if now < self.next_due {
            std::thread::sleep(self.next_due - now);
        }
        self.next_due = std::time::Instant::now().max(self.next_due) + Duration::from_micros(1_000_000 / self.format.fps.max(1) as u64);
        self.index += 1;
        Ok(scrolling_document(self.format.width, self.format.height, self.index))
    }

    fn native_size(&self) -> Option<(u32, u32)> {
        Some(*self.native.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn set_format(&mut self, format: CaptureFormat) -> bool {
        self.format = format;
        true
    }
}

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
