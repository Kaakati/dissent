//! Needs real audio hardware, so it is ignored by default. Run with `cargo test -p dissent-audio
//! --test device -- --ignored --nocapture`.

use dissent_audio::{list_inputs, list_outputs};

#[test]
#[ignore = "needs audio hardware"]
fn lists_input_and_output_devices() {
    let inputs = list_inputs().expect("list inputs");
    let outputs = list_outputs().expect("list outputs");
    println!("inputs:");
    for d in &inputs {
        println!(
            "  {}{}",
            d.name,
            if d.is_default { " (default)" } else { "" }
        );
    }
    println!("outputs:");
    for d in &outputs {
        println!(
            "  {}{}",
            d.name,
            if d.is_default { " (default)" } else { "" }
        );
    }
}

/// The speaker at the voice loop's cadence: 960 samples at 48 kHz every 20 ms, for ten seconds, as the desktop
/// voice thread writes them. Prints the underrun counter, which counts output frames the callback had no sample for.
#[test]
#[ignore = "needs audio hardware: run with --ignored --nocapture"]
fn speaker_underruns_at_the_voice_cadence() {
    use dissent_audio::PlayoutStream;
    use std::time::{Duration, Instant};
    const TICK: Duration = Duration::from_millis(20);
    let mut playout = PlayoutStream::open(None).expect("open the default speaker");
    println!("device rate {} Hz", playout.device_rate());
    let mut block = vec![0f32; 960];
    let mut chain = std::env::var_os("DISSENT_TEST_PROCESS").map(|_| {
        dissent_audio::CaptureChain::new(dissent_audio::ChainSettings {
            noise_suppression: Some(dissent_audio::STANDARD_STRENGTH),
            echo_cancellation: true,
            threshold: dissent_audio::Threshold::Manual(-120.0),
            ..Default::default()
        })
    });
    let mut phase = 0f32;
    let mut next = Instant::now();
    let start = playout.underruns();
    let started = Instant::now();
    let blocks = 1500;
    for _ in 0..blocks {
        for s in block.iter_mut() {
            *s = 0.05 * phase.sin();
            phase += 0.05;
        }
        // With DISSENT_TEST_PROCESS=1 the capture chain runs before each write, as the voice thread's tick does: its
        // work delays the write, so the cushion has to cover it.
        if let Some(chain) = chain.as_mut() {
            let mut frame = block.clone();
            chain.process(&mut frame, true);
            playout.write_48k(&frame);
        } else {
            playout.write_48k(&block);
        }
        next += TICK;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    let elapsed = started.elapsed();
    // Read before the drain below: once the writes stop, the queue empties and the callback counts silence that is
    // not a fault of the pipeline.
    let during = playout.underruns() - start;
    std::thread::sleep(Duration::from_millis(200));
    println!("writer: {blocks} blocks in {:.3} s", elapsed.as_secs_f64());
    println!("speaker underruns during the writes: {during}");
}
