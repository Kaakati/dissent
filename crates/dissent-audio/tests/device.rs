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
