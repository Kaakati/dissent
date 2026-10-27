//! The latency the status bar shows. Every `dht_probe_every` one DHT lookup is timed, and the latest DHT and relay readings
//! are emitted, so the title bar updates without polling. The probe is one lookup per interval and nothing is stored.

use crate::engine::{Engine, Inner};
use crate::views::EngineEvent;
use std::sync::Weak;

/// Runs for the life of the engine. Does nothing when there is no DHT node (relays disabled, or loopback mode).
pub(crate) async fn dht_probe_loop(weak: Weak<Inner>) {
    let every = match weak.upgrade() {
        Some(inner) => inner.timings.dht_probe_every,
        None => return,
    };
    let mut tick = tokio::time::interval(every);
    loop {
        tick.tick().await;
        // Hold no engine handle across the lookup, so the engine can end while one is in flight.
        let Some(discovery) = weak.upgrade().and_then(|inner| inner.discovery.clone()) else {
            break;
        };
        discovery.probe_latency().await;
        let dht_ms = discovery.stats().await.ok().and_then(|stats| stats.latency_ms);
        let Some(inner) = weak.upgrade() else {
            break;
        };
        let engine = Engine { inner };
        engine.emit(EngineEvent::NetLatency {
            dht_ms,
            relay_ms: engine.relay_ms(),
        });
    }
}

impl Engine {
    /// The home relay's round-trip time in milliseconds, from the latest net report.
    pub(crate) fn relay_ms(&self) -> Option<u64> {
        let report = self.relay_report();
        let home = report.home?;
        let (_, latency) = report.latencies.iter().find(|(url, _)| *url == home)?;
        latency.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}
