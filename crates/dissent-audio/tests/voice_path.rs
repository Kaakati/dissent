//! Measurements of the call audio path: the microphone chain, the Opus sender and receiver, the
//! jitter buffer, the resampler and the desktop I/O loop. The printing tests run with
//! `cargo test -p dissent-audio --test voice_path -- --nocapture`. Tests that need audio hardware
//! or depend on wall-clock timing are `#[ignore]`, and run with `--ignored`.

mod common;

use std::time::{Duration, Instant};

use common::{aligned_snr, power, snr_db, voice_like, Rng};
use dissent_audio::dsp::{db_to_amp, rms, NoiseSuppressor, DENOISE_BLOCK};
use dissent_audio::jitter::{JitterBuffer, JitterStats, Packet, Playout};
use dissent_audio::resample::LinearResampler;
use dissent_audio::{
    CaptureChain, CaptureStream, ChainSettings, Decoder, Encoder, OpusDecoder, OpusEncoder,
    PlayoutStream, SenderConfig, Threshold, VoiceSender, FRAME_SAMPLES, SAMPLE_RATE, STRONG_STRENGTH,
};

fn db(power: f64) -> f64 {
    10.0 * power.max(1e-30).log10()
}

/// Mean power of a signal in dBFS, as the level meter reads it.
fn level_db(x: &[f32]) -> f64 {
    db(power(x))
}

/// Echo return loss enhancement of the chain's canceller in the steady state, with the echo
/// arriving `delay_ms` after the far-end sample was made.
///
/// The desktop loop processes a mic chunk, then pushes that tick's far-end chunk. `oracle` pushes
/// the far-end chunk first, which is the alignment the reference would need to be exact.
fn erle_db(delay_ms: u64, seconds: usize, speech_like: bool) -> f64 {
    let mut rng = Rng::new(11);
    let n = SAMPLE_RATE as usize * seconds;
    let far: Vec<f32> = if speech_like {
        voice_like(n, &mut rng).iter().map(|s| 0.3 * s).collect()
    } else {
        (0..n).map(|_| 0.3 * rng.bipolar()).collect()
    };
    let near: Vec<f32> = (0..n).map(|_| 0.001 * rng.bipolar()).collect();
    let d = (SAMPLE_RATE as u64 * delay_ms / 1000) as usize;
    // Two-tap echo path: the direct echo at 0.5, and a reflection 37 samples later at 0.25.
    let mic: Vec<f32> = (0..n)
        .map(|t| {
            let a = if t >= d { far[t - d] } else { 0.0 };
            let b = if t >= d + 37 { far[t - d - 37] } else { 0.0 };
            near[t] + 0.5 * a + 0.25 * b
        })
        .collect();

    let mut chain = CaptureChain::new(ChainSettings {
        echo_cancellation: true,
        threshold: Threshold::Manual(-120.0),
        ..ChainSettings::default()
    });
    let mut out = Vec::with_capacity(n);
    for (k, far_chunk) in far.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
        let mut frame = mic[k * FRAME_SAMPLES..(k + 1) * FRAME_SAMPLES].to_vec();
        // The desktop loop pushes each tick's playout before that tick's capture.
        chain.push_far(far_chunk);
        chain.process(&mut frame, true);
        out.extend_from_slice(&frame);
    }

    let tail = n / 2..n;
    let echo_in: Vec<f32> = tail.clone().map(|t| mic[t] - near[t]).collect();
    let echo_out: Vec<f32> = tail.map(|t| out[t] - near[t]).collect();
    db(power(&echo_in) / power(&echo_out))
}

#[test]
fn echo_return_loss_by_echo_delay_with_a_white_far_end() {
    // White noise has a flat spectrum, so NLMS converges quickly. This is the fast regression.
    println!("echo delay   ERLE, white far end, 6 s");
    for delay in [0u64, 100, 200] {
        let erle = erle_db(delay, 6, false);
        println!("{delay:>7} ms   {erle:>6.1} dB");
        assert!(erle >= 15.0, "ERLE {erle:.1} dB at a {delay} ms echo delay, expected at least 15 dB");
    }
}

#[test]
#[ignore = "NLMS converges slowly on the colored voice-like signal; 8 s of audio per delay. Run in release"]
fn echo_return_loss_by_echo_delay_with_a_speech_like_far_end() {
    println!("echo delay   ERLE, voice-like far end, 8 s");
    for delay in [0u64, 50, 100, 150, 200] {
        let erle = erle_db(delay, 8, true);
        println!("{delay:>7} ms   {erle:>6.1} dB");
        assert!(erle >= 15.0, "ERLE {erle:.1} dB at a {delay} ms echo delay, expected at least 15 dB");
    }
}

/// Talk spurts of 1.0 s separated by 0.6 s of room noise at about -70 dBFS.
fn spurts(seconds: usize, rng: &mut Rng) -> Vec<f32> {
    let n = SAMPLE_RATE as usize * seconds;
    let speech = voice_like(n, rng);
    (0..n)
        .map(|t| {
            let frame = t / FRAME_SAMPLES;
            if frame % 80 < 50 {
                speech[t]
            } else {
                0.0003 * rng.bipolar()
            }
        })
        .collect()
}

/// One packet as the wire carries it: the frame count of what was sent, the sender's sample
/// clock, and the Opus payload.
struct Wire {
    seq: u32,
    timestamp: u32,
    payload: Vec<u8>,
    /// Quiet enough to count as silence for gap handling.
    silent: bool,
}

/// Runs the sender over `signal`. With `voiced_only`, frames the sender marks unvoiced are not
/// sent, as `VoiceAudio::capture` does in the desktop build.
fn send(signal: &[f32], voiced_only: bool) -> Vec<Wire> {
    let mut sender = VoiceSender::opus(SenderConfig {
        bitrate_bps: 32_000,
        loss_percent: 5,
        noise_suppression: false,
        agc: false,
    })
    .unwrap();
    let mut wire = Vec::new();
    let mut seq = 0u32;
    for frame in signal.as_chunks::<FRAME_SAMPLES>().0.iter() {
        for p in sender.push_capture(frame).unwrap() {
            if voiced_only && !p.voiced {
                continue;
            }
            wire.push(Wire {
                seq,
                timestamp: p.timestamp,
                payload: p.payload,
                silent: p.audio_level >= dissent_audio::QUIET_LEVEL,
            });
            seq += 1;
        }
    }
    wire
}

/// Puts each packet on a path with `loss` and up to `max_jitter_ms` of random delay. Also
/// returns which wire packets were dropped.
fn arrivals(wire: &[Wire], loss: f64, max_jitter_ms: u64, rng: &mut Rng) -> (Vec<Packet>, Vec<bool>) {
    let dropped: Vec<bool> = wire.iter().map(|_| rng.unit() < loss).collect();
    let mut out: Vec<Packet> = wire
        .iter()
        .zip(&dropped)
        .filter_map(|(w, drop)| {
            if *drop {
                return None;
            }
            let sent_ms = u64::from(w.timestamp) / 48;
            let jitter = (rng.unit() * (max_jitter_ms + 1) as f64) as u64;
            Some(Packet {
                seq: w.seq,
                timestamp: w.timestamp,
                payload: w.payload.clone(),
                arrival_ms: sent_ms + jitter,
                silent: w.silent,
            })
        })
        .collect();
    out.sort_by_key(|p| p.arrival_ms);
    (out, dropped)
}

/// What the buffer should count for the injected drops. A slot after a silent packet that was
/// dropped is a silent gap. Any other dropped slot is a loss. This mirrors `JitterBuffer::pop`.
fn expected_losses(wire: &[Wire], dropped: &[bool]) -> (u64, u64) {
    let (mut lost, mut silent) = (0u64, 0u64);
    let mut after_silent = false;
    // The buffer starts its cursor at the first packet that arrives, so leading drops are not slots.
    let first_arrival = dropped.iter().position(|d| !d).unwrap_or(dropped.len());
    for (w, drop) in wire.iter().zip(dropped).skip(first_arrival) {
        if *drop {
            if after_silent {
                silent += 1;
            } else {
                lost += 1;
                after_silent = false;
            }
        } else {
            after_silent = w.silent;
        }
    }
    (lost, silent)
}

/// Plays the arrivals on a 20 ms tick clock and returns each tick's kind and level, plus stats.
fn play(arrivals: &[Packet], ticks: usize) -> (Vec<(Playout, f64)>, JitterStats) {
    let mut jb = JitterBuffer::new(OpusDecoder::new().unwrap());
    let mut next = 0;
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut out = Vec::with_capacity(ticks);
    for t in 0..ticks {
        let now = 20 * t as u64;
        while next < arrivals.len() && arrivals[next].arrival_ms <= now {
            jb.push(arrivals[next].clone());
            next += 1;
        }
        let kind = jb.pop(now, &mut frame).unwrap();
        out.push((kind, level_db(&frame)));
    }
    (out, jb.stats())
}

fn mean_db(levels: &[f64]) -> f64 {
    if levels.is_empty() {
        return f64::NAN;
    }
    levels.iter().sum::<f64>() / levels.len() as f64
}

#[test]
fn jitter_buffer_playout_of_the_desktop_send_pattern() {
    let seconds = 30;
    let mut rng = Rng::new(3);
    let signal = spurts(seconds, &mut rng);
    let ticks = seconds * 50 + 100;
    println!("stream        decoded    fec  concealed silence | lost  late  compressed stretched | concealed dBFS  decoded dBFS");
    // The third row keeps the voiced-only payloads but numbers each packet by the sender's frame
    // index, so a silent gap leaves a hole in seq. That is the test of the seq/time mismatch.
    for (name, voiced_only, index_seq) in [
        ("voiced-only", true, false),
        ("all frames", false, false),
        ("voiced, idx seq", true, true),
    ] {
        let mut wire = send(&signal, voiced_only);
        if index_seq {
            for w in &mut wire {
                w.seq = w.timestamp / FRAME_SAMPLES as u32;
            }
        }
        let mut net = Rng::new(7);
        let (packets, dropped) = arrivals(&wire, 0.02, 60, &mut net);
        let (out, stats) = play(&packets, ticks);
        let (want_lost, _) = expected_losses(&wire, &dropped);
        assert_eq!(stats.late, 0, "{name}: packets arrived late");
        assert_eq!(stats.lost, want_lost, "{name}: lost slots must match the injected loss");
        // Every packet that arrived plays from its own slot, unless the buffer trims it to reduce
        // delay. Silent gaps are slots the sender never sent, so they are counted by the buffer.
        let arrived = packets.len();
        let decoded = out.iter().filter(|(k, _)| *k == Playout::Decoded).count();
        assert_eq!(
            decoded as u64 + stats.compressed,
            arrived as u64,
            "{name}: each arrival plays from its own slot or is trimmed"
        );
        let count = |want: Playout| out.iter().filter(|(k, _)| *k == want).count();
        let levels = |want: Playout| -> Vec<f64> {
            out.iter().filter(|(k, _)| *k == want).map(|(_, l)| *l).collect()
        };
        println!(
            "{name:<13} {:>7} {:>6} {:>10} {:>7} | {:>5} {:>5} {:>10} {:>9} | {:>14.1} {:>14.1}",
            count(Playout::Decoded),
            count(Playout::Fec),
            count(Playout::Concealed),
            count(Playout::Silence),
            stats.lost,
            stats.late,
            stats.compressed,
            stats.stretched,
            mean_db(&levels(Playout::Concealed)),
            mean_db(&levels(Playout::Decoded)),
        );
    }
}

/// Scenario: speech bursts over room noise. Returns (chain output, gate open flags).
fn chain_run(input: &[f32], settings: ChainSettings) -> (Vec<f32>, Vec<bool>) {
    let mut chain = CaptureChain::new(settings);
    let mut out = Vec::with_capacity(input.len());
    let mut open = Vec::new();
    for chunk in input.as_chunks::<FRAME_SAMPLES>().0.iter() {
        let mut frame = chunk.to_vec();
        let report = chain.process(&mut frame, true);
        open.push(report.open);
        out.extend_from_slice(&frame);
    }
    (out, open)
}

#[test]
fn agc_on_room_noise_and_gate_interaction() {
    let mut rng = Rng::new(5);
    let n = SAMPLE_RATE as usize * 10;
    // Uniform noise with rms -50 dBFS. A uniform value in [-a, a] has rms a / sqrt(3).
    let a = db_to_amp(-50.0) * 3f32.sqrt();
    let noise: Vec<f32> = (0..n).map(|_| a * rng.bipolar()).collect();
    let tail = n / 2..n;
    println!("room noise at -50 dBFS, last 5 s:");
    println!("  agc    output dBFS   gate open   output/input dB");
    for agc in [false, true] {
        let (out, open) = chain_run(
            &noise,
            ChainSettings {
                agc,
                threshold: Threshold::Auto,
                ..ChainSettings::default()
            },
        );
        let out_db = level_db(&out[tail.clone()]);
        let open_frac = open[open.len() / 2..].iter().filter(|o| **o).count() as f64
            / (open.len() - open.len() / 2) as f64;
        if agc {
            assert_eq!(open_frac, 0.0, "room noise at -50 dBFS must stay gated with AGC on");
            assert!(out_db < -99.0, "gated room noise must be silent, got {out_db:.1} dBFS");
        }
        println!(
            "  {:<6} {:>11.1} {:>11.2} {:>16.1}",
            agc,
            out_db,
            open_frac,
            out_db - level_db(&noise[tail.clone()])
        );
    }
}

#[test]
fn agc_clipping_on_quiet_to_loud_onsets_and_input_gain() {
    let mut rng = Rng::new(9);
    let loud_len = SAMPLE_RATE as usize;
    let quiet_len = SAMPLE_RATE as usize;
    let mut input = Vec::new();
    let mut loud_mask = Vec::new();
    for _ in 0..5 {
        let quiet = voice_like(quiet_len, &mut rng);
        let loud = voice_like(loud_len, &mut rng);
        let g_quiet = db_to_amp(-40.0) / rms(&quiet);
        let g_loud = db_to_amp(-14.0) / rms(&loud);
        input.extend(quiet.iter().map(|s| s * g_quiet));
        loud_mask.extend(std::iter::repeat_n(false, quiet_len));
        input.extend(loud.iter().map(|s| s * g_loud));
        loud_mask.extend(std::iter::repeat_n(true, loud_len));
    }
    println!("speech: quiet -40 dBFS rms, loud -14 dBFS rms, 1 s each, 5 cycles");
    println!("  settings                      samples above the knee   peak");
    for (name, agc, input_gain) in [
        ("agc off, input gain 1.0", false, 1.0),
        ("agc off, input gain 2.0", false, 2.0),
        ("agc on,  input gain 1.0", true, 1.0),
    ] {
        let (out, _) = chain_run(
            &input,
            ChainSettings {
                input_gain,
                agc,
                threshold: Threshold::Manual(-120.0),
                ..ChainSettings::default()
            },
        );
        let loud_samples: Vec<f32> = out
            .iter()
            .zip(loud_mask.iter())
            .filter(|(_, m)| **m)
            .map(|(s, _)| *s)
            .collect();
        let over_knee = loud_samples.iter().filter(|s| s.abs() > 0.8).count();
        let peak = out.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        println!(
            "  {name:<28} {:>8} of {:>6} ({:>5.2}%) bent by the limiter   peak {peak:.4}",
            over_knee,
            loud_samples.len(),
            100.0 * over_knee as f64 / loud_samples.len() as f64
        );
        assert!(peak <= 1.0, "{name}: output peak {peak} is above full scale");
    }
}

/// Runs the chain's noise suppression on `clean` plus room noise at about -40 dBFS, and reports
/// the output SNR against the clean signal for each strength.
fn ns_fidelity(label: &str, clean: &[f32], rng: &mut Rng) -> f64 {
    let mut standard = f64::NAN;
    let noisy: Vec<f32> = clean.iter().map(|s| s + 0.0055 * rng.bipolar()).collect();
    let noise_only: Vec<f32> = noisy.iter().zip(clean).map(|(x, c)| x - c).collect();
    println!("{label}: input SNR {:.1} dB", db(power(clean) / power(&noise_only)));
    for (name, ns) in [
        ("noise suppression off", None),
        ("standard 0.6", Some(0.6)),
        ("strength 0.8", Some(0.8)),
        ("voice isolation 1.0", Some(STRONG_STRENGTH)),
    ] {
        let (out, _) = chain_run(
            &noisy,
            ChainSettings {
                noise_suppression: ns,
                threshold: Threshold::Manual(-120.0),
                ..ChainSettings::default()
            },
        );
        let (lag, snr) = aligned_snr(clean, &out, 960);
        println!("  {name:<24} output SNR {snr:>6.1} dB   (lag {lag} samples)");
        if name == "standard 0.6" {
            standard = snr;
        }
    }
    standard
}

#[test]
fn noise_suppression_strength_speech_fidelity() {
    let mut rng = Rng::new(21);
    let n = SAMPLE_RATE as usize * 4;
    let harmonic: Vec<f32> = voice_like(n, &mut rng).iter().map(|s| 0.5 * s).collect();
    let speech_snr = ns_fidelity("harmonic voice-like signal", &harmonic, &mut rng);
    assert!(speech_snr >= 13.0, "standard mix SNR {speech_snr:.1} dB, expected at least 13 dB");
    let sweep: Vec<f32> = (0..n)
        .map(|t| {
            let f = 100.0 + 3900.0 * t as f32 / n as f32;
            0.3 * (std::f32::consts::TAU * f * t as f32 / SAMPLE_RATE as f32).sin()
        })
        .collect();
    // A pure tone is distorted by RNNoise on its own, so this is reported, not asserted.
    ns_fidelity("100 Hz to 4 kHz sweep", &sweep, &mut rng);
}

#[test]
fn opus_waveform_gain_and_snr_sanity() {
    // A pure tone should round-trip at near unity gain with a high SNR. This checks the codec
    // path, separately from the signal used above.
    let n = SAMPLE_RATE as usize * 3;
    let tone: Vec<f32> = (0..n)
        .map(|t| 0.3 * (std::f32::consts::TAU * 300.0 * t as f32 / SAMPLE_RATE as f32).sin())
        .collect();
    let mut enc = OpusEncoder::new(64_000, 0).unwrap();
    let mut dec = OpusDecoder::new().unwrap();
    let mut decoded = Vec::with_capacity(n);
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    for frame in tone.as_chunks::<FRAME_SAMPLES>().0.iter() {
        let packet = enc.encode(frame).unwrap();
        dec.decode(Some(&packet), &mut out).unwrap();
        decoded.extend_from_slice(&out);
    }
    let (lag, snr) = aligned_snr(&tone, &decoded, 480);
    let x = &tone[..n - lag];
    let y = &decoded[lag..n];
    let gain: f64 = x.iter().zip(y).map(|(a, b)| f64::from(*a * *b)).sum::<f64>() / power(x) / n as f64;
    println!("300 Hz tone at 64 kbps, 0% loss: SNR {snr:.1} dB, lag {lag}, gain {gain:.3}");
}

#[test]
fn opus_round_trip_quality_and_rate() {
    let mut rng = Rng::new(31);
    let seconds = 5;
    let n = SAMPLE_RATE as usize * seconds;
    let speech: Vec<f32> = voice_like(n, &mut rng).iter().map(|s| 0.5 * s).collect();
    println!("Opus VOIP, 5% expected loss, 5 s voice-like signal:");
    println!("  bitrate set   bitrate measured   round-trip SNR (aligned)");
    for bps in [16_000u32, 24_000, 32_000, 48_000, 64_000] {
        let mut enc = OpusEncoder::new(bps, 5).unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let mut bytes = 0usize;
        let mut decoded = Vec::with_capacity(n);
        let mut out = vec![0.0f32; FRAME_SAMPLES];
        for frame in speech.as_chunks::<FRAME_SAMPLES>().0.iter() {
            let packet = enc.encode(frame).unwrap();
            bytes += packet.len();
            dec.decode(Some(&packet), &mut out).unwrap();
            decoded.extend_from_slice(&out);
        }
        let kbps = bytes as f64 * 8.0 / seconds as f64 / 1000.0;
        let (lag, snr) = aligned_snr(&speech, &decoded, 480);
        println!("  {:>6} kbps   {:>10.1} kbps   {snr:>6.1} dB  (lag {lag})", bps / 1000, kbps);
    }

    let mut enc = OpusEncoder::new(32_000, 5).unwrap();
    let silent = vec![0.0f32; FRAME_SAMPLES];
    let sizes: Vec<usize> = (0..100).map(|_| enc.encode(&silent).unwrap().len()).collect();
    println!(
        "  DTX: 100 silent frames, packet bytes min {} max {} (in_dtx: {})",
        sizes.iter().min().unwrap(),
        sizes.iter().max().unwrap(),
        enc.in_dtx()
    );
}

#[test]
fn resampler_fidelity_at_device_rates() {
    let n_in = 96_000;
    let tone = |f: f32, rate: f32, n: usize| -> Vec<f32> {
        (0..n)
            .map(|t| 0.5 * (std::f32::consts::TAU * f * t as f32 / rate).sin())
            .collect()
    };
    println!("resampler, linear interpolation, 1 kHz tone at 0.5 amplitude:");
    for (from, to) in [(44_100u32, 48_000u32), (96_000, 48_000), (48_000, 44_100)] {
        let n = n_in * from as usize / 96_000;
        let x = tone(1000.0, from as f32, n);
        let mut r = LinearResampler::new(from, to);
        let mut y = Vec::new();
        r.process(&x, &mut y);
        let ideal = tone(1000.0, to as f32, y.len());
        let (lag, snr) = aligned_snr(&ideal, &y, 8);
        println!("  {from} -> {to}: SNR {snr:.1} dB (lag {lag})");
    }
    // A 30 kHz tone is above the 24 kHz Nyquist of 48 kHz. Linear resampling does not filter it.
    let x = tone(30_000.0, 96_000.0, n_in);
    let mut r = LinearResampler::new(96_000, 48_000);
    let mut y = Vec::new();
    r.process(&x, &mut y);
    println!(
        "  96 -> 48 with a 30 kHz tone at 0.5: output {:.1} dBFS rms (the tone itself would be {:.1})",
        level_db(&y),
        level_db(&x)
    );
}

/// The desktop I/O loop's pacing, copied from `run` in `voice.rs`: a fixed 20 ms schedule, and a
/// reset to now when an iteration finishes late. Returns the wall-clock interval between
/// iterations, and how many iterations were late.
fn paced_intervals(seconds: u64, sleep_fn: impl Fn(Duration)) -> (Vec<f64>, u64) {
    let tick = Duration::from_millis(20);
    let mut next = Instant::now();
    let mut last = Instant::now();
    let mut late = 0u64;
    let mut intervals = Vec::new();
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        next += tick;
        let now = Instant::now();
        if next > now {
            sleep_fn(next - now);
        } else {
            next = now;
            late += 1;
        }
        let t = Instant::now();
        intervals.push((t - last).as_secs_f64() * 1000.0);
        last = t;
    }
    (intervals, late)
}

fn summarize(name: &str, mut v: Vec<f64>, late: u64) {
    let count = v.len();
    v.sort_by(f64::total_cmp);
    let pct = |p: f64| v[((count - 1) as f64 * p) as usize];
    let over = v.iter().filter(|x| **x >= 25.0).count();
    println!(
        "  {name:<26} n={count:<4} mean {:>5.2} p50 {:>5.2} p95 {:>5.2} p99 {:>5.2} max {:>6.2} ms | >=25 ms: {over:<4} late resets: {late}",
        v.iter().sum::<f64>() / count as f64,
        pct(0.5),
        pct(0.95),
        pct(0.99),
        v[count - 1],
    );
}

#[test]
#[ignore = "wall-clock timing; run with --ignored --nocapture on the machine under test"]
fn io_loop_pacing_on_this_machine() {
    println!("20 ms loop pacing over 5 s, no device work:");
    let (v, late) = paced_intervals(5, std::thread::sleep);
    summarize("voice.rs schedule", v, late);
    let (v, _) = paced_intervals(5, |d| {
        std::thread::sleep(d);
    });
    summarize("voice.rs schedule (repeat)", v, 0);
    let mut overshoot = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        overshoot.push((t.elapsed().as_secs_f64() - 0.020) * 1000.0);
    }
    overshoot.sort_by(f64::total_cmp);
    println!(
        "  raw sleep(20 ms) overshoot over 200 calls: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        overshoot[100],
        overshoot[190],
        overshoot[199]
    );
}

#[test]
#[ignore = "needs audio hardware; plays silence only. Run with --ignored --nocapture"]
fn device_rates_and_underruns_under_the_io_loop() {
    let mut playout = match PlayoutStream::open(None) {
        Ok(p) => p,
        Err(e) => {
            println!("no speaker: {e}");
            return;
        }
    };
    let mut capture = CaptureStream::open(None).ok();
    println!(
        "speaker rate {} Hz, microphone rate {:?} Hz",
        playout.device_rate(),
        capture.as_ref().map(|c| c.device_rate())
    );
    // Warm up for one second, then measure 10 s. Silence is written, so nothing audible plays.
    let tick = Duration::from_millis(20);
    let mut next = Instant::now();
    let mut read = Vec::new();
    let mut captured = 0usize;
    let silent = vec![0.0f32; FRAME_SAMPLES];
    let start = Instant::now();
    let mut underruns_at_warmup = 0;
    let mut measured_from = None;
    while start.elapsed() < Duration::from_secs(11) {
        if measured_from.is_none() && start.elapsed() >= Duration::from_secs(1) {
            measured_from = Some(Instant::now());
            underruns_at_warmup = playout.underruns();
            captured = 0;
        }
        if let Some(cap) = capture.as_mut() {
            read.clear();
            let n = cap.read_48k(&mut read);
            if measured_from.is_some() {
                captured += n;
            }
        }
        playout.write_48k(&silent);
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    let secs = measured_from.map_or(1.0, |t| t.elapsed().as_secs_f64());
    let underruns = playout.underruns() - underruns_at_warmup;
    println!(
        "10 s measured: playout underrun samples {underruns} ({:.1} per 20 ms tick)",
        underruns as f64 / (secs * 50.0)
    );
    if let Some(cap) = capture.as_ref() {
        println!(
            "capture: {:.0} samples/s at 48 kHz after resampling, device overflows {}",
            captured as f64 / secs,
            cap.overflows()
        );
    }
}

#[test]
#[ignore = "plays five 5 ms clicks through the speaker and listens on the mic. Use low system volume"]
fn acoustic_loopback_delay_from_speaker_write_to_mic_read() {
    let mut playout = match PlayoutStream::open(None) {
        Ok(p) => p,
        Err(e) => {
            println!("no speaker: {e}");
            return;
        }
    };
    let mut capture = CaptureStream::open(None).expect("microphone");
    println!("default input {:?}, default output {:?}", dissent_audio::default_input_name(), dissent_audio::default_output_name());
    let click: Vec<f32> = (0..240)
        .map(|t| 0.8 * (std::f32::consts::TAU * 1000.0 * t as f32 / SAMPLE_RATE as f32).sin())
        .collect();
    let silent = vec![0.0f32; FRAME_SAMPLES];
    let tick = Duration::from_millis(20);
    let start = Instant::now();
    let mut next = Instant::now();
    let mut read = Vec::new();
    // Each read: when it happened, and the power of the samples it returned.
    let mut reads: Vec<(f64, f64)> = Vec::new();
    // Write times of the clicks, in seconds from `start`.
    let mut writes: Vec<f64> = Vec::new();
    let click_at = [3.0, 3.5, 4.0, 4.5, 5.0];
    let mut next_click = 0;
    while start.elapsed() < Duration::from_millis(5600) {
        read.clear();
        capture.read_48k(&mut read);
        let at = start.elapsed().as_secs_f64();
        reads.push((at, power(&read)));
        if next_click < click_at.len() && at >= click_at[next_click] {
            let mut frame = silent.clone();
            frame[..click.len()].copy_from_slice(&click);
            writes.push(start.elapsed().as_secs_f64());
            playout.write_48k(&frame);
            next_click += 1;
        } else {
            playout.write_48k(&silent);
        }
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    // Ambient level from the reads before the first click.
    let first_samples = reads.iter().find(|(_, p)| *p > 0.0).map(|(t, _)| *t);
    println!("first mic samples at {first_samples:?} s after the stream opened");
    let mut ambient: Vec<f64> = reads.iter().filter(|(t, _)| *t > 2.0 && *t < 2.9).map(|(_, p)| *p).collect();
    ambient.sort_by(f64::total_cmp);
    let floor = (ambient.iter().sum::<f64>() / ambient.len() as f64).max(1e-6);
    let nonempty = reads.iter().filter(|(_, p)| *p > 0.0).count();
    println!("reads with mic samples: {nonempty} of {} (the loop reads every 20 ms)", reads.len());
    println!(
        "ambient mic power {:.1} dBFS; detection at 10x the ambient power",
        db(floor)
    );
    let mut delays = Vec::new();
    for w in &writes {
        let hit = reads
            .iter()
            .find(|(t, p)| *t >= *w && *p > 10.0 * floor);
        match hit {
            Some((t, p)) => {
                println!("  click at {w:.3} s heard at read {t:.3} s ({:.1} dBFS in that read)", db(*p));
                delays.push(t - w);
            }
            None => println!("  click at {w:.3} s not heard above 10x ambient"),
        }
    }
    if !delays.is_empty() {
        delays.sort_by(f64::total_cmp);
        println!(
            "speaker write to mic read: median {:.0} ms, range {:.0}..{:.0} ms (read quantized to 20 ms)",
            delays[delays.len() / 2] * 1000.0,
            delays[0] * 1000.0,
            delays[delays.len() - 1] * 1000.0
        );
    }
}

#[test]
fn dry_wet_mix_of_noise_suppression_is_not_time_aligned() {
    // The chain mixes raw and denoised audio as raw + (denoised - raw) * strength, at the same
    // index. RNNoise's output lags its input, so the mix adds a shifted copy of the raw signal.
    let mut rng = Rng::new(21);
    let n = SAMPLE_RATE as usize * 4;
    let clean: Vec<f32> = voice_like(n, &mut rng).iter().map(|s| 0.5 * s).collect();
    let noisy: Vec<f32> = clean.iter().map(|s| s + 0.0055 * rng.bipolar()).collect();
    let mut ns = NoiseSuppressor::new();
    let mut dn = noisy.clone();
    for block in dn.as_chunks_mut::<DENOISE_BLOCK>().0.iter_mut() {
        ns.process_block(block);
    }
    let lag = DENOISE_BLOCK;
    let reference = &clean[..n - lag];
    let snr = |y: &[f32]| snr_db(reference, &y[lag..]);
    // (a) The chain's mix, with the raw signal at the denoised output's index.
    let mix_a: Vec<f32> = noisy.iter().zip(&dn).map(|(r, d)| r + (d - r) * 0.6).collect();
    // (b) The same mix with the raw signal delayed by the same lag.
    let mut mix_b = vec![0.0f32; n];
    for t in lag..n {
        let r = noisy[t - lag];
        mix_b[t] = r + (dn[t] - r) * 0.6;
    }
    println!("speech with -40 dBFS noise, 0.6 mix, output SNR against clean speech delayed by {lag}:");
    println!("  denoised only (strength 1.0)     {:>6.1} dB", snr(&dn));
    println!("  chain mix as built (raw at t)     {:>6.1} dB", snr(&mix_a));
    println!("  mix with raw delayed to match    {:>6.1} dB", snr(&mix_b));
    println!("  raw input, no processing          {:>6.1} dB", snr_db(&clean, &noisy));
}

#[test]
fn noise_removed_by_each_ns_strength() {
    // Noise only, at -40 dBFS rms, through the chain. The output level is how much noise remains.
    let mut rng = Rng::new(51);
    let n = SAMPLE_RATE as usize * 4;
    let a = db_to_amp(-40.0) * 3f32.sqrt();
    let noise: Vec<f32> = (0..n).map(|_| a * rng.bipolar()).collect();
    println!("noise only at -40 dBFS, output level by NS strength:");
    for (name, ns) in [("off", None), ("standard 0.6", Some(0.6)), ("strength 0.8", Some(0.8)), ("voice isolation 1.0", Some(STRONG_STRENGTH))] {
        let (out, _) = chain_run(&noise, ChainSettings { noise_suppression: ns, threshold: Threshold::Manual(-120.0), ..ChainSettings::default() });
        let tail = &out[n / 2..];
        println!("  {name:<22} {:>6.1} dBFS rms", level_db(tail));
    }
}
