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
//!
//! `DownlinkRecovery` is the forwarder's side: it lets a downlink's video cap climb back after the link's estimate fell.

use std::time::{Duration, Instant};

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

/// The fall below its last level that counts as renewed delay on a recovering downlink, in percent.
const RECOVERY_FALL_PERCENT: u32 = 85;
/// How much a recovering downlink's cap rises at each step, in percent.
const RECOVERY_STEP_PERCENT: u32 = 30;
/// How often a recovering downlink's cap rises while the link stays clean.
const RECOVERY_STEP_EVERY: Duration = Duration::from_secs(4);
/// The wait before a downlink's first step up, after a fall. Each renewed fall doubles it, up to `RECOVERY_MAX_HOLD`.
const RECOVERY_HOLD: Duration = Duration::from_secs(4);
const RECOVERY_MAX_HOLD: Duration = Duration::from_secs(60);

/// What a forwarder lets through to one downlink after that link's estimate fell.
///
/// A forwarder does not probe its downlinks, and an estimate rises only on what is sent, so a downlink whose estimate
/// fell once, to a delay spike, stayed held below its higher layers for the rest of the call. While the link shows no
/// loss and its estimate does not fall again, the cap rises a step above the estimate every few seconds. Each step lets
/// a higher layer through, which sends more, and that is the probe: a link that carries it raises its estimate, and one
/// that cannot shows delay or loss, which drops the cap back to the estimate and doubles the wait before the next try.
/// At the ceiling, where an estimate starts, the link counts as unlimited again.
#[derive(Debug, Clone)]
pub struct DownlinkRecovery {
    ceiling_kbps: u32,
    /// The cap now, or `None` while the estimate is at the ceiling.
    cap: Option<u32>,
    /// The estimate when the cap last moved. A fall below it is renewed delay.
    level: u32,
    hold: Duration,
    next_step: Option<Instant>,
}

impl DownlinkRecovery {
    /// A downlink whose estimate starts at `ceiling_kbps`, where the forwarder sets no cap.
    pub fn new(ceiling_kbps: u32) -> Self {
        Self { ceiling_kbps, cap: None, level: 0, hold: RECOVERY_HOLD, next_step: None }
    }

    /// Takes the link's estimate and the loss its peer reports (a fraction from 0 to 1) at `now`. Returns the video cap
    /// in kbps, or `None` for none.
    pub fn observe(&mut self, now: Instant, estimate_kbps: u32, loss: f64) -> Option<u32> {
        let Some(cap) = self.cap else {
            if estimate_kbps >= self.ceiling_kbps {
                return None;
            }
            self.hold_at(now, estimate_kbps, self.hold);
            return self.cap;
        };
        if estimate_kbps >= self.ceiling_kbps {
            *self = Self::new(self.ceiling_kbps);
            return None;
        }
        let fell = u64::from(estimate_kbps) * 100 < u64::from(self.level) * u64::from(RECOVERY_FALL_PERCENT);
        if fell || loss > LOSS_LOW {
            self.hold = (self.hold * 2).min(RECOVERY_MAX_HOLD);
            self.hold_at(now, estimate_kbps, self.hold);
        } else if estimate_kbps > cap {
            self.cap = Some(estimate_kbps);
            self.level = estimate_kbps;
        } else if self.next_step.is_some_and(|at| now >= at) {
            let up = u64::from(cap) * u64::from(100 + RECOVERY_STEP_PERCENT) / 100;
            if up >= u64::from(self.ceiling_kbps) {
                *self = Self::new(self.ceiling_kbps);
                return None;
            }
            self.cap = Some(up as u32);
            self.level = estimate_kbps;
            self.next_step = Some(now + RECOVERY_STEP_EVERY);
        }
        self.cap
    }

    /// Caps the link at its estimate and waits `hold` before the next step up.
    fn hold_at(&mut self, now: Instant, estimate_kbps: u32, hold: Duration) {
        self.cap = Some(estimate_kbps);
        self.level = estimate_kbps;
        self.next_step = Some(now + hold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEILING: u32 = 1_000;

    /// Feeds `recovery` one estimate a second, at seconds `from..from + seconds` after `t0`. Returns the cap at each.
    fn run(recovery: &mut DownlinkRecovery, t0: Instant, from: u64, seconds: u64, estimate: u32, loss: f64) -> Vec<Option<u32>> {
        (from..from + seconds).map(|s| recovery.observe(t0 + Duration::from_secs(s), estimate, loss)).collect()
    }

    #[test]
    fn a_downlink_whose_estimate_fell_once_climbs_back_to_no_cap_within_half_a_minute() {
        let t0 = Instant::now();
        let mut recovery = DownlinkRecovery::new(CEILING);
        assert_eq!(recovery.observe(t0, CEILING, 0.0), None, "no cap at the start");
        // A delay spike leaves the estimate at 300, and nothing raises it again.
        let caps = run(&mut recovery, t0, 1, 30, 300, 0.0);
        assert_eq!(caps[0], Some(300), "the cap follows the fall at once");
        let free = caps.iter().position(Option::is_none).expect("the cap goes within 30 s");
        assert!(free <= 25, "the cap goes after {free} s: {caps:?}");
        assert!(caps[..free].windows(2).all(|w| w[1] >= w[0]), "it only rises: {caps:?}");
    }

    #[test]
    fn a_lossy_downlink_stays_at_its_estimate() {
        let t0 = Instant::now();
        let mut recovery = DownlinkRecovery::new(CEILING);
        let caps = run(&mut recovery, t0, 0, 120, 300, 0.08);
        assert!(caps.iter().all(|c| *c == Some(300)), "the cap never rises over a lossy link: {caps:?}");
    }

    #[test]
    fn a_step_that_brings_delay_drops_back_and_waits_longer_for_the_next() {
        let t0 = Instant::now();
        let mut recovery = DownlinkRecovery::new(CEILING);
        run(&mut recovery, t0, 0, 4, 300, 0.0);
        assert_eq!(recovery.observe(t0 + Duration::from_secs(4), 300, 0.0), Some(390), "the first step up");
        // The higher layer brings delay: the estimate falls below where the step left it.
        assert_eq!(recovery.observe(t0 + Duration::from_secs(5), 240, 0.0), Some(240), "back to the estimate");
        let caps = run(&mut recovery, t0, 6, 7, 240, 0.0);
        assert!(caps.iter().all(|c| *c == Some(240)), "the next step waits 8 s, double the first: {caps:?}");
        assert_eq!(recovery.observe(t0 + Duration::from_secs(13), 240, 0.0), Some(312));
    }

    #[test]
    fn an_estimate_that_rises_on_its_own_lifts_the_cap_and_one_at_the_ceiling_clears_it() {
        let t0 = Instant::now();
        let mut recovery = DownlinkRecovery::new(CEILING);
        assert_eq!(recovery.observe(t0, 300, 0.0), Some(300));
        assert_eq!(recovery.observe(t0 + Duration::from_secs(1), 600, 0.0), Some(600));
        assert_eq!(recovery.observe(t0 + Duration::from_secs(2), CEILING, 0.0), None);
    }

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
