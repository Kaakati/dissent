//! The DHT latency the status bar shows. Mainline does not report round-trip times per node, so the app times whole
//! lookups and reports the median of the recent ones.

use std::collections::VecDeque;
use std::time::Duration;

/// How many lookups the median is taken over. At one lookup every 15 seconds that is about two minutes.
pub const WINDOW: usize = 8;

/// The times of the most recent lookups. `None` is a lookup that no node answered.
#[derive(Debug, Default)]
pub struct LatencyWindow {
    samples: VecDeque<Option<Duration>>,
}

impl LatencyWindow {
    /// Records one lookup, dropping the oldest once the window is full.
    pub fn push(&mut self, sample: Option<Duration>) {
        if self.samples.len() == WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// The median time of the answered lookups, in whole milliseconds. `None` when no lookup has run, or when fewer
    /// than half of them were answered.
    pub fn median_ms(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut times: Vec<Duration> = self.samples.iter().flatten().copied().collect();
        if times.len() * 2 < self.samples.len() {
            return None;
        }
        times.sort();
        let mid = times.len() / 2;
        let median = if times.len() % 2 == 1 {
            times[mid]
        } else {
            (times[mid - 1] + times[mid]) / 2
        };
        Some(u64::try_from(median.as_millis()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Option<Duration> {
        Some(Duration::from_millis(n))
    }

    #[test]
    fn no_lookup_has_no_median() {
        assert_eq!(LatencyWindow::default().median_ms(), None);
    }

    #[test]
    fn the_median_of_an_odd_and_an_even_count() {
        let mut w = LatencyWindow::default();
        for n in [300, 100, 200] {
            w.push(ms(n));
        }
        assert_eq!(w.median_ms(), Some(200));
        w.push(ms(400));
        assert_eq!(w.median_ms(), Some(250), "the mean of the two middle values");
    }

    #[test]
    fn one_slow_lookup_does_not_move_the_median() {
        let mut w = LatencyWindow::default();
        for n in [80, 90, 85, 5000, 88] {
            w.push(ms(n));
        }
        assert_eq!(w.median_ms(), Some(88));
    }

    #[test]
    fn only_the_newest_window_counts() {
        let mut w = LatencyWindow::default();
        for _ in 0..WINDOW {
            w.push(ms(1000));
        }
        for _ in 0..WINDOW {
            w.push(ms(50));
        }
        assert_eq!(w.samples.len(), WINDOW, "the window holds {WINDOW} lookups");
        assert_eq!(w.median_ms(), Some(50), "the older lookups have left the window");
    }

    #[test]
    fn lookups_nobody_answered_count_against_the_median() {
        let mut w = LatencyWindow::default();
        w.push(ms(100));
        w.push(None);
        assert_eq!(w.median_ms(), Some(100), "half answered is still enough");
        w.push(None);
        assert_eq!(w.median_ms(), None, "fewer than half answered");
    }
}
