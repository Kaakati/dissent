//! The microphone chain: input gain, echo cancellation, noise suppression, AGC, the voice gate and
//! the level meter, in that order. The desktop app runs it on each 20 ms mic frame before the
//! frame goes to the call. Settings take effect on the next frame, so nothing restarts.
//!
//! Frames are 960 samples at 48 kHz. Noise suppression works on whole 480-sample blocks, so a
//! frame length that is not a multiple of 480 leaves its tail unsuppressed.

use crate::dsp::{apply_gain, Agc, EchoCanceller, NoiseSuppressor, DENOISE_BLOCK};
use crate::gate::{Threshold, VoiceGate};
use crate::meter::LevelMeter;

/// Echo canceller tail in milliseconds. It covers the speaker-to-mic path and device latency.
pub const ECHO_TAIL_MS: u32 = 64;

/// Noise suppression strength for the standard setting. The output is a mix of the raw and the
/// denoised signal, so 0.0 is off and 1.0 is full suppression.
pub const STANDARD_STRENGTH: f32 = 0.6;

/// Noise suppression strength for the strong setting, Voice Isolation.
pub const STRONG_STRENGTH: f32 = 1.0;

/// What the chain does. Build it from the user's settings and hand it to [`CaptureChain::set_settings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainSettings {
    /// Linear gain applied first. 1.0 is unchanged, 2.0 is +6 dB. Clamped at full scale.
    pub input_gain: f32,
    /// Noise suppression strength from 0.0 to 1.0, or `None` for no suppression.
    pub noise_suppression: Option<f32>,
    /// Run the echo canceller against the far-end reference from [`CaptureChain::push_far`].
    pub echo_cancellation: bool,
    /// Run the automatic gain control.
    pub agc: bool,
    /// When a frame passes the gate. Frames below it are silenced.
    pub threshold: Threshold,
}

impl Default for ChainSettings {
    /// Unity gain, with every processing stage off and the automatic gate.
    fn default() -> Self {
        ChainSettings {
            input_gain: 1.0,
            noise_suppression: None,
            echo_cancellation: false,
            agc: false,
            threshold: Threshold::Auto,
        }
    }
}

/// What one processed frame looked like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameReport {
    /// Level of the processed frame before the gate, in dBFS. Compare it with the threshold.
    pub level_db: f32,
    /// True when the frame passed the gate and was left as processed. False silences it.
    pub open: bool,
    /// The threshold in force for this frame, in dBFS. Under the automatic gate it follows the
    /// noise floor.
    pub threshold_db: f32,
}

/// The microphone processing chain. It keeps the filter state between frames, so one instance
/// lives for as long as the mic is open.
pub struct CaptureChain {
    settings: ChainSettings,
    denoise: NoiseSuppressor,
    echo: Option<EchoCanceller>,
    agc: Agc,
    gate: VoiceGate,
    meter: LevelMeter,
    dry: Vec<f32>,
}

impl CaptureChain {
    /// A chain with `settings`. Echo cancellation allocates its filter the first time it is on.
    pub fn new(settings: ChainSettings) -> Self {
        CaptureChain {
            settings,
            denoise: NoiseSuppressor::new(),
            echo: None,
            agc: Agc::new(),
            gate: VoiceGate::new(settings.threshold),
            meter: LevelMeter::new(),
            dry: Vec::new(),
        }
    }

    /// Replaces the settings. Filter state is kept, so changes apply from the next frame.
    pub fn set_settings(&mut self, settings: ChainSettings) {
        self.settings = settings;
        self.gate.set_threshold(settings.threshold);
    }

    /// The settings in effect.
    pub fn settings(&self) -> ChainSettings {
        self.settings
    }

    /// Feeds the far-end audio that was just played, the echo canceller's reference. Call it for
    /// every playout frame, whether or not echo cancellation is on, so enabling it mid-call starts
    /// with a current reference.
    pub fn push_far(&mut self, far: &[f32]) {
        if let Some(echo) = self.echo.as_mut() {
            echo.push_far(far);
        } else if self.settings.echo_cancellation {
            let mut echo = EchoCanceller::with_tail_ms(ECHO_TAIL_MS);
            echo.push_far(far);
            self.echo = Some(echo);
        }
    }

    /// Processes one mic frame in place. `allowed` is false when the user is muted or holds
    /// push-to-talk up. That closes the gate and silences the frame, and the level still updates.
    pub fn process(&mut self, frame: &mut [f32], allowed: bool) -> FrameReport {
        apply_gain(frame, self.settings.input_gain);

        if self.settings.echo_cancellation {
            if let Some(echo) = self.echo.as_mut() {
                echo.process(frame);
            }
        }

        if let Some(strength) = self.settings.noise_suppression {
            self.suppress(frame, strength);
        }

        if self.settings.agc {
            self.agc.process(frame);
        }

        let level_db = self.meter.push(frame);
        let open = self.gate.update(level_db) && allowed;
        if !open {
            frame.fill(0.0);
        }
        FrameReport {
            level_db,
            open,
            threshold_db: self.gate.threshold_db(),
        }
    }

    /// Runs RNNoise on each block and mixes the result with the raw signal by `strength`.
    fn suppress(&mut self, frame: &mut [f32], strength: f32) {
        self.dry.clear();
        self.dry.extend_from_slice(frame);
        for block in frame.as_chunks_mut::<DENOISE_BLOCK>().0 {
            self.denoise.process_block(block);
        }
        let strength = strength.clamp(0.0, 1.0);
        let whole = frame.len() / DENOISE_BLOCK * DENOISE_BLOCK;
        for (out, raw) in frame[..whole].iter_mut().zip(self.dry.iter()) {
            *out = raw + (*out - raw) * strength;
        }
    }
}

impl std::fmt::Debug for CaptureChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureChain")
            .field("settings", &self.settings)
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..960).map(|i| amplitude * (i as f32 * 0.1).sin()).collect()
    }

    #[test]
    fn input_gain_scales_the_frame() {
        let mut chain = CaptureChain::new(ChainSettings {
            input_gain: 2.0,
            threshold: Threshold::Manual(-80.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.25);
        let before = frame.clone();
        let report = chain.process(&mut frame, true);
        assert!(report.open);
        for (after, raw) in frame.iter().zip(before.iter()) {
            assert!((after - raw * 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn muted_frames_are_silenced_and_the_gate_closes() {
        let mut chain = CaptureChain::new(ChainSettings {
            threshold: Threshold::Manual(-80.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.5);
        let report = chain.process(&mut frame, false);
        assert!(!report.open);
        assert!(frame.iter().all(|s| *s == 0.0));
        assert!(report.level_db > -20.0, "the level still reads the mic");
    }

    #[test]
    fn quiet_room_noise_is_gated_under_a_manual_threshold() {
        let mut chain = CaptureChain::new(ChainSettings {
            threshold: Threshold::Manual(-30.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.001);
        let report = chain.process(&mut frame, true);
        assert!(!report.open);
        assert!(frame.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn settings_change_applies_on_the_next_frame() {
        let mut chain = CaptureChain::new(ChainSettings {
            threshold: Threshold::Manual(-80.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.25);
        chain.process(&mut frame, true);
        chain.set_settings(ChainSettings {
            input_gain: 0.0,
            threshold: Threshold::Manual(-80.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.25);
        chain.process(&mut frame, true);
        assert!(frame.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn echo_filter_is_only_built_when_enabled() {
        let mut chain = CaptureChain::new(ChainSettings::default());
        chain.push_far(&[0.1; 960]);
        assert!(chain.echo.is_none());
        chain.set_settings(ChainSettings {
            echo_cancellation: true,
            ..ChainSettings::default()
        });
        chain.push_far(&[0.1; 960]);
        assert!(chain.echo.is_some());
    }

    #[test]
    fn strength_zero_leaves_the_raw_signal_when_suppression_is_on() {
        let mut chain = CaptureChain::new(ChainSettings {
            noise_suppression: Some(0.0),
            threshold: Threshold::Manual(-80.0),
            ..ChainSettings::default()
        });
        let mut frame = tone(0.25);
        let before = frame.clone();
        chain.process(&mut frame, true);
        for (after, raw) in frame.iter().zip(before.iter()) {
            assert!((after - raw).abs() < 1e-6);
        }
    }

    #[test]
    fn silence_stays_silent_through_the_whole_chain() {
        let mut chain = CaptureChain::new(ChainSettings {
            noise_suppression: Some(STRONG_STRENGTH),
            echo_cancellation: true,
            agc: true,
            ..ChainSettings::default()
        });
        for _ in 0..10 {
            chain.push_far(&[0.0; 960]);
            let mut frame = vec![0.0; 960];
            let report = chain.process(&mut frame, true);
            assert!(frame.iter().all(|s| s.abs() < 1e-6));
            assert!(report.level_db <= -99.0);
        }
    }
}
