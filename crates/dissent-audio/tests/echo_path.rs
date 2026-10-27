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

/// The chain with `settings`, pushing the reference tick by tick as the desktop loop does.
fn run_with(far: &[f32], mic: &[f32], settings: ChainSettings) -> Vec<f32> {
    let mut chain = CaptureChain::new(settings);
    let mut out = Vec::with_capacity(mic.len());
    for (f, m) in far.as_chunks::<FRAME_SAMPLES>().0.iter().zip(mic.as_chunks::<FRAME_SAMPLES>().0) {
        chain.push_far(f);
        let mut frame = m.to_vec();
        chain.process(&mut frame, true);
        out.extend_from_slice(&frame);
    }
    out
}

/// Measurement, not a gate: counts 20 ms blocks of double talk where the output is louder than the mic.
/// The output is the mic minus the echo estimate, scaled by the residual gain, so it can only exceed the mic
/// when the estimate adds energy the mic does not have. That is a diverging filter.
#[test]
#[ignore = "measurement: run with --ignored --nocapture"]
fn measure_double_talk_output_above_mic() {
    let seconds = 12.0;
    let n = (seconds * RATE) as usize;
    let far = far_end(seconds, 31);
    let near: Vec<f32> = speech_like(n, &mut Rng::new(33)).iter().map(|s| 0.2 * s).collect();
    let spk = speaker_output(&far, 60.0, 100.0);
    let mic = mic_signal(&near, &spk, &room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5));
    let block = FRAME_SAMPLES;
    let start = 2 * SAMPLE_RATE as usize;
    let variants = [
        ("AEC only", ChainSettings { noise_suppression: None, agc: false, echo_cancellation: true, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() }),
        ("default chain", ChainSettings { echo_cancellation: true, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() }),
    ];
    for (name, settings) in variants {
        let out = run_with(&far, &mic, settings);
        // Double talk: near voice and far echo both present in the block.
        let (mut blocks, mut over0, mut over3, mut over6, mut worst) = (0usize, 0usize, 0usize, 0usize, f64::MIN);
        let (mut near_blocks, mut near_loss) = (0usize, 0.0f64);
        for b in (start..n - block).step_by(block) {
            let range = b..b + block;
            let near_p = power(&near[range.clone()]);
            let echo_p = power(&spk[range.clone()]);
            let mic_p = power(&mic[range.clone()]);
            let out_p = power(&out[range]);
            let excess = db(out_p) - db(mic_p);
            if near_p > 0.1 * mic_p && echo_p > 0.1 * mic_p {
                blocks += 1;
                worst = worst.max(excess);
                over0 += (excess > 0.0) as usize;
                over3 += (excess > 3.0) as usize;
                over6 += (excess > 6.0) as usize;
            }
            if near_p > 0.1 * mic_p && echo_p < 0.01 * mic_p {
                near_blocks += 1;
                near_loss += db(near_p) - db(out_p);
            }
        }
        println!(
            "{name}: double-talk blocks {blocks}, out > mic: {over0} (>0 dB), {over3} (>3 dB), {over6} (>6 dB), worst {worst:+.1} dB"
        );
        println!(
            "{name}: near-only blocks {near_blocks}, mean near loss {:.1} dB",
            if near_blocks > 0 { near_loss / near_blocks as f64 } else { 0.0 }
        );
    }
}

/// Measurement: how much echo gets through, in double talk and in far-end-only blocks. The echo is the mic minus the
/// near end, so the leak is the output minus the near end, and the ratio is an echo return loss enhancement over
/// the same blocks. Double talk that leaks echo at the level of the near voice is audible as static.
#[test]
#[ignore = "measurement: run with --ignored --nocapture"]
fn measure_echo_leak_in_double_talk() {
    let seconds = 12.0;
    let n = (seconds * RATE) as usize;
    let far = far_end(seconds, 31);
    let near: Vec<f32> = speech_like(n, &mut Rng::new(33)).iter().map(|s| 0.2 * s).collect();
    let spk = speaker_output(&far, 60.0, 100.0);
    let ir = room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5);
    let echo_only = mic_signal(&vec![0.0; n], &spk, &ir);
    let mic = mic_signal(&near, &spk, &ir);
    let settings = ChainSettings { noise_suppression: None, agc: false, echo_cancellation: true, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() };
    let out_far = run_with(&far, &echo_only, settings);
    let out_dt = run_with(&far, &mic, settings);
    let start = 2 * SAMPLE_RATE as usize;
    let block = FRAME_SAMPLES;
    let (mut dt_echo, mut dt_leak, mut far_echo, mut far_leak) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut dt_n, mut far_n) = (0usize, 0usize);
    for b in (start..n - block).step_by(block) {
        let r = b..b + block;
        let near_p = power(&near[r.clone()]);
        let echo_p = power(&echo_only[r.clone()]);
        let mic_p = power(&mic[r.clone()]);
        if near_p > 0.1 * mic_p && echo_p > 0.1 * mic_p {
            dt_n += 1;
            dt_echo += echo_p;
            dt_leak += power(&out_dt[r.clone()].iter().zip(&near[r.clone()]).map(|(o, n)| o - n).collect::<Vec<f32>>());
        }
        if echo_p > 0.0 {
            far_n += 1;
            far_echo += echo_p;
            far_leak += power(&out_far[r]);
        }
    }
    println!("double-talk blocks {dt_n}: echo leak {:.1} dB below the echo", db(dt_echo / dt_leak.max(1e-30)));
    println!("far-only blocks {far_n}: echo leak {:.1} dB below the echo", db(far_echo / far_leak.max(1e-30)));
}

/// The chain with echo cancellation switched by `echo_on(frame)`, the way the desktop loop applies a profile change:
/// settings change between frames, and the reference is pushed on every tick whatever the switch says.
fn run_switched(far: &[f32], mic: &[f32], echo_on: impl Fn(usize) -> bool) -> Vec<f32> {
    let base = ChainSettings {
        echo_cancellation: true,
        threshold: Threshold::Manual(-120.0),
        ..ChainSettings::default()
    };
    let mut chain = CaptureChain::new(base);
    let mut out = Vec::with_capacity(mic.len());
    for (i, (f, m)) in far.as_chunks::<FRAME_SAMPLES>().0.iter().zip(mic.as_chunks::<FRAME_SAMPLES>().0).enumerate() {
        chain.set_settings(ChainSettings { echo_cancellation: echo_on(i), ..base });
        chain.push_far(f);
        let mut frame = m.to_vec();
        chain.process(&mut frame, true);
        out.extend_from_slice(&frame);
    }
    out
}

/// The echo return loss enhancement over `range`, with no near-end speech.
fn erle_over(mic: &[f32], out: &[f32], range: std::ops::Range<usize>) -> f64 {
    db(power(&mic[range.clone()]) / power(&out[range]))
}

/// Processing goes off (the Studio profile) and back on in the middle of a call, with the far end talking throughout.
/// Turning it back on must bring the echo canceller back: its reference stays in step with the mic, and ERLE returns
/// to the level of a call that never switched.
#[test]
fn echo_recovers_after_processing_is_switched_off_and_on() {
    let s = Scenario { buffer_ms: 40.0, ppm: 0.0 };
    let (far, mic) = scenario_signals(26.0, &s);
    let frame = |sec: f64| (sec * RATE / FRAME_SAMPLES as f64) as usize;
    let off = frame(8.0)..frame(16.0);
    let out = run_switched(&far, &mic, |i| !off.contains(&i));
    let baseline = run_switched(&far, &mic, |_| true);
    let at = |sec: f64| (sec * RATE) as usize;
    println!("ERLE per 2 s window, processing off from 8 s to 16 s:");
    for start in [4.0, 8.0, 16.0, 18.0, 20.0, 22.0, 24.0] {
        let r = at(start)..at(start + 2.0);
        println!("  {start:>4.0}-{:>4.0} s  switched {:6.1} dB   never switched {:6.1} dB", start + 2.0, erle_over(&mic, &out, r.clone()), erle_over(&mic, &baseline, r));
    }
    let erle = erle_over(&mic, &out, at(18.0)..at(20.0));
    assert!(erle >= 35.0, "ERLE {erle:.1} dB 2 to 4 s after the processing came back on, expected at least 35 dB");
}

/// Mute as the voice loop does it: while the mic is closed, neither the reference nor the mic frames reach the chain.
/// Unmuting must find the canceller and the reference in step.
#[test]
fn echo_recovers_after_the_mic_is_muted_and_unmuted() {
    let s = Scenario { buffer_ms: 40.0, ppm: 0.0 };
    let (far, mic) = scenario_signals(26.0, &s);
    let base = ChainSettings { echo_cancellation: true, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() };
    let mut chain = CaptureChain::new(base);
    let mut out = Vec::with_capacity(mic.len());
    let muted = |i: usize| (8.0 * RATE / FRAME_SAMPLES as f64) as usize <= i && i < (16.0 * RATE / FRAME_SAMPLES as f64) as usize;
    for (i, (f, m)) in far.as_chunks::<FRAME_SAMPLES>().0.iter().zip(mic.as_chunks::<FRAME_SAMPLES>().0).enumerate() {
        if muted(i) {
            out.extend(std::iter::repeat_n(0.0, FRAME_SAMPLES));
            continue;
        }
        chain.push_far(f);
        let mut frame = m.to_vec();
        chain.process(&mut frame, true);
        out.extend_from_slice(&frame);
    }
    let erle = erle_over(&mic, &out, (18.0 * RATE) as usize..(20.0 * RATE) as usize);
    assert!(erle >= 35.0, "ERLE {erle:.1} dB 2 to 4 s after the mic came back, expected at least 35 dB");
}

/// Controls for the near-only loss: the same chain and near speech, with the echo path or the far end removed in turn.
/// A loss that stays when there is no echo is not the canceller removing echo, so it would be a measurement artefact.
#[test]
#[ignore = "measurement: run with --ignored --nocapture"]
fn measure_near_only_loss_controls() {
    let seconds = 12.0;
    let n = (seconds * RATE) as usize;
    let far = far_end(seconds, 31);
    let near: Vec<f32> = speech_like(n, &mut Rng::new(33)).iter().map(|s| 0.2 * s).collect();
    let spk = speaker_output(&far, 60.0, 100.0);
    let ir = room_ir(&mut Rng::new(5), 250.0, 250.0, 0.5);
    let echo_mic = mic_signal(&vec![0.0; n], &spk, &ir);
    let silent_far = vec![0.0f32; n];
    let settings = ChainSettings { noise_suppression: None, agc: false, echo_cancellation: true, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() };
    let cases: [(&str, &[f32], Vec<f32>); 3] = [
        ("no echo, far silent", &silent_far, near.clone()),
        ("far talks, no echo path", &far, near.clone()),
        ("far talks through the room", &far, mic_signal(&near, &spk, &ir)),
    ];
    let _ = echo_mic;
    for (name, far_in, mic) in cases {
        let out = run_with(far_in, &mic, settings);
        let (mut loss, mut blocks) = (0.0f64, 0usize);
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for b in (2 * SAMPLE_RATE as usize..n - FRAME_SAMPLES).step_by(FRAME_SAMPLES) {
            let r = b..b + FRAME_SAMPLES;
            if power(&near[r.clone()]) > 0.1 * power(&mic[r.clone()]) {
                blocks += 1;
                loss += db(power(&near[r.clone()])) - db(power(&out[r.clone()]));
                for t in r {
                    num += out[t] as f64 * near[t] as f64;
                    den += near[t] as f64 * near[t] as f64;
                }
            }
        }
        println!("{name}: near-only blocks {blocks}, mean loss per block {:.1} dB, best-fit near gain {:.1} dB", loss / blocks.max(1) as f64, db(num * num / den.max(1e-30) / den.max(1e-30)).max(-200.0));
    }
}
