//! Capture processing: RNNoise noise suppression, AGC, voice activity, the RFC 6464 audio level,
//! and a normalised LMS echo canceller.
//!
//! All samples are `f32` in the range -1.0 to 1.0 at 48 kHz unless noted.

use std::collections::VecDeque;

use nnnoiseless::DenoiseState;
use tracing::debug;

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
/// The coarse lag search works on block averages of this many samples, which keeps it cheap.
const LAG_DECIMATE: usize = 4;
/// Largest speaker-to-mic delay the lag estimate searches for: 250 ms.
const LAG_MAX: usize = SAMPLE_RATE as usize / 4;
/// Samples between two lag estimates: one second.
const LAG_PERIOD: usize = SAMPLE_RATE as usize;
/// The reference is delayed so the filter window starts this far before the echo, about 16 ms.
const LAG_MARGIN: usize = 768;
/// Normalised correlation the lag estimate needs before the delay changes.
const LAG_MIN_CORR: f32 = 0.3;
/// A new delay further than this from the current one jumps. Smaller changes slew, so the
/// reference never steps: a step looks like an error the filter has to learn again.
const DELAY_JUMP: f64 = 64.0;
/// The reference kept for interpolation: two seconds, far more than any delay needs.
const REFERENCE_KEEP: usize = 2 * SAMPLE_RATE as usize;

/// Normalised LMS echo canceller.
///
/// The filter predicts the echo of the far-end reference, the audio that was played out, from the
/// last `taps` reference samples, and subtracts that prediction from the microphone signal. The
/// tail must cover the longest speaker-to-mic delay the filter has to model, which is the acoustic
/// path plus the device latency. The window is placed by a delay estimate: every second the
/// reference is cross-correlated with the mic, and the reference is read at that delay, so the
/// window starts just before the echo. The delay is fractional: it is refined to a fraction of a
/// sample, and it slews toward each new estimate, so a clock drift between the speaker and the mic
/// moves the reference smoothly instead of stepping it. Adaptation pauses when the reference is
/// quiet, which avoids drifting the filter on silence. It does not detect double talk, so near-end
/// speech during far-end speech slows convergence and can leave some residual.
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
    /// Energy of the window. Kept in f64: a running sum over thousands of taps drifts in f32.
    power: f64,
    /// Reference samples as pushed, the newest last. `far_base` is the index of the first one.
    far_buf: VecDeque<f32>,
    far_base: u64,
    /// Reference samples pushed so far.
    pushed: u64,
    /// Mic samples processed so far. Mic sample `n` is paired with reference position `n - delay`.
    processed: u64,
    /// The reference as it was pushed, for the lag estimate.
    far_log: VecDeque<f32>,
    /// The mic as it was processed, for the lag estimate.
    near_log: VecDeque<f32>,
    /// Samples since the last lag estimate. It starts at a full period, so the first estimate runs
    /// as soon as the logs hold enough audio.
    since_estimate: usize,
    /// The reference delay in samples, fractional. It slews toward `target` by `slope` a sample.
    delay: f64,
    target: f64,
    slope: f64,
    slew_left: usize,
    lag: Option<usize>,
    /// Smoothed powers of the residual suppressor: the error, the echo estimate and the mic.
    res_err: f64,
    res_echo: f64,
    res_mic: f64,
    /// The residual suppressor's gain, 1.0 when it is not suppressing.
    res_gain: f64,
    /// Per-sample smoothing factors of the residual suppressor: power, gain opening and gain closing.
    res_smooth: [f64; 3],
    /// Set by `DISSENT_AEC_TRACE=1`: the canceller logs its state once a second at debug level.
    trace: Option<Trace>,
}

/// Power smoothing of the residual suppressor, about 10 ms.
const RES_SMOOTHING_SECONDS: f64 = 0.01;
/// Gain attack (opening) and release (closing) time constants of the residual suppressor. It opens
/// fast, so near-end speech is not clipped, and closes slowly, so the gain does not pump.
const RES_OPEN_SECONDS: f64 = 0.002;
const RES_CLOSE_SECONDS: f64 = 0.02;
/// The residual suppressor works only when the residual is this far below the echo estimate, a
/// power ratio of 0.1 (-10 dB). Above it the gain is 1.
const RES_THRESHOLD: f64 = 0.1;
/// Lowest gain of the residual suppressor, -20 dB: it removes residue without a dead silence.
const RES_FLOOR: f64 = 0.1;
/// Mean reference power below which the far end counts as silent (about -50 dBFS).
const RES_FAR_ACTIVE: f64 = 1e-5;
/// The mic power over the echo estimate above which near-end speech is present (+1.8 dB).
const RES_NEAR_RATIO: f64 = 1.5;

/// The per-sample smoothing factor of a one-pole filter with time constant `seconds`.
fn smoothing(seconds: f64) -> f64 {
    (-1.0 / (seconds * SAMPLE_RATE as f64)).exp()
}

/// Sums over the current trace second.
#[derive(Default)]
struct Trace {
    mic: f64,
    out: f64,
    ey: f64,
    ee: f64,
    yy: f64,
    samples: usize,
}

const EC_EPS: f32 = 1e-3;
const EC_MIN_REFERENCE_POWER: f64 = 1e-6;

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
            far_buf: VecDeque::with_capacity(REFERENCE_KEEP),
            far_base: 0,
            pushed: 0,
            processed: 0,
            far_log: VecDeque::with_capacity(LAG_WINDOW + LAG_MAX),
            near_log: VecDeque::with_capacity(LAG_WINDOW),
            since_estimate: LAG_PERIOD,
            delay: 0.0,
            target: 0.0,
            slope: 0.0,
            slew_left: 0,
            lag: None,
            res_err: 0.0,
            res_echo: 0.0,
            res_mic: 0.0,
            res_gain: 1.0,
            res_smooth: [RES_SMOOTHING_SECONDS, RES_OPEN_SECONDS, RES_CLOSE_SECONDS].map(smoothing),
            trace: std::env::var("DISSENT_AEC_TRACE").is_ok_and(|v| v == "1").then(Trace::default),
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

    /// The reference delay the filter reads at, in samples, fractional.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// Feeds far-end reference samples, the audio that is being played out.
    pub fn push_far(&mut self, far: &[f32]) {
        self.far_buf.extend(far.iter().copied());
        self.pushed += far.len() as u64;
        while self.far_buf.len() > REFERENCE_KEEP {
            self.far_buf.pop_front();
            self.far_base += 1;
        }
        self.far_log.extend(far.iter().copied());
        while self.far_log.len() > LAG_WINDOW + LAG_MAX {
            self.far_log.pop_front();
        }
    }

    /// The reference at position `r`, a fractional sample index. Catmull-Rom interpolation between
    /// the four samples around it. Positions outside the kept reference read as silence.
    fn far_at(&self, r: f64) -> f32 {
        if r < 0.0 {
            return 0.0;
        }
        let i = r.floor() as i64;
        let f = (r - i as f64) as f32;
        let get = |k: i64| -> f32 {
            if k < 0 || (k as u64) < self.far_base || (k as u64) >= self.pushed {
                0.0
            } else {
                self.far_buf[(k as u64 - self.far_base) as usize]
            }
        };
        let (x0, x1, x2, x3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        x1 + 0.5 * f * (x2 - x0 + f * (2.0 * x0 - 5.0 * x1 + 4.0 * x2 - x3 + f * (3.0 * (x1 - x2) + x3 - x0)))
    }

    /// Removes the predicted echo from `near` in place.
    ///
    /// Each near sample is paired with the reference at its delay. Samples with no reference yet
    /// pass through unchanged.
    pub fn process(&mut self, near: &mut [f32]) {
        for sample in near.iter_mut() {
            self.near_log.push_back(*sample);
            if self.near_log.len() > LAG_WINDOW {
                self.near_log.pop_front();
            }
            if self.slew_left > 0 {
                self.delay += self.slope;
                self.slew_left -= 1;
                if self.slew_left == 0 {
                    self.delay = self.target;
                }
            }
            let far = self.far_at(self.processed as f64 - self.delay);
            let mic_in = *sample;
            self.processed += 1;
            self.push_history(far);
            let echo = self.dot_window();
            let error = *sample - echo;
            if self.power > EC_MIN_REFERENCE_POWER {
                let g = self.mu * error / (self.power as f32 + EC_EPS);
                self.adapt(g);
            }
            let out = self.suppress_residual(error, echo, mic_in);
            *sample = out;
            if let Some(t) = self.trace.as_mut() {
                let (m, e, y) = (mic_in as f64, out as f64, echo as f64);
                t.mic += m * m;
                t.out += e * e;
                t.ey += e * y;
                t.ee += e * e;
                t.yy += y * y;
                t.samples += 1;
            }
        }
        self.since_estimate += near.len();
        self.trace_second();
        if self.since_estimate >= LAG_PERIOD && self.far_log.len() == LAG_WINDOW + LAG_MAX {
            self.since_estimate = 0;
            if let Some(lag) = self.estimate_lag() {
                self.apply_lag(lag);
            }
        }
    }

    /// Cross-correlates the last second of mic with the reference, lag by lag up to [`LAG_MAX`].
    /// A coarse search on decimated blocks finds the peak, and a full-rate search around it refines
    /// the lag to a fraction of a sample. Returns `None` when the peak is too weak.
    fn estimate_lag(&self) -> Option<f64> {
        let max_lag = LAG_MAX;
        if self.far_log.len() < LAG_WINDOW + max_lag || self.near_log.len() < LAG_WINDOW {
            return None;
        }
        let near: Vec<f32> = self.near_log.iter().copied().collect();
        let far: Vec<f32> = self.far_log.iter().copied().collect();
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
        if best.0 < LAG_MIN_CORR {
            return None;
        }
        Some(refine_lag(best.1 * LAG_DECIMATE, &near, &far))
    }

    /// Sets the reference delay for an echo `lag` samples late. The delay is the lag less
    /// [`LAG_MARGIN`]. A small change slews toward it over the next period. A large one, the first
    /// estimate for instance, jumps, and the weights move with it: tap `i` models a delay of
    /// `delay + i`, so a jump of `d` moves tap `i + d` to tap `i`.
    fn apply_lag(&mut self, lag: f64) {
        let first = self.lag.is_none();
        self.lag = Some(lag.round() as usize);
        let target = (lag - LAG_MARGIN as f64).max(0.0);
        let diff = target - self.delay;
        if first || diff.abs() > DELAY_JUMP {
            self.shift_weights(diff.round() as isize);
            self.delay = target;
            self.slew_left = 0;
        } else {
            self.target = target;
            self.slope = diff / LAG_PERIOD as f64;
            self.slew_left = LAG_PERIOD;
        }
    }

    /// Applies the residual suppressor to one error sample and returns the output. The gain drops
    /// only when the far end talks, the mic is no louder than the echo estimate (so no near-end
    /// speech), and the error is well below the echo estimate: then the error is residual echo,
    /// and it is scaled by the square root of the error-to-echo ratio over the threshold.
    fn suppress_residual(&mut self, error: f32, echo: f32, mic: f32) -> f32 {
        let [smooth, open, close] = self.res_smooth;
        let (e, y, m) = (error as f64, echo as f64, mic as f64);
        self.res_err = smooth * self.res_err + (1.0 - smooth) * e * e;
        self.res_echo = smooth * self.res_echo + (1.0 - smooth) * y * y;
        self.res_mic = smooth * self.res_mic + (1.0 - smooth) * m * m;

        let far_active = self.power / self.taps as f64 > RES_FAR_ACTIVE;
        let near_present = self.res_mic > RES_NEAR_RATIO * self.res_echo;
        let target = if !far_active || near_present || self.res_echo < EC_MIN_REFERENCE_POWER {
            1.0
        } else {
            ((self.res_err / self.res_echo) / RES_THRESHOLD).sqrt().clamp(RES_FLOOR, 1.0)
        };
        let rate = if target > self.res_gain { open } else { close };
        self.res_gain += (target - self.res_gain) * (1.0 - rate);
        (e * self.res_gain) as f32
    }

    /// Logs the trace sums once a second of mic audio, then clears them. The echo return loss
    /// enhancement is the mic power over the output power, and the correlation is the error's
    /// correlation with the echo estimate, the statistic a double-talk test would act on.
    fn trace_second(&mut self) {
        let Some(t) = self.trace.as_mut() else { return };
        if t.samples < SAMPLE_RATE as usize {
            return;
        }
        let n = t.samples as f64;
        let erle = 10.0 * (t.mic / t.out.max(1e-30)).log10();
        let out_dbfs = 10.0 * (t.out / n).max(1e-30).log10();
        let corr = t.ey.abs() / (t.ee * t.yy).sqrt().max(1e-30);
        debug!(
            lag = ?self.lag,
            delay = self.delay,
            erle_db = erle,
            out_dbfs,
            correlation = corr,
            "echo canceller"
        );
        *t = Trace::default();
    }

    /// Moves the weights for a delay change of `d` samples, as described on [`EchoCanceller::apply_lag`].
    fn shift_weights(&mut self, d: isize) {
        let taps = self.taps;
        let by = d.unsigned_abs().min(taps);
        if d > 0 {
            self.weights.copy_within(by.., 0);
            self.weights[taps - by..].fill(0.0);
        } else if d < 0 {
            self.weights.copy_within(..taps - by, by);
            self.weights[..by].fill(0.0);
        }
    }

    fn push_history(&mut self, x: f32) {
        // The newest sample lives at `pos`, and older samples follow at increasing offsets.
        let old = self.history[(self.pos + self.taps - 1) % self.taps];
        self.power += (x as f64) * (x as f64) - (old as f64) * (old as f64);
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

/// The full-rate correlation at lags around `coarse`, with a parabola through the peak, so the lag
/// is a fraction of a sample. `near` is the last second of mic, and `far` the reference over the
/// same second plus [`LAG_MAX`] before it. Near sample `t` pairs with far sample `LAG_MAX + t - lag`.
fn refine_lag(coarse: usize, near: &[f32], far: &[f32]) -> f64 {
    let e_near: f64 = near.iter().map(|x| (*x as f64).powi(2)).sum();
    let score = |lag: usize| -> f64 {
        let base = LAG_MAX - lag;
        let (mut corr, mut e_far) = (0.0f64, 0.0f64);
        for (t, n) in near.iter().enumerate() {
            let f = far[base + t] as f64;
            corr += *n as f64 * f;
            e_far += f * f;
        }
        corr / (e_near * e_far).sqrt().max(1e-12)
    };
    let lo = coarse.saturating_sub(LAG_DECIMATE);
    let hi = (coarse + LAG_DECIMATE).min(LAG_MAX);
    let (mut best_lag, mut best) = (coarse.min(LAG_MAX), f64::MIN);
    for lag in lo..=hi {
        let s = score(lag);
        if s > best {
            (best_lag, best) = (lag, s);
        }
    }
    if best_lag == 0 || best_lag == LAG_MAX {
        return best_lag as f64;
    }
    let (a, b, c) = (score(best_lag - 1), best, score(best_lag + 1));
    let denom = a - 2.0 * b + c;
    let offset = if denom < 0.0 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    best_lag as f64 + offset
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
