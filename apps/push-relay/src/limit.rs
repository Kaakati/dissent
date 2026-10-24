//! Token-bucket rate limiter keyed by any hashable value.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Above this many tracked keys, idle buckets are dropped.
const PRUNE_ABOVE: usize = 10_000;
/// A bucket idle this long is dropped during a prune. A full bucket refills in far less time.
const IDLE_FOR: Duration = Duration::from_secs(3600);

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Lets each key make `burst` calls at once, then one call every `1 / rate_per_sec` seconds.
pub struct Limiter<K> {
    rate_per_sec: f64,
    burst: f64,
    buckets: Mutex<HashMap<K, Bucket>>,
}

impl<K: Hash + Eq + Clone> Limiter<K> {
    pub fn new(rate_per_sec: f64, burst: f64) -> Self {
        Self {
            rate_per_sec,
            burst,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// True if `key` may act now.
    pub fn allow(&self, key: &K) -> bool {
        self.allow_at(key, Instant::now())
    }

    /// True if `key` may act at `now`. Taking the time as an argument makes tests exact.
    pub fn allow_at(&self, key: &K, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() > PRUNE_ABOVE {
            buckets.retain(|_, b| now.saturating_duration_since(b.last) < IDLE_FOR);
        }
        let bucket = buckets.entry(key.clone()).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate_per_sec).min(self.burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_one_per_interval() {
        // 1 call per 10 s, bursts of 5.
        let limiter: Limiter<u8> = Limiter::new(0.1, 5.0);
        let t0 = Instant::now();
        let allowed: Vec<bool> = (0..6).map(|_| limiter.allow_at(&1, t0)).collect();
        assert_eq!(allowed, [true, true, true, true, true, false]);
        assert!(!limiter.allow_at(&1, t0 + Duration::from_secs(9)));
        assert!(limiter.allow_at(&1, t0 + Duration::from_secs(10)));
        assert!(!limiter.allow_at(&1, t0 + Duration::from_secs(10)));
    }

    #[test]
    fn keys_have_separate_buckets() {
        let limiter: Limiter<u8> = Limiter::new(0.1, 1.0);
        let t0 = Instant::now();
        assert!(limiter.allow_at(&1, t0));
        assert!(!limiter.allow_at(&1, t0));
        assert!(limiter.allow_at(&2, t0));
    }

    #[test]
    fn refill_never_exceeds_burst() {
        let limiter: Limiter<u8> = Limiter::new(1.0, 3.0);
        let t0 = Instant::now();
        assert!(limiter.allow_at(&1, t0));
        // A long idle period refills to the burst, not beyond it.
        let later = t0 + Duration::from_secs(3600);
        let allowed = (0..5).filter(|_| limiter.allow_at(&1, later)).count();
        assert_eq!(allowed, 3);
    }
}
