//! Shared test helpers: a deterministic RNG, synthetic signals and signal measures.
#![allow(dead_code)]

use std::f32::consts::TAU;

/// Deterministic xorshift RNG, so every test run sees the same jitter and loss pattern.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f32 {
        (self.unit() * 2.0 - 1.0) as f32
    }
}

/// A sine sweep from 100 Hz to 4 kHz over `n` samples at 48 kHz, plus white noise at roughly
/// -40 dBFS. It stands in for voice: it has a moving pitch and a wideband noise floor.
pub fn sweep_plus_noise(n: usize, rng: &mut Rng) -> Vec<f32> {
    let sr = 48_000.0f32;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let f = 100.0 + (4000.0 - 100.0) * t;
            phase += TAU * f / sr;
            0.3 * phase.sin() + 0.01 * rng.bipolar()
        })
        .collect()
}

/// Harmonic "voice": a 130 Hz fundamental with a slow vibrato, 20 harmonics, and syllable-like
/// amplitude bursts at about 3 Hz, plus low-level noise.
pub fn voice_like(n: usize, rng: &mut Rng) -> Vec<f32> {
    let sr = 48_000.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let f0 = 130.0 * (1.0 + 0.05 * (TAU * 5.0 * t).sin());
            let mut v = 0.0f32;
            for k in 1..=20 {
                v += (TAU * f0 * k as f32 * t).sin() / k as f32;
            }
            let burst = 0.5 + 0.5 * (TAU * 3.0 * t).sin();
            0.25 * burst * v + 0.003 * rng.bipolar()
        })
        .collect()
}

/// Mean power of a signal.
pub fn power(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / x.len() as f64
}

/// Signal-to-noise ratio of `test` against `reference`, both the same length, in dB.
pub fn snr_db(reference: &[f32], test: &[f32]) -> f64 {
    let n = reference.len().min(test.len());
    let sig: f64 = reference[..n].iter().map(|&s| f64::from(s).powi(2)).sum();
    let err: f64 = reference[..n]
        .iter()
        .zip(test[..n].iter())
        .map(|(&r, &t)| f64::from(r - t).powi(2))
        .sum();
    10.0 * (sig.max(1e-30) / err.max(1e-30)).log10()
}

/// Finds the delay of `y` relative to `x` (y[t] ~ x[t - lag]) by correlating a window in the
/// middle of the signals over lags 0 to `max_lag`.
pub fn best_lag(x: &[f32], y: &[f32], max_lag: usize) -> usize {
    let n = x.len().min(y.len());
    let win = 19_200.min(n / 2);
    let start = (n - win) / 2;
    let mut best = (f64::MIN, 0usize);
    for lag in 0..=max_lag {
        let mut dot = 0.0f64;
        for t in start..start + win {
            dot += f64::from(x[t - lag]) * f64::from(y[t]);
        }
        if dot > best.0 {
            best = (dot, lag);
        }
    }
    best.1
}

/// Aligns `y` to `x` by the best lag and returns `(lag, snr_db)` over the overlap.
pub fn aligned_snr(x: &[f32], y: &[f32], max_lag: usize) -> (usize, f64) {
    let lag = best_lag(x, y, max_lag);
    let n = x.len().min(y.len());
    (lag, snr_db(&x[..n - lag], &y[lag..n]))
}
