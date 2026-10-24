//! Streaming linear resampler between the device rate and 48 kHz.
//!
//! Linear interpolation is cheap and has no latency, but it does not filter. Downsampling lets
//! some energy above the new Nyquist frequency alias into the band. For voice this is acceptable,
//! and devices almost always run at 48 kHz already, so the resampler does little work.

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
}

impl LinearResampler {
    /// Creates a resampler from `from_hz` to `to_hz`.
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        LinearResampler {
            step: f64::from(from_hz) / f64::from(to_hz.max(1)),
            prev: 0.0,
            pos: 1.0,
            identity: from_hz == to_hz,
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
}
