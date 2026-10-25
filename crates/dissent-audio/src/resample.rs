//! Streaming resampler between the device rate and 48 kHz.
//!
//! When the rate drops, a windowed-sinc low-pass runs first, so energy above the new Nyquist
//! frequency is stopped before it can alias into the band. Then linear interpolation picks the
//! output samples. Linear interpolation is cheap and has no latency of its own. The low-pass adds
//! a fixed delay of half its length, about 0.65 ms at 48 kHz. Upsampling needs no filter. Devices
//! almost always run at 48 kHz already, so the resampler does little work.

/// Taps in the anti-alias low-pass. Odd, so the filter is symmetric about one sample.
const LOWPASS_TAPS: usize = 63;

/// Passband edge of the low-pass, as a fraction of the output rate. The Hamming-windowed sinc
/// stops energy from about 0.5 of the output rate, which is the output Nyquist.
const LOWPASS_EDGE: f64 = 0.45;

/// Windowed-sinc FIR low-pass, with state across chunks.
#[derive(Debug, Clone)]
struct LowPass {
    taps: Vec<f32>,
    /// The last `LOWPASS_TAPS - 1` input samples of the previous chunk.
    history: Vec<f32>,
}

impl LowPass {
    /// A low-pass whose edge is `cutoff_hz` at an input rate of `input_hz`. Unity gain at DC.
    fn new(cutoff_hz: f64, input_hz: f64) -> Self {
        let fc = cutoff_hz / input_hz;
        let mid = (LOWPASS_TAPS - 1) as f64 / 2.0;
        let mut taps: Vec<f64> = (0..LOWPASS_TAPS)
            .map(|n| {
                let x = n as f64 - mid;
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
                };
                let window = 0.54
                    - 0.46 * (2.0 * std::f64::consts::PI * n as f64 / (LOWPASS_TAPS - 1) as f64).cos();
                sinc * window
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        for t in &mut taps {
            *t /= sum;
        }
        LowPass {
            taps: taps.into_iter().map(|t| t as f32).collect(),
            history: vec![0.0; LOWPASS_TAPS - 1],
        }
    }

    /// Filters `input`, using and updating the history from the previous chunk.
    fn apply(&mut self, input: &[f32]) -> Vec<f32> {
        let len = input.len();
        let mut buf = Vec::with_capacity(LOWPASS_TAPS - 1 + len);
        buf.extend_from_slice(&self.history);
        buf.extend_from_slice(input);
        // The newest sample of output `i` is `buf[LOWPASS_TAPS - 1 + i]`.
        let out = (0..len)
            .map(|i| {
                let newest = LOWPASS_TAPS - 1 + i;
                (0..LOWPASS_TAPS).map(|k| self.taps[k] * buf[newest - k]).sum()
            })
            .collect();
        self.history = buf[len..].to_vec();
        out
    }
}

/// Converts a stream from one sample rate to another, one chunk at a time, with state.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    /// Input samples per output sample.
    step: f64,
    /// Last input sample of the previous chunk. Index 0 of the working buffer.
    prev: f32,
    /// Read position in the working buffer, where index 0 is `prev`.
    pos: f64,
    identity: bool,
    /// Anti-alias filter, present only when the rate drops.
    lowpass: Option<LowPass>,
}

impl LinearResampler {
    /// Creates a resampler from `from_hz` to `to_hz`.
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        let to = f64::from(to_hz.max(1));
        let from = f64::from(from_hz);
        LinearResampler {
            step: from / to,
            prev: 0.0,
            pos: 1.0,
            identity: from_hz == to_hz,
            lowpass: (from_hz > to_hz).then(|| LowPass::new(LOWPASS_EDGE * to, from)),
        }
    }

    /// True when input and output rates match, so samples pass through unchanged.
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// Resamples `input` and appends the result to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.identity {
            out.extend_from_slice(input);
            return;
        }
        if input.is_empty() {
            return;
        }
        let filtered;
        let input = match self.lowpass.as_mut() {
            Some(lowpass) => {
                filtered = lowpass.apply(input);
                &filtered[..]
            }
            None => input,
        };
        let mut buf = Vec::with_capacity(input.len() + 1);
        buf.push(self.prev);
        buf.extend_from_slice(input);
        let limit = (buf.len() - 1) as f64;
        let mut pos = self.pos;
        while pos < limit {
            let i = pos.floor() as usize;
            let f = (pos - i as f64) as f32;
            out.push(buf[i] * (1.0 - f) + buf[i + 1] * f);
            pos += self.step;
        }
        // The last buffer sample becomes index 0 of the next chunk.
        self.pos = pos - limit;
        self.prev = buf[buf.len() - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::LinearResampler;

    fn tone(freq: f32, rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|t| 0.5 * (std::f32::consts::TAU * freq * t as f32 / rate).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn identity_passes_through() {
        let mut r = LinearResampler::new(48_000, 48_000);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2, 0.3], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn downsample_count_matches_rate_over_many_chunks() {
        let mut r = LinearResampler::new(48_000, 16_000);
        let mut out = Vec::new();
        let input = vec![0.5f32; 480];
        for _ in 0..100 {
            r.process(&input, &mut out);
        }
        // 48000 input samples at 3:1 is 16000 output samples, within one sample of rounding.
        assert!((out.len() as i64 - 16_000).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn upsample_keeps_a_constant_signal_constant() {
        let mut r = LinearResampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            r.process(&[0.25f32; 441], &mut out);
        }
        assert!(out.iter().skip(2).all(|s| (s - 0.25).abs() < 1e-6));
        let expected = 4410 * 48_000 / 44_100;
        assert!(
            (out.len() as i64 - expected as i64).abs() <= 2,
            "got {}",
            out.len()
        );
    }

    #[test]
    fn downsampling_stops_a_tone_above_the_new_nyquist() {
        // A 30 kHz tone is above the 24 kHz Nyquist of 48 kHz. Without the low-pass it folds back
        // to 18 kHz at full level.
        let x = tone(30_000.0, 96_000.0, 96_000);
        let mut r = LinearResampler::new(96_000, 48_000);
        let mut y = Vec::new();
        r.process(&x, &mut y);
        let tail = &y[2_000..];
        let out_db = 20.0 * rms(tail).max(1e-12).log10();
        println!("30 kHz tone at 0.5 after 96 -> 48 kHz: {out_db:.1} dBFS rms");
        assert!(out_db < -60.0, "alias at {out_db:.1} dBFS rms");
    }

    #[test]
    fn downsampling_keeps_an_in_band_tone_at_its_level() {
        let x = tone(1_000.0, 96_000.0, 96_000);
        let mut r = LinearResampler::new(96_000, 48_000);
        let mut y = Vec::new();
        r.process(&x, &mut y);
        let gain_db = 20.0 * (rms(&y[2_000..]) / rms(&tone(1_000.0, 48_000.0, 48_000)[2_000..])).log10();
        assert!(gain_db.abs() < 0.1, "1 kHz gain {gain_db:.3} dB");
    }

    #[test]
    fn chunked_and_whole_inputs_resample_the_same() {
        let x = tone(3_000.0, 48_000.0, 48_000);
        let mut whole = LinearResampler::new(48_000, 44_100);
        let mut all = Vec::new();
        whole.process(&x, &mut all);
        let mut chunked = LinearResampler::new(48_000, 44_100);
        let mut parts = Vec::new();
        for chunk in x.chunks(960) {
            chunked.process(chunk, &mut parts);
        }
        assert_eq!(all.len(), parts.len());
        assert!(all.iter().zip(&parts).all(|(a, b)| (a - b).abs() < 1e-6));
    }
}
