//! Capture processing: RNNoise noise suppression, AGC, voice activity, the RFC 6464 audio level,
//! and a normalised LMS echo canceller.
//!
//! All samples are `f32` in the range -1.0 to 1.0 at 48 kHz unless noted.

use std::collections::VecDeque;

use nnnoiseless::DenoiseState;

use crate::codec::SAMPLE_RATE;

/// Samples per RNNoise block. RNNoise works on 10 ms blocks at 48 kHz.
pub const DENOISE_BLOCK: usize = nnnoiseless::FRAME_SIZE;

/// RNNoise works on samples scaled to the i16 range.
const RNN_SCALE: f32 = 32_768.0;

/// RNNoise noise suppressor. Feed it 480-sample blocks.
pub struct NoiseSuppressor {
    state: Box<DenoiseState<'static>>,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl NoiseSuppressor {
    /// Creates a suppressor with a fresh RNNoise state.
    pub fn new() -> Self {
        NoiseSuppressor {
            state: DenoiseState::new(),
            input: vec![0.0; DENOISE_BLOCK],
            output: vec![0.0; DENOISE_BLOCK],
        }
    }

    /// Denoises one block in place and returns RNNoise's voice probability, 0.0 to 1.0.
    ///
    /// # Panics
    ///
    /// Panics if `block` is not [`DENOISE_BLOCK`] samples long.
    pub fn process_block(&mut self, block: &mut [f32]) -> f32 {
        assert_eq!(
            block.len(),
            DENOISE_BLOCK,
            "RNNoise needs 480-sample blocks"
        );
        for (dst, src) in self.input.iter_mut().zip(block.iter()) {
            *dst = src * RNN_SCALE;
        }
        let vad = self.state.process_frame(&mut self.output, &self.input);
        for (dst, src) in block.iter_mut().zip(self.output.iter()) {
            *dst = src / RNN_SCALE;
        }
        vad
    }
}

impl Default for NoiseSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for NoiseSuppressor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoiseSuppressor").finish_non_exhaustive()
    }
}

/// Slow automatic gain control for speech.
///
/// It moves the frame RMS toward -20 dBFS, with at most +/-12 dB of gain. Gain falls quickly
/// and rises slowly. Frames below the gate are left alone, so the gain does not climb on silence.
#[derive(Debug, Clone)]
pub struct Agc {
    gain: f32,
    /// Mean square of recent frames above the gate, smoothed over about 200 ms.
    power: f32,
    target_rms: f32,
    min_gain: f32,
    max_gain: f32,
    gate_rms: f32,
}

/// Per-frame smoothing of the level estimate, about 200 ms at 20 ms frames.
const AGC_SMOOTHING: f32 = 0.9;

impl Agc {
    /// Creates an AGC with a -20 dBFS target and a +/-12 dB gain range.
    pub fn new() -> Self {
        Agc {
            gain: 1.0,
            power: 0.0,
            target_rms: db_to_amp(-20.0),
            min_gain: db_to_amp(-12.0),
            max_gain: db_to_amp(12.0),
            gate_rms: db_to_amp(-55.0),
        }
    }

    /// Applies the current gain to `frame`, then updates it from the frame's level.
    pub fn process(&mut self, frame: &mut [f32]) {
        let level = rms(frame);
        if level > self.gate_rms {
            let p = level * level;
            self.power = if self.power == 0.0 {
                p
            } else {
                AGC_SMOOTHING * self.power + (1.0 - AGC_SMOOTHING) * p
            };
        }
        if self.power > 0.0 {
            let wanted = (self.target_rms / self.power.sqrt()).clamp(self.min_gain, self.max_gain);
            let step = if wanted < self.gain { 0.5 } else { 0.05 };
            self.gain += (wanted - self.gain) * step;
        }
        for s in frame.iter_mut() {
            *s = (*s * self.gain).clamp(-1.0, 1.0);
        }
    }

    /// Current linear gain.
    pub fn gain(&self) -> f32 {
        self.gain
    }
}

impl Default for Agc {
    fn default() -> Self {
        Self::new()
    }
}

/// Turns per-block voice probabilities into a voiced/unvoiced decision with hangover.
///
/// Speech stays marked for `hangover` blocks after the probability drops, so word endings are
/// not clipped when DTX takes over.
#[derive(Debug, Clone)]
pub struct VoiceDetector {
    threshold: f32,
    hangover: u32,
    remaining: u32,
}

impl VoiceDetector {
    /// Creates a detector. A probability at or above `threshold` counts as voice.
    pub fn new(threshold: f32, hangover_blocks: u32) -> Self {
        VoiceDetector {
            threshold,
            hangover: hangover_blocks,
            remaining: 0,
        }
    }

    /// Updates with one block's voice probability and returns whether the block is voiced.
    pub fn update(&mut self, probability: f32) -> bool {
        if probability >= self.threshold {
            self.remaining = self.hangover;
            true
        } else if self.remaining > 0 {
            self.remaining -= 1;
            true
        } else {
            false
        }
    }
}

impl Default for VoiceDetector {
    /// 0.5 threshold with 10 blocks (100 ms) of hangover.
    fn default() -> Self {
        Self::new(0.5, 10)
    }
}

/// Audio level as in RFC 6464: the negative of the level in dBov, clamped to 0 to 127.
///
/// 0 is full scale and 127 is silence. Only the 0 to 127 range is meaningful, so values
/// below -127 dBov are clamped.
pub fn audio_level(frame: &[f32]) -> u8 {
    let ms = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    if ms <= 0.0 {
        return 127;
    }
    let dbov = 10.0 * ms.log10();
    (-dbov).round().clamp(0.0, 127.0) as u8
}

/// Normalised LMS echo canceller.
///
/// The filter predicts the echo of the far-end reference, the audio that was played out, from the
/// last `taps` reference samples, and subtracts that prediction from the microphone signal. The
/// tail length must cover the acoustic path plus the device latency. Adaptation pauses when the
/// reference is quiet, which avoids drifting the filter on silence. It does not detect double
/// talk, so near-end speech during far-end speech slows convergence and can leave some residual.
pub struct EchoCanceller {
    taps: usize,
    mu: f32,
    weights: Vec<f32>,
    /// Mirrored ring of reference samples. The newest `taps` samples are always contiguous.
    history: Vec<f32>,
    pos: usize,
    power: f32,
    pending: VecDeque<f32>,
}

const EC_EPS: f32 = 1e-3;
const EC_MIN_REFERENCE_POWER: f32 = 1e-6;

impl EchoCanceller {
    /// Creates a canceller with `taps` filter taps and step size `mu`, typically 0.2 to 0.5.
    pub fn new(taps: usize, mu: f32) -> Self {
        let taps = taps.max(1);
        EchoCanceller {
            taps,
            mu,
            weights: vec![0.0; taps],
            history: vec![0.0; 2 * taps],
            pos: 0,
            power: 0.0,
            pending: VecDeque::new(),
        }
    }

    /// A canceller with a tail of `tail_ms` at 48 kHz, using step size 0.3.
    pub fn with_tail_ms(tail_ms: u32) -> Self {
        let taps = (SAMPLE_RATE as usize * tail_ms as usize) / 1000;
        Self::new(taps, 0.3)
    }

    /// Filter length in samples.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Feeds far-end reference samples, the audio that is being played out.
    pub fn push_far(&mut self, far: &[f32]) {
        self.pending.extend(far.iter().copied());
        // If the capture side stalls, keep only the newest second of reference.
        let max = SAMPLE_RATE as usize;
        while self.pending.len() > max {
            self.pending.pop_front();
        }
    }

    /// Removes the predicted echo from `near` in place.
    ///
    /// Each near sample is paired with the next queued reference sample. Samples with no
    /// reference yet pass through unchanged.
    pub fn process(&mut self, near: &mut [f32]) {
        for sample in near.iter_mut() {
            let far = self.pending.pop_front().unwrap_or(0.0);
            self.push_history(far);
            let echo = self.dot_window();
            let error = *sample - echo;
            if self.power > EC_MIN_REFERENCE_POWER {
                let g = self.mu * error / (self.power + EC_EPS);
                self.adapt(g);
            }
            *sample = error;
        }
    }

    fn push_history(&mut self, x: f32) {
        // The newest sample lives at `pos`, and older samples follow at increasing offsets.
        let old = self.history[(self.pos + self.taps - 1) % self.taps];
        self.power += x * x - old * old;
        if self.power < 0.0 {
            self.power = 0.0;
        }
        self.pos = (self.pos + self.taps - 1) % self.taps;
        self.history[self.pos] = x;
        self.history[self.pos + self.taps] = x;
    }

    fn window(&self) -> &[f32] {
        &self.history[self.pos..self.pos + self.taps]
    }

    fn dot_window(&self) -> f32 {
        self.window()
            .iter()
            .zip(self.weights.iter())
            .map(|(x, w)| x * w)
            .sum()
    }

    fn adapt(&mut self, g: f32) {
        let pos = self.pos;
        let taps = self.taps;
        let window = &self.history[pos..pos + taps];
        for (w, x) in self.weights.iter_mut().zip(window.iter()) {
            *w += g * x;
        }
    }
}

impl std::fmt::Debug for EchoCanceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EchoCanceller")
            .field("taps", &self.taps)
            .field("mu", &self.mu)
            .finish_non_exhaustive()
    }
}

/// Root mean square of a frame, 0.0 for an empty frame.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Converts decibels to a linear amplitude ratio.
pub fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
