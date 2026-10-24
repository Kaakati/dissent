//! Warm connections. Pinging member devices ahead of use lets the network finish hole punching
//! before a message burst or a call, instead of during it.

use std::time::{Duration, Instant};

use dissent_identity::CommunityId;
use tokio::task::JoinSet;

use crate::engine::Engine;
use crate::error::Result;

/// Most member devices pinged per warm-up.
const WARM_LIMIT: usize = 16;
/// Shortest gap between two warm-ups of the same community.
const WARM_EVERY: Duration = Duration::from_secs(30);

impl Engine {
    /// Pings up to 16 member devices of `community` at once, so connections to them are ready.
    /// Returns how many answered. Returns 0 without pinging when there is nobody to ping yet, or
    /// when the community was warmed within the last 30 seconds.
    pub async fn warm(&self, community: CommunityId) -> Result<usize> {
        let me = self.inner.identity.device_id();
        let targets: Vec<_> = self
            .member_addrs(community)
            .into_iter()
            .filter(|signed| signed.addr.id != me)
            .take(WARM_LIMIT)
            .map(|signed| signed.addr)
            .collect();
        if targets.is_empty() {
            return Ok(0);
        }
        {
            let mut warmed = self
                .inner
                .warmed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if warmed
                .get(&community)
                .is_some_and(|at| at.elapsed() < WARM_EVERY)
            {
                return Ok(0);
            }
            warmed.insert(community, Instant::now());
        }
        let node = self.node();
        let mut pings = JoinSet::new();
        for addr in targets {
            let node = node.clone();
            pings.spawn(async move { node.ping(&addr).await.is_ok() });
        }
        let mut answered = 0;
        while let Some(done) = pings.join_next().await {
            if matches!(done, Ok(true)) {
                answered += 1;
            }
        }
        Ok(answered)
    }
}
