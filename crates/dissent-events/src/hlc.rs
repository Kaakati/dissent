//! Hybrid logical clock: a physical wall time plus a counter that breaks ties.

use serde::{Deserialize, Serialize};

/// Ordered by wall time, then counter. Field order matters for `Ord`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
}

/// Per-device clock. Every stamp it returns is strictly greater than the last.
#[derive(Clone, Copy, Debug, Default)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp a local event at physical time `wall_ms`.
    pub fn now(&mut self, wall_ms: u64) -> Hlc {
        self.last = if wall_ms > self.last.wall_ms {
            Hlc {
                wall_ms,
                counter: 0,
            }
        } else {
            Hlc {
                wall_ms: self.last.wall_ms,
                counter: self.last.counter.saturating_add(1),
            }
        };
        self.last
    }

    /// Merge a remote stamp, then stamp a local event at physical time `wall_ms`.
    pub fn observe(&mut self, remote: Hlc, wall_ms: u64) -> Hlc {
        let wall = wall_ms.max(self.last.wall_ms).max(remote.wall_ms);
        let counter = match (wall == self.last.wall_ms, wall == remote.wall_ms) {
            (true, true) => self.last.counter.max(remote.counter).saturating_add(1),
            (true, false) => self.last.counter.saturating_add(1),
            (false, true) => remote.counter.saturating_add(1),
            (false, false) => 0,
        };
        self.last = Hlc {
            wall_ms: wall,
            counter,
        };
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_strictly_monotonic_even_if_wall_goes_backwards() {
        let mut c = HlcClock::new();
        let walls = [100, 100, 50, 100, 101, 7, 7];
        let mut prev = None;
        for w in walls {
            let h = c.now(w);
            if let Some(p) = prev {
                assert!(h > p, "{h:?} not after {p:?}");
            }
            prev = Some(h);
        }
        assert_eq!(
            c.now(1000),
            Hlc {
                wall_ms: 1000,
                counter: 0
            }
        );
    }

    #[test]
    fn observe_moves_past_remote_and_local() {
        let mut c = HlcClock::new();
        let local = c.now(100);

        // Remote far ahead of local and physical clock: adopt its wall time.
        let remote = Hlc {
            wall_ms: 500,
            counter: 3,
        };
        let h = c.observe(remote, 100);
        assert!(h > remote);
        assert_eq!(
            h,
            Hlc {
                wall_ms: 500,
                counter: 4
            }
        );

        // Physical clock ahead of both: counter resets.
        let h = c.observe(
            Hlc {
                wall_ms: 10,
                counter: 9,
            },
            900,
        );
        assert_eq!(
            h,
            Hlc {
                wall_ms: 900,
                counter: 0
            }
        );
        assert!(h > local);

        // Same wall as remote and local: counter is above both.
        let mut d = HlcClock::new();
        let a = d.now(42);
        let b = d.observe(
            Hlc {
                wall_ms: 42,
                counter: 5,
            },
            0,
        );
        assert_eq!(
            b,
            Hlc {
                wall_ms: 42,
                counter: 6
            }
        );
        assert!(b > a);
    }
}
