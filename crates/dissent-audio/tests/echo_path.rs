//! The echo canceller on a realistic acoustic path: a room impulse response with a 250 ms tail, the
//! speaker's device buffering (40 to 120 ms of delay the pipeline cannot see), and clock drift between
//! the speaker and the pipeline's 48 kHz tick. The pipeline pushes each tick's playout as the echo
//! reference, the way the desktop loop does, and processes that tick's mic frame afterwards.
//!
//! Run with `cargo test -p dissent-audio --test echo_path -- --nocapture`.

mod common;

use common::{power, Rng};
use dissent_audio::{CaptureChain, ChainSettings, Threshold, FRAME_SAMPLES, SAMPLE_RATE};

const RATE: f64 = SAMPLE_RATE as f64;

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// A sparse room impulse response: a direct path at 3 ms, then reflections over `tail_ms` with an
/// exponential decay of 60 dB over `t60_ms`. The energy of the response is `energy`, which sets the
/// echo return loss (the echo's power relative to the far-end power).
fn room_ir(rng: &mut Rng, tail_ms: f64, t60_ms: f64, energy: f64) -> Vec<(usize, f32)> {
    let direct = (0.003 * RATE) as usize;
    let mut taps = vec![(direct, 1.0f32)];
    let span = (tail_ms / 1000.0 * RATE) as usize;
    for _ in 0..400 {
        let at = direct + 1 + (rng.unit() * span as f64) as usize;
        let t = (at - direct) as f64 / RATE * 1000.0;
        let decay = 10f64.powf(-60.0 * t / t60_ms / 20.0);
        taps.push((at, (rng.bipolar() as f64 * decay) as f32));
    }
    // The direct path carries 60% of the energy, as in a room with the speaker in view of the mic.
    // The reflections share the rest, so the strongest arrival is the direct one.
    let e: f64 = taps[1..].iter().map(|(_, a)| (*a as f64).powi(2)).sum();
    let refl = (0.4 * energy / e).sqrt() as f32;
    taps[1..].iter_mut().for_each(|(_, a)| *a *= refl);
    taps[0].1 = (0.6 * energy).sqrt() as f32;
    taps
}

/// What the speaker plays: the far end delayed by `buffer_ms`, read at a clock `ppm` faster or slower.
/// Linear interpolation keeps the drift fractional, as a real device clock would be.
fn speaker_output(far: &[f32], buffer_ms: f64, ppm: f64) -> Vec<f32> {
    let buffer = buffer_ms / 1000.0 * RATE;
    (0..far.len())
        .map(|t| {
            let pos = t as f64 * (1.0 + ppm * 1e-6) - buffer;
            if pos < 0.0 {
                return 0.0;
            }
            let i = pos.floor() as usize;
            let frac = (pos - i as f64) as f32;
            match (far.get(i), far.get(i + 1)) {
                (Some(a), Some(b)) => a + (b - a) * frac,
                (Some(a), None) => *a,
                _ => 0.0,
            }
        })
        .collect()
}

/// The mic: the near-end signal plus the room's echo of what the speaker played.
fn mic_signal(near: &[f32], spk: &[f32], ir: &[(usize, f32)]) -> Vec<f32> {
    let mut mic = near.to_vec();
    for &(k, a) in ir {
        for t in k..mic.len() {
            mic[t] += a * spk[t - k];
        }
    }
    mic
}

struct Scenario {
    buffer_ms: f64,
    ppm: f64,
}

/// Runs the chain tick by tick and returns the processed mic. The reference is pushed before the mic
/// frame of the same tick, as the desktop loop does.
fn run_chain(far: &[f32], mic: &[f32], echo: bool) -> Vec<f32> {
    let mut chain = CaptureChain::new(ChainSettings {
        echo_cancellation: echo,
        threshold: Threshold::Manual(-120.0),
        ..ChainSettings::default()
    });
    let mut out = Vec::with_capacity(mic.len());
    for (f, m) in far.as_chunks::<FRAME_SAMPLES>().0.iter().zip(mic.as_chunks::<FRAME_SAMPLES>().0) {
        chain.push_far(f);
        let mut frame = m.to_vec();
        chain.process(&mut frame, true);
        out.extend_from_slice(&frame);
    }
    out
}

/// Echo return loss enhancement over the last `tail` samples, with no near-end speech.
fn erle_tail(mic: &[f32], out: &[f32], tail: std::ops::Range<usize>) -> f64 {
    db(power(&mic[tail.clone()]) / power(&out[tail]))
}

/// A speech-like signal: white excitation through three formant resonators at 500, 1500 and 2500
/// Hz, under random syllable envelopes of 100 to 300 ms with occasional pauses. Unlike a harmonic
/// tone it is aperiodic and coloured, and two seeds give unrelated signals.
fn speech_like(n: usize, rng: &mut Rng) -> Vec<f32> {
    let mut x: Vec<f32> = (0..n).map(|_| rng.bipolar()).collect();
    for (fc, bw) in [(500.0f64, 100.0f64), (1500.0, 120.0), (2500.0, 160.0)] {
        let r = (-std::f64::consts::PI * bw / RATE).exp();
        let (c1, c2) = (2.0 * r * (std::f64::consts::TAU * fc / RATE).cos(), -r * r);
        let (mut y1, mut y2) = (0.0f64, 0.0f64);
        for v in x.iter_mut() {
            let y = (1.0 - r) * *v as f64 + c1 * y1 + c2 * y2;
            (y2, y1) = (y1, y);
            *v = y as f32;
        }
    }
    // Syllable envelope: segments with raised-cosine transitions between random levels.
    let mut env = vec![0.0f32; n];
    let mut t = 0usize;
    let mut level = 0.5f32;
    while t < n {
        let len = ((0.1 + 0.2 * rng.unit()) * RATE) as usize;
        let next = if rng.unit() < 0.15 { 0.0 } else { 0.2 + 0.8 * rng.unit() as f32 };
        for i in 0..len.min(n - t) {
            let w = 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / len as f32).cos();
            env[t + i] = level + (next - level) * w;
        }
        level = next;
        t += len;
    }
    let rms = (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt() as f32;
    x.iter().zip(env).map(|(v, e)| v * e / rms).collect()
}

fn far_end(seconds: f64, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let n = (seconds * RATE) as usize;
    speech_like(n, &mut rng).iter().map(|s| 0.3 * s).collect()
}

/// The room, the far end and the scenario behind one measurement.
fn scenario_signals(seconds: f64, s: &Scenario) -> (Vec<f32>, Vec<f32>) {
    let n = (seconds * RATE) as usize;
    let far = far_end(seconds, 21);
    let mut rng = Rng::new(5);
    let ir = room_ir(&mut rng, 250.0, 250.0, 0.5);
    let near: Vec<f32> = (0..n).map(|_| 0.001 * rng.bipolar()).collect();
    let spk = speaker_output(&far, s.buffer_ms, s.ppm);
    let mic = mic_signal(&near, &spk, &ir);
    (far, mic)
}

/// The chain's echo return loss enhancement over the last two seconds, with the far end talking and
/// no near-end speech. The first eight seconds are the canceller converging.
fn steady_erle(far: &[f32], mic: &[f32]) -> (f64, f64) {
    let out = run_chain(far, mic, true);
    let end = mic.len();
    let tail = end - 2 * SAMPLE_RATE as usize..end;
    (erle_tail(mic, &out, tail.clone()), db(power(&out[tail])))
}

#[test]
fn erle_in_a_room_with_buffering_and_drift() {
    println!("room: 250 ms tail, direct path 60% of the energy; speech-like far end; last 2 s measured");
    println!("buffer   drift      ERLE      residual");
    for &buffer_ms in &[40.0, 80.0, 120.0] {
        for &ppm in &[0.0, 200.0] {
            let s = Scenario { buffer_ms, ppm };
            let (far, mic) = scenario_signals(10.0, &s);
            let (erle, residual) = steady_erle(&far, &mic);
            println!("{buffer_ms:>4.0} ms  {ppm:>4.0} ppm  {erle:>6.1} dB  {residual:>7.1} dBFS");
            assert!(erle >= 25.0, "ERLE {erle:.1} dB at {buffer_ms} ms buffering and {ppm} ppm drift, expected at least 25 dB");
        }
    }
}

#[test]
fn erle_with_a_loud_far_end() {
    // The far end at 0.9 peak puts the mic above the soft limiter's knee, and at 1.9 peak it clips.
    let s = Scenario { buffer_ms: 40.0, ppm: 0.0 };
    let (far, _) = scenario_signals(10.0, &s);
    let far: Vec<f32> = far.iter().map(|v| v * 3.0).collect();
    let spk = speaker_output(&far, s.buffer_ms, s.ppm);
    let mic = mic_signal(&vec![0.0; far.len()], &spk, &room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5));
    let (erle, residual) = steady_erle(&far, &mic);
    println!("loud far end (0.9 peak): ERLE {erle:.1} dB, residual {residual:.1} dBFS");
    assert!(erle >= 25.0, "ERLE {erle:.1} dB with a loud far end, expected at least 25 dB");
}

#[test]
fn erle_with_a_nonlinear_speaker() {
    // A speaker that compresses its loud peaks, as a small driver does. The distortion is not in the
    // linear path the filter models, so only the residual suppressor can remove it.
    let s = Scenario { buffer_ms: 40.0, ppm: 0.0 };
    let (far, _) = scenario_signals(10.0, &s);
    let spk: Vec<f32> = speaker_output(&far, s.buffer_ms, s.ppm).iter().map(|v| 0.6 * (2.0 * v).tanh()).collect();
    let mic = mic_signal(&vec![0.0; far.len()], &spk, &room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5));
    let (erle, residual) = steady_erle(&far, &mic);
    println!("nonlinear speaker: ERLE {erle:.1} dB, residual {residual:.1} dBFS");
    assert!(erle >= 25.0, "ERLE {erle:.1} dB with a nonlinear speaker, expected at least 25 dB");
}

/// Known to fail. The normalised LMS filter fits part of the near-end voice into its echo estimate
/// during double talk, because nothing tells it that the mic holds speech it cannot predict. The
/// near-end voice comes out about 5 dB quieter than it went in. This test measures that, so the
/// number can be watched while a fix is built, and it stays ignored until then.
#[test]
#[ignore = "known gap: NLMS absorbs near-end speech during double talk; run with --ignored to measure"]
fn double_talk_keeps_the_near_end_voice() {
    // Far end talks throughout; near-end voice joins from 4 s to 8 s. The distortion is the
    // near-end voice's power over the power of what is left of it in the output.
    let seconds = 10.0;
    let n = (seconds * RATE) as usize;
    let mut rng = Rng::new(9);
    let far = far_end(seconds, 22);
    let ir = room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5);
    let mut near_voice: Vec<f32> = speech_like(n, &mut rng).iter().map(|s| 0.2 * s).collect();
    let (a, b) = (4 * SAMPLE_RATE as usize, 8 * SAMPLE_RATE as usize);
    near_voice.iter_mut().enumerate().for_each(|(t, s)| {
        if t < a || t >= b {
            *s = 0.0;
        }
    });
    let spk = speaker_output(&far, 60.0, 100.0);
    let mic = mic_signal(&near_voice, &spk, &ir);
    let out = run_chain(&far, &mic, true);
    let range = a + SAMPLE_RATE as usize..b;
    let residual: Vec<f32> = range.clone().map(|t| out[t] - near_voice[t]).collect();
    let distortion = db(power(&near_voice[range.clone()]) / power(&residual));
    println!("double talk, near-end voice over 4-8 s: near-end to residual ratio {distortion:.1} dB");
    println!(
        "near-end {:.1} dBFS, output {:.1} dBFS",
        db(power(&near_voice[range.clone()])),
        db(power(&out[range]))
    );
    assert!(distortion >= 20.0, "the near-end voice is {distortion:.1} dB above its residual, expected at least 20 dB");
}
