//! Per-receiver bandwidth estimation, a small version of Google Congestion Control.
//!
//! The estimator takes the round-trip time and the loss fraction that str0m reports each stats
//! interval, and returns a target audio bitrate. It works in two halves.
//!
//! - Delay-based: queuing delay is the RTT above the lowest RTT seen. A large queue means the
//!   path is full, so the target falls multiplicatively. An empty queue lets it rise slowly.
//! - Loss-based: heavy loss pulls the target down in proportion to the loss. Light loss lets it
//!   rise.
//!
//! The target is clamped to the range Opus can use for speech, 16 to 64 kbps.

/// The lowest audio bitrate the estimator returns, in kbps.
pub const MIN_KBPS: u32 = 16;
/// The highest audio bitrate the estimator returns, in kbps.
pub const MAX_KBPS: u32 = 64;

/// Starting point, before any measurement.
const START_KBPS: f64 = 32.0;
/// Queuing delay above which the path counts as full, in milliseconds.
const QUEUE_FULL_MS: f64 = 40.0;
/// Queuing delay below which the path counts as empty, in milliseconds.
const QUEUE_EMPTY_MS: f64 = 10.0;
/// Loss above which the target falls.
const LOSS_HIGH: f64 = 0.10;
/// Loss below which the target may rise.
const LOSS_LOW: f64 = 0.02;
/// Multiplicative decrease when the path is full.
const DECREASE: f64 = 0.85;
/// Additive increase per measurement when the path has room, in kbps.
const INCREASE_KBPS: f64 = 1.0;

/// Estimates how much audio bitrate a receiver can take.
#[derive(Debug, Clone, PartialEq)]
pub struct BandwidthEstimator {
    estimate: f64,
    rtt_min: Option<f64>,
    last_rtt: Option<f64>,
}

impl Default for BandwidthEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl BandwidthEstimator {
    /// An estimator at the starting point, 32 kbps.
    pub fn new() -> Self {
        Self {
            estimate: START_KBPS,
            rtt_min: None,
            last_rtt: None,
        }
    }

    /// Feeds one measurement: the round-trip time in milliseconds, and the loss as a fraction
    /// from 0 to 1. Returns the target bitrate in kbps.
    pub fn update(&mut self, rtt_ms: f64, loss: f64) -> u32 {
        let min = self.rtt_min.map_or(rtt_ms, |m| m.min(rtt_ms));
        self.rtt_min = Some(min);
        let queue = rtt_ms - min;
        let rising = self.last_rtt.is_some_and(|last| rtt_ms > last + 1.0);
        self.last_rtt = Some(rtt_ms);

        if queue > QUEUE_FULL_MS {
            self.estimate *= DECREASE;
        } else if queue < QUEUE_EMPTY_MS && !rising {
            self.estimate += INCREASE_KBPS;
        }

        if loss > LOSS_HIGH {
            self.estimate *= 1.0 - loss / 2.0;
        } else if loss < LOSS_LOW {
            self.estimate += INCREASE_KBPS;
        }

        self.estimate = self
            .estimate
            .clamp(f64::from(MIN_KBPS), f64::from(MAX_KBPS));
        self.target_kbps()
    }

    /// The current target bitrate in kbps.
    pub fn target_kbps(&self) -> u32 {
        self.estimate.round() as u32
    }
}

/// Picks the highest layer whose bitrate fits the budget. `layers_kbps` is ascending. If even the
/// lowest layer does not fit, it is still the layer to send, so the listener hears something.
/// Returns `None` when there are no layers.
///
/// Simulcast frames are not carried yet. This selection is what a forwarder will use per listener
/// once they are.
pub fn pick_layer(budget_kbps: u32, layers_kbps: &[u32]) -> Option<usize> {
    if layers_kbps.is_empty() {
        return None;
    }
    let fitting = layers_kbps.iter().rposition(|kbps| *kbps <= budget_kbps);
    Some(fitting.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_path_raises_the_target_to_the_top() {
        let mut bwe = BandwidthEstimator::new();
        let mut last = 0;
        for _ in 0..40 {
            last = bwe.update(20.0, 0.0);
        }
        assert_eq!(last, MAX_KBPS);
        assert!((MIN_KBPS..=MAX_KBPS).contains(&last));
    }

    #[test]
    fn heavy_loss_drops_the_target_to_the_floor() {
        let mut bwe = BandwidthEstimator::new();
        let mut last = 0;
        for _ in 0..40 {
            last = bwe.update(20.0, 0.15);
        }
        assert_eq!(last, MIN_KBPS);
    }

    #[test]
    fn a_filling_queue_lowers_the_target_before_any_loss() {
        let mut bwe = BandwidthEstimator::new();
        for _ in 0..10 {
            bwe.update(20.0, 0.0);
        }
        let before = bwe.target_kbps();
        // The RTT climbs 80 ms above the lowest seen, so the queue is full.
        let after = bwe.update(100.0, 0.0);
        assert!(after < before, "{after} should be below {before}");
    }

    #[test]
    fn a_rising_rtt_with_a_small_queue_holds_the_target() {
        let mut bwe = BandwidthEstimator::new();
        bwe.update(20.0, 0.0);
        let held = bwe.target_kbps();
        // Still under the full-queue threshold, but climbing: no increase.
        let after = bwe.update(25.0, 0.04);
        assert_eq!(after, held);
    }

    #[test]
    fn the_target_never_leaves_the_opus_range() {
        let mut bwe = BandwidthEstimator::new();
        for i in 0..200 {
            let loss = if i % 2 == 0 { 0.0 } else { 0.9 };
            let target = bwe.update(20.0 + f64::from(i), loss);
            assert!((MIN_KBPS..=MAX_KBPS).contains(&target));
        }
    }

    #[test]
    fn pick_layer_takes_the_best_layer_that_fits() {
        let layers = [16, 32, 64];
        assert_eq!(pick_layer(64, &layers), Some(2));
        assert_eq!(pick_layer(40, &layers), Some(1));
        assert_eq!(pick_layer(16, &layers), Some(0));
    }

    #[test]
    fn pick_layer_falls_back_to_the_lowest_layer_and_handles_no_layers() {
        assert_eq!(pick_layer(8, &[16, 32, 64]), Some(0));
        assert_eq!(pick_layer(100, &[]), None);
    }
}
