mod common;

use common::{power, voice_like, Rng};
use dissent_audio::dsp::{audio_level, Agc, EchoCanceller, NoiseSuppressor, VoiceDetector};
use dissent_audio::FRAME_SAMPLES;

const BLOCK: usize = 480;

fn to_db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

#[test]
fn noise_suppression_lowers_the_noise_floor() {
    // Background noise in rooms and on fans is coloured, with most energy at low frequencies.
    // RNNoise is trained on that kind of noise, so it is the case we test. White noise is its weak
    // case, and the probe measured only 1 to 3 dB reduction there.
    let mut rng = Rng::new(1);
    let mut ns = NoiseSuppressor::new();
    let mut in_power = 0.0;
    let mut out_power = 0.0;
    let mut state = 0.0f32;
    for b in 0..400 {
        let raw: Vec<f32> = (0..BLOCK)
            .map(|_| {
                state = 0.97 * state + rng.bipolar();
                state
            })
            .collect();
        let scale = 0.02 / power(&raw).sqrt() as f32;
        let noise: Vec<f32> = raw.iter().map(|s| s * scale).collect();
        let mut block = noise.clone();
        ns.process_block(&mut block);
        if b >= 100 {
            in_power += power(&noise);
            out_power += power(&block);
        }
    }
    let reduction = to_db(in_power / out_power);
    println!("coloured noise floor reduced by {reduction:.1} dB");
    assert!(
        reduction > 15.0,
        "only {reduction:.1} dB of noise reduction"
    );
}

#[test]
fn voice_probability_is_higher_for_speech_than_silence() {
    let mut rng = Rng::new(2);
    let speech = voice_like(BLOCK * 300, &mut rng);

    let mut ns = NoiseSuppressor::new();
    let mut silence_p = 0.0f32;
    for b in 0..100 {
        let mut block = vec![0.0f32; BLOCK];
        let p = ns.process_block(&mut block);
        if b >= 50 {
            silence_p += p;
        }
    }
    let silence_p = silence_p / 50.0;

    let mut ns = NoiseSuppressor::new();
    let mut speech_p = 0.0f32;
    for (b, chunk) in speech.as_chunks::<BLOCK>().0.iter().enumerate() {
        let mut block = chunk.to_vec();
        let p = ns.process_block(&mut block);
        if b >= 50 {
            speech_p += p;
        }
    }
    let speech_p = speech_p / (300.0 - 50.0);
    println!("mean VAD probability: speech {speech_p:.2}, silence {silence_p:.2}");
    assert!(
        speech_p > silence_p + 0.2,
        "speech {speech_p:.2} vs silence {silence_p:.2}"
    );

    let mut detector = VoiceDetector::default();
    assert!(!detector.update(silence_p));
}

#[test]
fn audio_level_follows_rfc6464_scale() {
    assert_eq!(audio_level(&[0.0; FRAME_SAMPLES]), 127);
    // A sine at amplitude 0.3 has RMS 0.3/sqrt(2), which is about -13.5 dBov.
    let sine: Vec<f32> = (0..FRAME_SAMPLES)
        .map(|i| 0.3 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin())
        .collect();
    let level = audio_level(&sine);
    println!("audio level of a 0.3 amplitude sine: {level}");
    assert!((12..=14).contains(&level), "level {level}");
    let full: Vec<f32> = sine.iter().map(|s| s / 0.3).collect();
    assert!(
        audio_level(&full) < level,
        "louder frame must have a lower level byte"
    );
}

#[test]
fn voice_detector_holds_over_hangover() {
    let mut d = VoiceDetector::new(0.5, 3);
    assert!(d.update(0.9));
    assert!(d.update(0.1));
    assert!(d.update(0.1));
    assert!(d.update(0.1));
    assert!(!d.update(0.1));
}

#[test]
fn agc_brings_quiet_speech_toward_target() {
    let mut rng = Rng::new(3);
    // Speech at about -30 dBFS RMS, which the +12 dB gain range can reach.
    let mut speech = voice_like(FRAME_SAMPLES * 200, &mut rng);
    let raw_rms = (power(&speech)).sqrt();
    let scale = 0.0316 / raw_rms as f32;
    for s in speech.iter_mut() {
        *s *= scale;
    }
    let mut agc = Agc::new();
    let mut out = speech.clone();
    for frame in out.as_chunks_mut::<FRAME_SAMPLES>().0 {
        agc.process(frame);
    }
    let tail = &out[out.len() - FRAME_SAMPLES * 80..];
    let level_db = to_db(power(tail));
    println!("AGC output {level_db:.1} dBFS, gain {:.2}", agc.gain());
    assert!(
        (level_db - (-20.0)).abs() < 3.0,
        "output {level_db:.1} dBFS"
    );
}

/// Echo of far-end noise through a two-tap path with 20 ms and 35 ms delays.
fn echo_scenario(seconds: usize, tail_ms: u32, seed: u64) -> (Vec<f32>, Vec<f32>) {
    let mut rng = Rng::new(seed);
    let n = 48_000 * seconds;
    let far: Vec<f32> = (0..n).map(|_| 0.2 * rng.bipolar()).collect();
    let near: Vec<f32> = (0..n)
        .map(|t| {
            let a = if t >= 960 { far[t - 960] } else { 0.0 };
            let b = if t >= 1680 { far[t - 1680] } else { 0.0 };
            0.5 * a + 0.25 * b + 0.0005 * rng.bipolar()
        })
        .collect();

    let mut ec = EchoCanceller::with_tail_ms(tail_ms);
    let mut out = near.clone();
    for (chunk_far, chunk_near) in far
        .as_chunks::<FRAME_SAMPLES>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<FRAME_SAMPLES>().0.iter_mut())
    {
        ec.push_far(chunk_far);
        ec.process(chunk_near);
    }
    (near, out)
}

#[test]
fn nlms_echo_canceller_attenuates_echo_by_15db_after_convergence() {
    let seconds = 3;
    let (near, out) = echo_scenario(seconds, 64, 4);
    let tail = FRAME_SAMPLES * 100..;
    let erle = to_db(power(&near[tail.clone()]) / power(&out[tail]));
    println!("echo return loss enhancement after convergence: {erle:.1} dB");
    assert!(erle >= 15.0, "ERLE {erle:.1} dB below 15 dB");
}

#[test]
fn echo_canceller_passes_near_end_speech_it_cannot_predict() {
    // With no far-end signal the canceller has nothing to subtract, so the output equals the input.
    let mut ec = EchoCanceller::with_tail_ms(16);
    let mut rng = Rng::new(6);
    let near: Vec<f32> = (0..FRAME_SAMPLES * 4)
        .map(|_| 0.1 * rng.bipolar())
        .collect();
    let mut out = near.clone();
    for chunk in out.as_chunks_mut::<FRAME_SAMPLES>().0 {
        ec.process(chunk);
    }
    assert_eq!(out, near);
}
