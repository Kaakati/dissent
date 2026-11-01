//! What is moving right now, as this device sees it: bytes it sends to and receives from each device, per blob. Read
//! about once a second by an open file page, from memory only, so asking costs nothing on the network. Who else holds
//! what comes from `tracker::swarm`, at a slower pace.

use std::collections::{BTreeMap, HashMap, VecDeque};

use dissent_events::EventId;
use dissent_identity::{CommunityId, DeviceId, RootId};
use serde::Serialize;

use super::catalog::TRANSFER_PREFIX;
use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};

/// Samples closer together than this are merged.
const SAMPLE_MS: u64 = 250;
/// The span a rate is measured over.
const RATE_MS: u64 = 3_000;
/// How far back a meter can answer. Must reach past the time between two holdings answers.
const KEPT_MS: u64 = 120_000;
/// A meter with no bytes for this long is dropped.
const IDLE_MS: u64 = 600_000;

/// A running total of bytes, with enough history to tell its recent rate and what it was a while ago.
#[derive(Clone, Debug, Default)]
pub(crate) struct Meter {
    total: u64,
    samples: VecDeque<(u64, u64)>,
}

impl Meter {
    pub(crate) fn add(&mut self, now: u64, bytes: u64) {
        self.set_total(now, self.total.saturating_add(bytes));
    }

    /// Sets the running total. Only moves it forward.
    pub(crate) fn set_total(&mut self, now: u64, total: u64) {
        if self.samples.is_empty() {
            // A sample just before the first bytes, so a rate has a start to measure from.
            self.samples.push_back((now.saturating_sub(SAMPLE_MS), self.total));
        }
        self.total = self.total.max(total);
        match self.samples.back_mut() {
            Some((at, value)) if now.saturating_sub(*at) < SAMPLE_MS => *value = self.total,
            _ => self.samples.push_back((now, self.total)),
        }
        while self.samples.front().is_some_and(|(at, _)| now.saturating_sub(*at) > KEPT_MS) {
            self.samples.pop_front();
        }
    }

    /// The total as it stood at `at`.
    fn total_at(&self, at: u64) -> u64 {
        self.samples.iter().rev().find(|(t, _)| *t <= at).or(self.samples.front()).map_or(0, |(_, v)| *v)
    }

    /// Bytes per second over the last `RATE_MS`. Falls to zero once bytes stop.
    pub(crate) fn rate(&self, now: u64) -> u64 {
        let Some((first, _)) = self.samples.front() else {
            return 0;
        };
        let span = now.saturating_sub(*first).clamp(SAMPLE_MS, RATE_MS);
        let moved = self.total.saturating_sub(self.total_at(now.saturating_sub(RATE_MS)));
        moved.saturating_mul(1000) / span
    }

    /// Bytes since `at`.
    fn since(&self, at: u64) -> u64 {
        self.total.saturating_sub(self.total_at(at))
    }

    /// What the device at the other end holds by now, given its last answer and when it came. That answer may be up to
    /// the answerer's cache old, so this may fall short for a moment. It must not overshoot, so a share only grows.
    fn holds(&self, heard: Option<(u64, u64)>) -> u64 {
        let (bytes, at) = heard.unwrap_or((0, 0));
        self.total.max(bytes.saturating_add(self.since(at)))
    }

    fn idle(&self, now: u64) -> bool {
        self.samples.back().is_none_or(|(at, _)| now.saturating_sub(*at) > IDLE_MS)
    }
}

/// The meters of one blob, by the device at the other end.
#[derive(Debug, Default)]
pub(crate) struct BlobMeters {
    sent: HashMap<DeviceId, Meter>,
    received: HashMap<DeviceId, Meter>,
}

/// A device sent to: its id, the rate, and what it holds by now.
type SentTo = (DeviceId, u64, u64);

/// One member's part in what is moving.
#[derive(Clone, Debug, Serialize)]
pub struct LivePeer {
    pub root_id: String,
    pub name: String,
    /// What this device receives from them, in bytes per second.
    pub from_bps: u64,
    /// What this device sends them, in bytes per second.
    pub to_bps: u64,
    /// How much of the file they hold, 0 to 1: their last answer plus what they were sent since.
    pub has: f64,
}

/// What is moving for one listing.
#[derive(Clone, Debug, Serialize)]
pub struct LiveSwarm {
    pub active: bool,
    pub down_bps: u64,
    pub up_bps: u64,
    /// Seconds until this device's download finishes, at the present rate.
    pub eta_s: Option<u64>,
    /// This device's own download, 0 to 1.
    pub progress: Option<f64>,
    /// The pieces this device's download holds, as (start, end).
    pub own_pieces: Vec<(u64, u64)>,
    pub peers: Vec<LivePeer>,
}

impl Engine {
    /// Counts `bytes` of `blob` sent to `device`.
    pub(crate) fn meter_sent(&self, device: DeviceId, blob: [u8; 32], bytes: u64) {
        let now = now_ms();
        let mut state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.meters.entry(blob).or_default().sent.entry(device).or_default().add(now, bytes);
    }

    /// Sets the bytes of `blob` received so far from each of `totals`.
    pub(crate) fn meter_received(&self, blob: [u8; 32], totals: &[(DeviceId, u64)]) {
        let now = now_ms();
        let mut state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let meters = state.meters.entry(blob).or_default();
        for (device, total) in totals {
            meters.received.entry(*device).or_default().set_total(now, *total);
        }
    }

    /// The bytes per second `blob` comes in at.
    pub(crate) fn received_rate(&self, blob: &[u8; 32]) -> u64 {
        let now = now_ms();
        let state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.meters.get(blob).map_or(0, |m| m.received.values().map(|meter| meter.rate(now)).sum())
    }

    /// Drops the meters nothing has moved through for a while.
    pub(crate) fn prune_meters(&self) {
        let now = now_ms();
        let mut state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.meters.retain(|_, m| {
            m.sent.retain(|_, meter| !meter.idle(now));
            m.received.retain(|_, meter| !meter.idle(now));
            !m.sent.is_empty() || !m.received.is_empty()
        });
    }

    /// Whether anything moved through `blob` in the last `RATE_MS`.
    pub(crate) fn blob_moving(&self, blob: &[u8; 32]) -> bool {
        let now = now_ms();
        let state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.meters.get(blob).is_some_and(|m| m.sent.values().chain(m.received.values()).any(|meter| meter.rate(now) > 0))
    }

    /// What is moving for `listing`, from this device's own counts. Never touches the network.
    pub async fn listing_live(&self, community: CommunityId, listing: EventId) -> Result<LiveSwarm> {
        let opened = self.opened_listings(community);
        let o = opened
            .iter()
            .find(|o| o.id == listing)
            .ok_or_else(|| CoreError::NotFound(format!("listing {listing}")))?;
        let (blob, size) = (o.body.blob, o.body.cipher_size);
        let heard: BTreeMap<DeviceId, (u64, u64)> = self.with_tracker(community, |l| {
            l.heard
                .iter()
                .filter_map(|(d, h)| h.holdings.iter().find(|x| x.blob == blob).map(|x| (*d, (x.bytes, h.at_ms))))
                .collect()
        });
        let now = now_ms();
        let (sent, received): (Vec<SentTo>, Vec<(DeviceId, u64)>) = {
            let state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match state.meters.get(&blob) {
                Some(m) => (
                    m.sent
                        .iter()
                        .map(|(d, meter)| {
                            let holds = meter.holds(heard.get(d).copied());
                            (*d, meter.rate(now), holds)
                        })
                        .collect(),
                    m.received.iter().map(|(d, meter)| (*d, meter.rate(now))).collect(),
                ),
                None => (Vec::new(), Vec::new()),
            }
        };
        let (progress, downloading) = {
            let w = self.read();
            let wanted = format!("{TRANSFER_PREFIX}{community}:{listing}");
            match w.attach.transfers.values().find(|t| t.message_id == wanted && t.state == "active") {
                Some(t) => (Some(t.progress), true),
                None => (None, false),
            }
        };
        let mut peers: BTreeMap<RootId, LivePeer> = BTreeMap::new();
        {
            let w = self.read();
            let blank = |root: RootId| LivePeer { root_id: root.to_string(), name: w.display_name(&root), from_bps: 0, to_bps: 0, has: 0.0 };
            for (device, rate, holds) in &sent {
                if let Some(root) = w.directory.root_of(device) {
                    let peer = peers.entry(root).or_insert_with(|| blank(root));
                    peer.to_bps += *rate;
                    peer.has = peer.has.max((*holds as f64 / size.max(1) as f64).min(1.0));
                }
            }
            for (device, rate) in received.iter().filter(|_| downloading) {
                if let Some(root) = w.directory.root_of(device) {
                    peers.entry(root).or_insert_with(|| blank(root)).from_bps += *rate;
                }
            }
        }
        if let Some(progress) = progress {
            let me = self.inner.me;
            let name = self.read().display_name(&me);
            peers.entry(me).or_insert_with(|| LivePeer { root_id: me.to_string(), name, from_bps: 0, to_bps: 0, has: 0.0 }).has = progress;
        }
        // A finished download's rate is not left to fade.
        let down_bps: u64 = received.iter().filter(|_| downloading).map(|(_, r)| *r).sum();
        let up_bps: u64 = sent.iter().map(|(_, r, _)| *r).sum();
        let eta_s = progress
            .filter(|_| down_bps > 0)
            .map(|p| (((1.0 - p).max(0.0) * size as f64) / down_bps as f64).ceil() as u64);
        let own_pieces = if downloading { self.inner.files.held_pieces(blob, size).await.unwrap_or_default() } else { Vec::new() };
        Ok(LiveSwarm {
            active: downloading || down_bps > 0 || up_bps > 0,
            down_bps,
            up_bps,
            eta_s,
            progress,
            own_pieces,
            peers: peers.into_values().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_follows_the_bytes_and_falls_to_zero_when_they_stop() {
        let mut m = Meter::default();
        for s in 0..10u64 {
            m.add(s * 1000, 1_000_000);
        }
        let rate = m.rate(9_000);
        assert!((900_000..=1_100_000).contains(&rate), "{rate}");
        assert_eq!(m.rate(20_000), 0, "nothing moved in the last few seconds");
    }

    #[test]
    fn a_first_burst_is_not_measured_as_a_huge_rate_or_as_nothing() {
        let mut m = Meter::default();
        m.add(1_000, 1_000_000);
        assert_eq!(m.rate(1_000), 4_000_000, "measured over one sample");
        assert_eq!(m.rate(3_500), 363_636);
    }

    #[test]
    fn since_counts_only_what_came_after() {
        let mut m = Meter::default();
        m.add(1_000, 100);
        m.add(10_000, 100);
        m.add(20_000, 100);
        assert_eq!(m.since(10_000), 100);
        assert_eq!(m.since(1_000), 200);
        assert_eq!(m.since(0), 300);
    }

    #[test]
    fn a_fresh_answer_does_not_make_a_peers_share_jump() {
        let mut m = Meter::default();
        for s in 1..=20u64 {
            m.add(s * 1000, 1_000);
        }
        assert_eq!(m.holds(None), 20_000, "with no answer, what it was sent");
        assert_eq!(m.holds(Some((20_000, 20_000))), 20_000, "an answer just now adds nothing to itself");
        assert_eq!(m.holds(Some((5_000, 10_000))), 20_000, "an old answer does not lower it");
        assert_eq!(m.holds(Some((50_000, 10_000))), 60_000, "it had more from elsewhere, plus what came since");
    }

    #[test]
    fn totals_only_move_forward_and_history_is_bounded() {
        let mut m = Meter::default();
        m.set_total(0, 500);
        m.set_total(1_000, 100);
        assert_eq!(m.total, 500);
        for s in 0..1_000u64 {
            m.add(s * 1000, 1);
        }
        assert!(m.samples.len() as u64 <= KEPT_MS / 1000 + 2, "{}", m.samples.len());
    }
}
