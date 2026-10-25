//! Input level meter. It reports the loudness of the processed mic, in dBFS, for the level bars
//! and the sensitivity slider.

use crate::dsp::rms;

/// The quietest level the meter reports. Anything at or below it reads as this floor.
pub const FLOOR_DB: f32 = -100.0;

/// Fraction of the gap to a quieter frame that the meter closes each frame. Rises are immediate.
const FALL: f32 = 0.3;

/// Level of a frame in dBFS, 0.0 at full scale. Silence reads as [`FLOOR_DB`].
pub fn dbfs(frame: &[f32]) -> f32 {
    to_db(rms(frame))
}

/// Converts a linear RMS value to dBFS, floored at [`FLOOR_DB`].
pub fn to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        return FLOOR_DB;
    }
    (20.0 * linear.log10()).max(FLOOR_DB)
}

/// A meter that follows each frame's level. It jumps up at once and falls back slowly, so the
/// bars stay readable at 20 ms frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelMeter {
    db: f32,
}

impl LevelMeter {
    /// A meter at the floor.
    pub fn new() -> Self {
        LevelMeter { db: FLOOR_DB }
    }

    /// Takes one frame and returns the displayed level in dBFS.
    pub fn push(&mut self, frame: &[f32]) -> f32 {
        let level = dbfs(frame);
        self.db = if level > self.db {
            level
        } else {
            self.db + (level - self.db) * FALL
        };
        self.db
    }

    /// The displayed level in dBFS.
    pub fn db(&self) -> f32 {
        self.db
    }
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..960).map(|i| amplitude * (i as f32 * 0.1).sin()).collect()
    }

    #[test]
    fn silence_reads_as_the_floor() {
        assert_eq!(dbfs(&[0.0; 960]), FLOOR_DB);
        assert_eq!(dbfs(&[]), FLOOR_DB);
    }

    #[test]
    fn full_scale_reads_near_zero_db() {
        // A sine's RMS is 1/sqrt(2) of its peak, so a 1.0 sine reads about -3 dBFS.
        let level = dbfs(&tone(1.0));
        assert!((level + 3.01).abs() < 0.1, "got {level}");
    }

    #[test]
    fn meter_jumps_up_and_falls_back_slowly() {
        let mut meter = LevelMeter::new();
        let loud = meter.push(&tone(0.5));
        assert!((loud - dbfs(&tone(0.5))).abs() < 1e-4, "rise should be immediate");

        let after_one = meter.push(&[0.0; 960]);
        assert!(after_one < loud && after_one > FLOOR_DB, "fall should be gradual");

        for _ in 0..50 {
            meter.push(&[0.0; 960]);
        }
        assert!((meter.db() - FLOOR_DB).abs() < 0.01, "got {}", meter.db());
    }

    #[test]
    fn floor_clamps_very_quiet_frames() {
        assert_eq!(to_db(1e-9), FLOOR_DB);
        assert_eq!(to_db(0.0), FLOOR_DB);
    }
}
