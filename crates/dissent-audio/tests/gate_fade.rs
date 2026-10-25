//! Gate transitions. A frame that passes the gate after a closed one must not start from a jump
//! between speech and digital zero within one sample, since that is an audible click.

mod common;

use common::Rng;
use dissent_audio::{CaptureChain, ChainSettings, Threshold, FRAME_SAMPLES, SAMPLE_RATE};

/// Bursts of a 1 kHz tone, 200 ms on and 600 ms off, over a -70 dBFS floor. The pause is longer
/// than the gate's hold, so the gate closes in it. Each burst starts
/// at a phase of 1 rad, so an onset is mid-waveform, as speech onsets usually are.
fn bursts(seconds: usize, rng: &mut Rng) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let n = SAMPLE_RATE as usize * seconds;
    (0..n)
        .map(|t| {
            let local = t % 38400;
            if local < 9600 {
                0.3 * (std::f32::consts::TAU * 1000.0 * local as f32 / sr + 1.0).sin()
            } else {
                0.0003 * rng.bipolar()
            }
        })
        .collect()
}

#[test]
fn gate_transitions_do_not_click() {
    let mut rng = Rng::new(41);
    let input = bursts(4, &mut rng);
    let mut chain = CaptureChain::new(ChainSettings {
        threshold: Threshold::Manual(-50.0),
        ..ChainSettings::default()
    });
    let mut out = Vec::with_capacity(input.len());
    let mut edges = Vec::new();
    let mut was_open = false;
    for (k, chunk) in input.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
        let mut frame = chunk.to_vec();
        let report = chain.process(&mut frame, true);
        if report.open != was_open {
            edges.push(k * FRAME_SAMPLES);
            was_open = report.open;
        }
        out.extend_from_slice(&frame);
    }
    // The step between the last sample before a gate edge and the first one after it.
    let mut at_cut = 0.0f32;
    for &e in edges.iter().filter(|e| **e > 0) {
        at_cut = at_cut.max((out[e] - out[e - 1]).abs());
    }
    println!("gate edges {}: largest sample step at a gate edge {at_cut:.4}, edges at {:?}", edges.len(), &edges[..edges.len().min(8)]);
    assert!(at_cut < 0.01, "step {at_cut:.4} at a gate edge is an audible click");
}
