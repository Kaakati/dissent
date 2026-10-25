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
        self.process_while(frame, true);
    }

    /// Like [`Agc::process`], but the level and gain adapt only while `adapt` is true. The gain
    /// still applies. The chain passes whether its gate is open, so the gain does not climb on
    /// room noise the gate is about to silence.
    pub fn process_while(&mut self, frame: &mut [f32], adapt: bool) {
        if adapt {
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
                let wanted =
                    (self.target_rms / self.power.sqrt()).clamp(self.min_gain, self.max_gain);
                let step = if wanted < self.gain { 0.5 } else { 0.05 };
                self.gain += (wanted - self.gain) * step;
            }
        }
        for s in frame.iter_mut() {
            *s *= self.gain;
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
/// not cut off when the frames turn to silence.
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
    /// 0.5 threshold with 10 frames (200 ms) of hangover. The sender calls it once per 20 ms frame.
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

/// Samples of mic audio the lag estimate uses: one second.
const LAG_WINDOW: usize = SAMPLE_RATE as usize;
/// The lag estimate works on block averages of this many samples, which keeps it cheap.
const LAG_DECIMATE: usize = 4;
/// Largest speaker-to-mic delay the lag estimate searches for: 250 ms.
const LAG_MAX: usize = SAMPLE_RATE as usize / 4;
/// Samples between two lag estimates: one second.
const LAG_PERIOD: usize = SAMPLE_RATE as usize;
/// The reference is delayed so the filter window starts this far before the echo, about 16 ms.
const LAG_MARGIN: usize = 768;
/// Normalised correlation the lag estimate needs before the reference shift changes.
const LAG_MIN_CORR: f32 = 0.3;
/// A new shift this close to the current one is ignored, so jitter does not reset the filter.
const SHIFT_HYSTERESIS: usize = 480;

/// Normalised LMS echo canceller.
///
/// The filter predicts the echo of the far-end reference, the audio that was played out, from the
/// last `taps` reference samples, and subtracts that prediction from the microphone signal. The
/// tail must cover the longest speaker-to-mic delay the filter has to model, which is the acoustic
/// path plus the device latency. The window is placed by a delay estimate: every second the
/// reference is cross-correlated with the mic, and the reference queue is set to that delay so the
/// window starts just before the echo. Adaptation pauses when the reference is quiet, which
/// avoids drifting the filter on silence. It does not detect double talk, so near-end speech during
/// far-end speech slows convergence and can leave some residual.
///
/// Push the reference before processing the mic samples it echoes, on every tick. The desktop loop
/// pushes each tick's playout before that tick's capture.
pub struct EchoCanceller {
    taps: usize,
    mu: f32,
    weights: Vec<f32>,
    /// Mirrored ring of reference samples. The newest `taps` samples are always contiguous.
    history: Vec<f32>,
    pos: usize,
    power: f32,
    pending: VecDeque<f32>,
    /// The reference as it was pushed, before any delay, for the lag estimate.
    far_log: VecDeque<f32>,
    /// The mic as it was processed, for the lag estimate.
    near_log: VecDeque<f32>,
    /// Samples since the last lag estimate. It starts at a full period, so the first estimate runs
    /// as soon as the logs hold enough audio.
    since_estimate: usize,
    lag: Option<usize>,
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
            far_log: VecDeque::with_capacity(LAG_WINDOW + LAG_MAX),
            near_log: VecDeque::with_capacity(LAG_WINDOW),
            since_estimate: LAG_PERIOD,
            lag: None,
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

    /// The speaker-to-mic delay found by the last estimate, in samples, or `None` before one
    /// succeeds.
    pub fn lag_estimate(&self) -> Option<usize> {
        self.lag
    }

    /// Feeds far-end reference samples, the audio that is being played out.
    pub fn push_far(&mut self, far: &[f32]) {
        self.pending.extend(far.iter().copied());
        self.far_log.extend(far.iter().copied());
        while self.far_log.len() > LAG_WINDOW + LAG_MAX {
            self.far_log.pop_front();
        }
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
            self.near_log.push_back(*sample);
            if self.near_log.len() > LAG_WINDOW {
                self.near_log.pop_front();
            }
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
        self.since_estimate += near.len();
        if self.since_estimate >= LAG_PERIOD && self.far_log.len() == LAG_WINDOW + LAG_MAX {
            self.since_estimate = 0;
            if let Some(lag) = self.estimate_lag() {
                self.apply_lag(lag);
            }
        }
    }

    /// Cross-correlates the last second of mic with the reference, lag by lag up to [`LAG_MAX`].
    /// Returns the lag with the best normalised correlation, if that is strong enough.
    fn estimate_lag(&self) -> Option<usize> {
        let max_lag = LAG_MAX;
        if self.far_log.len() < LAG_WINDOW + max_lag || self.near_log.len() < LAG_WINDOW {
            return None;
        }
        let near: Vec<f32> = self.near_log.iter().copied().collect();
        let skip = self.far_log.len() - (LAG_WINDOW + max_lag);
        let far: Vec<f32> = self.far_log.iter().skip(skip).copied().collect();
        let near_d = block_means(&near);
        let far_d = block_means(&far);
        let wd = near_d.len();
        let md = max_lag / LAG_DECIMATE;
        let e_near: f32 = near_d.iter().map(|x| x * x).sum();
        let mut best = (0.0f32, 0usize);
        for l in 0..=md {
            let window = &far_d[md - l..md - l + wd];
            let corr: f32 = near_d.iter().zip(window).map(|(a, b)| a * b).sum();
            let e_far: f32 = window.iter().map(|x| x * x).sum();
            let rho = corr / (e_near * e_far).sqrt().max(1e-12);
            if rho > best.0 {
                best = (rho, l);
            }
        }
        (best.0 >= LAG_MIN_CORR).then_some(best.1 * LAG_DECIMATE)
    }

    /// Places the filter window for an echo `lag` samples late. The reference queue holds the
    /// delay between a reference sample and the mic sample it pairs with, so the queue is sized to
    /// the wanted delay. A large change resets the weights, since they describe the old window.
    fn apply_lag(&mut self, lag: usize) {
        self.lag = Some(lag);
        let wanted = lag.saturating_sub(LAG_MARGIN);
        if wanted.abs_diff(self.pending.len()) < SHIFT_HYSTERESIS {
            return;
        }
        while self.pending.len() < wanted {
            self.pending.push_front(0.0);
        }
        while self.pending.len() > wanted {
            self.pending.pop_front();
        }
        self.weights.fill(0.0);
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

/// Averages `x` in blocks of [`LAG_DECIMATE`] samples. A trailing partial block is dropped.
fn block_means(x: &[f32]) -> Vec<f32> {
    x.as_chunks::<LAG_DECIMATE>()
        .0
        .iter()
        .map(|c| c.iter().sum::<f32>() / LAG_DECIMATE as f32)
        .collect()
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

/// Multiplies every sample by `gain` and clamps to -1.0 to 1.0. A gain of 1.0 leaves the frame
/// unchanged.
pub fn apply_gain(frame: &mut [f32], gain: f32) {
    for s in frame.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

/// Where the soft limiter starts to bend, about -2 dBFS.
const LIMIT_KNEE: f32 = 0.8;

/// Soft limiter. Samples up to [`LIMIT_KNEE`] pass unchanged. Above it the curve bends smoothly
/// toward full scale, so a sample is never larger than 1.0 in magnitude. Unlike a hard clamp, it
/// adds no flat tops, so a peak above full scale rounds off instead of clicking.
pub fn soft_limit(frame: &mut [f32]) {
    for s in frame.iter_mut() {
        let a = s.abs();
        if a > LIMIT_KNEE {
            let over = (a - LIMIT_KNEE) / (1.0 - LIMIT_KNEE);
            *s = (LIMIT_KNEE + (1.0 - LIMIT_KNEE) * over.tanh()).copysign(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_gain_leaves_the_frame_alone() {
        let mut frame = vec![0.25, -0.5, 0.0];
        apply_gain(&mut frame, 1.0);
        assert_eq!(frame, vec![0.25, -0.5, 0.0]);
    }

    #[test]
    fn gain_scales_and_clamps() {
        let mut frame = vec![0.25, -0.6, 0.0];
        apply_gain(&mut frame, 2.0);
        assert_eq!(frame, vec![0.5, -1.0, 0.0]);
        apply_gain(&mut frame, 0.0);
        assert!(frame.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn agc_pulls_loud_speech_down_and_stays_put_on_silence() {
        let mut agc = Agc::new();
        let loud: Vec<f32> = (0..960).map(|i| 0.5 * (i as f32 * 0.3).sin()).collect();
        for _ in 0..200 {
            let mut frame = loud.clone();
            agc.process(&mut frame);
        }
        assert!(agc.gain() < 0.5, "gain {} should drop for loud input", agc.gain());

        let mut quiet_agc = Agc::new();
        for _ in 0..200 {
            let mut frame = vec![0.0; 960];
            quiet_agc.process(&mut frame);
        }
        assert_eq!(quiet_agc.gain(), 1.0, "silence must not raise the gain");
    }

    #[test]
    fn agc_raises_quiet_speech_within_its_range() {
        let mut agc = Agc::new();
        let quiet: Vec<f32> = (0..960).map(|i| 0.02 * (i as f32 * 0.3).sin()).collect();
        for _ in 0..400 {
            let mut frame = quiet.clone();
            agc.process(&mut frame);
        }
        assert!(agc.gain() > 1.5, "gain {} should rise for quiet input", agc.gain());
        assert!(agc.gain() <= db_to_amp(12.0) + 1e-3);
    }
}
