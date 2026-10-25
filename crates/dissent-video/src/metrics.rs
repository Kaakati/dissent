//! Picture quality measures used by the codec tests and the benchmark.

use crate::frame::Frame;

/// Luma peak signal-to-noise ratio in dB, or `f64::INFINITY` for identical luma.
///
/// Both frames must have the same size. Only the Y plane is compared, which is the usual
/// convention for video quality.
pub fn psnr_luma(reference: &Frame, decoded: &Frame) -> f64 {
    assert_eq!(
        (reference.width(), reference.height()),
        (decoded.width(), decoded.height()),
        "PSNR needs frames of the same size"
    );
    let squared: f64 = reference
        .luma()
        .iter()
        .zip(decoded.luma())
        .map(|(&a, &b)| {
            let d = a as f64 - b as f64;
            d * d
        })
        .sum();
    if squared == 0.0 {
        return f64::INFINITY;
    }
    let mse = squared / reference.luma().len() as f64;
    10.0 * (255.0_f64 * 255.0 / mse).log10()
}
