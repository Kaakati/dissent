//! The engine's real-time intervals. Production uses `Timings::default()`. Integration tests start
//! their engines with `test_support::fast_timings()`, so a wait that takes a minute in production
//! takes a few seconds in a test. Nothing else about the engine depends on which values are used.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// How long a presence beacon keeps its sender online.
    pub presence_window: Duration,
    /// How often a device sends its presence beacon. Whole seconds.
    pub beacon_every: Duration,
    /// How often peers whose beacon ran out of its window are reported. Whole seconds.
    pub presence_sweep: Duration,
    /// How long a direct call rings before it counts as unanswered.
    pub ring_timeout: Duration,
    /// How long a call must hold a media tier before it moves there.
    pub tier_sustain: Duration,
    /// How long a better forwarder must stay better before a call re-elects it.
    pub reelect_sustain: Duration,
    /// How often a call re-sends its roster while it is the current call.
    pub call_presence_every: Duration,
    /// How long a call roster entry stays live after its last refresh.
    pub call_presence_ttl: Duration,
    /// How long a typer stays listed after their last signal.
    pub typing_expiry: Duration,
    /// The shortest gap between two typing signals for one target.
    pub typing_every: Duration,
    /// How long a joiner waits before its first request for epoch keys, and the base of the backoff
    /// after a request that no member answers. Invite-card key requests start the same way.
    pub history_first_retry: Duration,
    /// How often the DHT latency shown in the status bar is measured. Whole seconds.
    pub dht_probe_every: Duration,
    /// How often a volunteer renews its offer to forward a call. An offer lapses after three of
    /// these unless renewed. See `call_volunteers`.
    pub forward_offer_every: Duration,
    /// How long a device in a boosted call may go unheard, after it is first seen live, before it counts as a
    /// released client. Until then the call keeps its tier. See `calls::effective_mesh_limit`.
    pub unheard_grace: Duration,
    /// A forwarded call that returned to a mesh at its limit and fell back within this long waits twice as long
    /// before it returns again. See `calls::tier_check`.
    pub tier_return_backoff: Duration,
    /// The longest a forwarded call waits to return to a mesh at its limit.
    pub tier_return_backoff_cap: Duration,
    /// How long a room just swapped in counts as settling. Its links are still ramping, so what it measures of this
    /// device's upload is low: until then that measure is neither announced nor allowed to move the call, and a
    /// limit lowered by capacity holds the tier. See `calls::settling_holds`.
    pub settle_after_swap: Duration,
    /// How long a send waits for the commit that adds a member this device already lists, so the new member can read
    /// it. After this it goes out at the current epoch. See `membership::join::wait_for_pending_adds`.
    pub pending_add_wait: Duration,
    /// How often a device sends its epoch check while its epoch stays the same. See `forks`.
    pub epoch_check_every: Duration,
    /// The shortest gap between two epoch checks for one community.
    pub epoch_check_gap: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            presence_window: Duration::from_secs(60),
            beacon_every: Duration::from_secs(30),
            presence_sweep: Duration::from_secs(15),
            ring_timeout: Duration::from_secs(45),
            tier_sustain: Duration::from_secs(10),
            reelect_sustain: Duration::from_secs(15),
            call_presence_every: Duration::from_secs(30),
            call_presence_ttl: Duration::from_secs(90),
            typing_expiry: Duration::from_secs(6),
            typing_every: Duration::from_secs(3),
            history_first_retry: Duration::from_secs(1),
            dht_probe_every: Duration::from_secs(15),
            forward_offer_every: Duration::from_secs(20),
            unheard_grace: Duration::from_secs(30),
            tier_return_backoff: Duration::from_secs(30),
            tier_return_backoff_cap: Duration::from_secs(300),
            settle_after_swap: Duration::from_secs(10),
            pending_add_wait: Duration::from_secs(2),
            epoch_check_every: Duration::from_secs(300),
            epoch_check_gap: Duration::from_secs(30),
        }
    }
}

/// A duration as whole milliseconds, for the clock arithmetic that works in `u64` ms.
pub(crate) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production values. Changing one of these changes how every user's presence, calls and
    /// typing behave, so the change should be deliberate and show up here.
    #[test]
    fn production_defaults_are_unchanged() {
        let t = Timings::default();
        assert_eq!(t.presence_window, Duration::from_secs(60));
        assert_eq!(t.beacon_every, Duration::from_secs(30));
        assert_eq!(t.presence_sweep, Duration::from_secs(15));
        assert_eq!(t.ring_timeout, Duration::from_secs(45));
        assert_eq!(t.tier_sustain, Duration::from_secs(10));
        assert_eq!(t.reelect_sustain, Duration::from_secs(15));
        assert_eq!(t.call_presence_every, Duration::from_secs(30));
        assert_eq!(t.call_presence_ttl, Duration::from_secs(90));
        assert_eq!(t.typing_expiry, Duration::from_secs(6));
        assert_eq!(t.typing_every, Duration::from_secs(3));
        assert_eq!(t.dht_probe_every, Duration::from_secs(15));
        assert_eq!(t.forward_offer_every, Duration::from_secs(20));
        assert_eq!(t.unheard_grace, Duration::from_secs(30));
        assert_eq!(t.tier_return_backoff, Duration::from_secs(30));
        assert_eq!(t.tier_return_backoff_cap, Duration::from_secs(300));
        assert_eq!(t.settle_after_swap, Duration::from_secs(10));
        assert_eq!(t.pending_add_wait, Duration::from_secs(2));
        assert_eq!(t.epoch_check_every, Duration::from_secs(300));
        assert_eq!(t.epoch_check_gap, Duration::from_secs(30));
    }

    #[test]
    fn millis_saturates_instead_of_wrapping() {
        assert_eq!(millis(Duration::from_secs(60)), 60_000);
        assert_eq!(millis(Duration::MAX), u64::MAX);
    }
}
