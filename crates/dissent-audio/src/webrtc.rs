//! The WebRTC engine: sonora's port of WebRTC's audio processing, with its echo canceller (AEC3), noise suppressor and
//! AGC2. It replaces our echo canceller, RNNoise pass and AGC in [`crate::CaptureChain`] when
//! [`EchoEngine::WebRtc`] is chosen and the `sonora` feature is built. Without the feature this is an empty stub, so
//! the chain keeps the standard engine.

use crate::chain::{ChainSettings, EchoEngine};

/// Sonora works on 10 ms blocks, 480 samples at 48 kHz.
pub const BLOCK: usize = 480;

#[cfg(feature = "sonora")]
mod engine {
    use sonora::config::{AdaptiveDigital, Config, EchoCanceller, GainController2, NoiseSuppression, NoiseSuppressionLevel};
    use sonora::{AudioProcessing, StreamConfig};

    use super::BLOCK;
    use crate::chain::{ChainSettings, STRONG_STRENGTH};

    /// The sonora pipeline for one call, with the settings it was last configured with.
    pub struct Apm {
        apm: AudioProcessing,
        applied: ChainSettings,
        render_out: Vec<f32>,
    }

    /// The sonora configuration for `settings`. The echo canceller follows the echo setting, the noise suppressor
    /// the suppression strength, and AGC2 the AGC setting. Its adaptive digital gain is on only when AGC is.
    pub fn config_for(settings: &ChainSettings) -> Config {
        Config {
            echo_canceller: settings.echo_cancellation.then(EchoCanceller::default),
            noise_suppression: settings.noise_suppression.map(|strength| NoiseSuppression {
                level: if strength >= STRONG_STRENGTH { NoiseSuppressionLevel::High } else { NoiseSuppressionLevel::Moderate },
                analyze_linear_aec_output_when_available: false,
            }),
            gain_controller2: settings.agc.then(|| GainController2 {
                adaptive_digital: Some(AdaptiveDigital::default()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    impl Apm {
        /// A pipeline configured for `settings`, mono at 48 kHz.
        pub fn new(settings: &ChainSettings) -> Self {
            let stream = StreamConfig::new(crate::SAMPLE_RATE, 1);
            let apm = AudioProcessing::builder()
                .config(config_for(settings))
                .capture_config(stream)
                .render_config(stream)
                .build();
            Apm { apm, applied: *settings, render_out: vec![0.0; BLOCK] }
        }

        /// Applies `settings` if they differ from the ones in effect. Only the processing configuration changes, so the
        /// echo state is kept.
        pub fn apply(&mut self, settings: &ChainSettings) {
            if settings.echo_cancellation != self.applied.echo_cancellation
                || settings.noise_suppression != self.applied.noise_suppression
                || settings.agc != self.applied.agc
            {
                self.apm.apply_config(config_for(settings));
                self.applied = *settings;
            }
        }

        /// Feeds the far-end audio that was just played, in 10 ms blocks. Its output is not used.
        pub fn push_far(&mut self, far: &[f32]) {
            for block in far.as_chunks::<BLOCK>().0 {
                self.apm.process_render_f32(&[&block[..]], &mut [&mut self.render_out]).expect("sonora render block");
            }
        }

        /// Processes one mic frame in place, in 10 ms blocks.
        pub fn process(&mut self, frame: &mut [f32]) {
            for block in frame.as_chunks_mut::<BLOCK>().0 {
                let input = *block;
                self.apm.process_capture_f32(&[&input[..]], &mut [&mut block[..]]).expect("sonora capture block");
            }
        }
    }
}

#[cfg(not(feature = "sonora"))]
mod engine {
    use crate::chain::ChainSettings;

    /// Built without the `sonora` feature: there is no pipeline, so the chain never selects this engine.
    pub struct Apm;

    impl Apm {
        pub fn new(_settings: &ChainSettings) -> Self {
            Apm
        }
        pub fn apply(&mut self, _settings: &ChainSettings) {}
        pub fn push_far(&mut self, _far: &[f32]) {}
        pub fn process(&mut self, _frame: &mut [f32]) {}
    }
}

pub use engine::Apm;

/// Whether this build can run the WebRTC engine. Without the `sonora` feature the chain runs the standard engine
/// whatever is asked for.
pub const AVAILABLE: bool = cfg!(feature = "sonora");

/// The engine that runs for `settings` in this build.
pub fn engine_for(settings: &ChainSettings) -> EchoEngine {
    match settings.engine {
        EchoEngine::WebRtc if AVAILABLE => EchoEngine::WebRtc,
        _ => EchoEngine::Standard,
    }
}
