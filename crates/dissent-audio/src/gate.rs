//! Voice-activity gate. A frame passes when its level is at or above a threshold. The gate stays
//! open for a short hold after the level drops, so word endings are not clipped.

use crate::meter::FLOOR_DB;

/// How far above the tracked noise floor the automatic threshold sits, in dB.
pub const AUTO_MARGIN_DB: f32 = 12.0;

/// Frames the gate stays open after the last frame above the threshold, 160 ms at 20 ms frames.
const HOLD_FRAMES: u32 = 8;

/// The noise floor starts here, and stays inside these bounds.
const FLOOR_START_DB: f32 = -60.0;
const FLOOR_MIN_DB: f32 = -90.0;
const FLOOR_MAX_DB: f32 = -30.0;

/// How fast the noise floor follows a quieter frame, and a louder frame that is still under the
/// threshold. Louder speech is never learned as noise.
const FLOOR_FALL: f32 = 0.2;
const FLOOR_RISE: f32 = 0.01;

/// The gate's threshold: fixed, or following the noise floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Threshold {
    /// The threshold tracks the noise floor, [`AUTO_MARGIN_DB`] above it.
    Auto,
    /// A fixed level in dBFS.
    Manual(f32),
}

/// Opens for frames above the threshold and closes after [`HOLD_FRAMES`] quiet frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceGate {
    threshold: Threshold,
    floor_db: f32,
    hold: u32,
}

impl VoiceGate {
    /// A gate with the given threshold. The noise floor starts at -60 dBFS.
    pub fn new(threshold: Threshold) -> Self {
        VoiceGate {
            threshold,
            floor_db: FLOOR_START_DB,
            hold: 0,
        }
    }

    /// Changes the threshold. The open state and the noise floor carry over.
    pub fn set_threshold(&mut self, threshold: Threshold) {
        self.threshold = threshold;
    }

    /// The level, in dBFS, a frame must reach to open the gate right now.
    pub fn threshold_db(&self) -> f32 {
        match self.threshold {
            Threshold::Auto => self.floor_db + AUTO_MARGIN_DB,
            Threshold::Manual(db) => db,
        }
    }

    /// The tracked noise floor in dBFS. Only meaningful for the automatic threshold.
    pub fn floor_db(&self) -> f32 {
        self.floor_db
    }

    /// Decides for one frame's level in dBFS and returns whether the gate is open.
    pub fn update(&mut self, level_db: f32) -> bool {
        if self.threshold == Threshold::Auto {
            self.track_floor(level_db);
        }
        if level_db >= self.threshold_db() {
            self.hold = HOLD_FRAMES;
            true
        } else if self.hold > 0 {
            self.hold -= 1;
            true
        } else {
            false
        }
    }

    /// Moves the noise floor toward frames that are not speech. A frame near or above the
    /// threshold is left out, so long speech does not raise the floor.
    fn track_floor(&mut self, level_db: f32) {
        if level_db >= self.floor_db + AUTO_MARGIN_DB {
            return;
        }
        let rate = if level_db < self.floor_db { FLOOR_FALL } else { FLOOR_RISE };
        self.floor_db = (self.floor_db + (level_db - self.floor_db) * rate)
            .clamp(FLOOR_MIN_DB, FLOOR_MAX_DB);
    }
}

impl Default for VoiceGate {
    fn default() -> Self {
        Self::new(Threshold::Auto)
    }
}

/// The lowest threshold a user can pick, in dBFS. Below this the gate passes almost anything.
pub const MANUAL_MIN_DB: f32 = -80.0;
/// The highest threshold a user can pick, in dBFS.
pub const MANUAL_MAX_DB: f32 = -10.0;

/// Keeps a manual threshold inside the range the sensitivity slider offers.
pub fn clamp_manual(db: f32) -> f32 {
    if db.is_nan() {
        return FLOOR_DB;
    }
    db.clamp(MANUAL_MIN_DB, MANUAL_MAX_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_threshold_opens_above_and_holds_before_closing() {
        let mut gate = VoiceGate::new(Threshold::Manual(-40.0));
        assert!(!gate.update(-50.0), "quiet frame stays closed");
        assert!(gate.update(-30.0), "loud frame opens");
        for _ in 0..HOLD_FRAMES {
            assert!(gate.update(-60.0), "hold keeps the gate open");
        }
        assert!(!gate.update(-60.0), "closes after the hold");
    }

    #[test]
    fn a_frame_exactly_at_the_threshold_opens_the_gate() {
        let mut gate = VoiceGate::new(Threshold::Manual(-40.0));
        assert!(gate.update(-40.0));
    }

    #[test]
    fn auto_threshold_settles_above_the_room_noise() {
        let mut gate = VoiceGate::new(Threshold::Auto);
        for _ in 0..500 {
            gate.update(-60.0);
        }
        assert!((gate.floor_db() + 60.0).abs() < 1.0, "floor {}", gate.floor_db());
        assert!(!gate.update(-60.0), "room noise stays closed");
        assert!(gate.update(-30.0), "speech opens the gate");
    }

    #[test]
    fn speech_does_not_raise_the_auto_floor() {
        let mut gate = VoiceGate::new(Threshold::Auto);
        for _ in 0..500 {
            gate.update(-60.0);
        }
        let before = gate.floor_db();
        for _ in 0..500 {
            gate.update(-20.0);
        }
        assert_eq!(gate.floor_db(), before);
    }

    #[test]
    fn auto_floor_follows_a_quieter_room_down_quickly() {
        let mut gate = VoiceGate::new(Threshold::Auto);
        for _ in 0..50 {
            gate.update(-80.0);
        }
        assert!(gate.floor_db() < -79.0, "floor {}", gate.floor_db());
    }

    #[test]
    fn manual_threshold_is_clamped_to_the_slider_range() {
        assert_eq!(clamp_manual(-200.0), MANUAL_MIN_DB);
        assert_eq!(clamp_manual(0.0), MANUAL_MAX_DB);
        assert_eq!(clamp_manual(-40.0), -40.0);
        assert_eq!(clamp_manual(f32::NAN), FLOOR_DB);
    }
}
