//! Which address source resolved each dial, and how long the dial took. Every lookup service is
//! wrapped by `Recorded`, which notes the first address each dial receives. The label is the
//! item's provenance when it names a known source, or the service's own label otherwise.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use futures_lite::StreamExt;
use iroh::{
    EndpointId,
    address_lookup::{AddressLookup, Error, Item},
};
use n0_future::boxed::BoxStream;

/// Label for addresses already held on this device: the cache of signed records and remembered
/// peers.
pub const CACHE: &str = "cache";
/// Label for an address a scoped peer returned in a live lookup.
pub const PEER: &str = "peer";
/// Label for an address a seeder returned in a live lookup.
pub const SEEDER: &str = "seeder";
/// Label for an address learned on the local network, after a scoped handshake.
pub const LAN: &str = "lan";
/// Label for an address found on the Mainline DHT.
pub const DHT: &str = "dht";
/// Label for a dial that needed no lookup: the connection was already open.
pub const OPEN: &str = "open";

const KNOWN: [&str; 5] = [CACHE, PEER, SEEDER, LAN, DHT];

/// Counts of dials by the source that resolved them, plus the most recent dial.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialStats {
    /// Successful dials per source label. See the label constants.
    pub by_source: BTreeMap<&'static str, u64>,
    /// Dials that failed. They are not counted by source.
    pub failed: u64,
    pub last: Option<DialSample>,
}

/// One dial: the source that resolved it and the time from start to the reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialSample {
    pub via: &'static str,
    pub connect_ms: u64,
}

/// Remembers, per endpoint, which source delivered the first address of the dial in progress.
/// Dials to one device share that slot, so when two run at once a source can be credited to the
/// other dial. The counts are a guide to where addresses come from, not an exact ledger.
#[derive(Debug, Default)]
pub(crate) struct DialLog {
    winners: Mutex<BTreeMap<[u8; 32], &'static str>>,
    stats: Mutex<DialStats>,
}

impl DialLog {
    /// Clears the winner for `peer` before a dial starts.
    pub(crate) fn begin(&self, peer: [u8; 32]) {
        self.winners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&peer);
    }

    /// Records `label` as the source of `peer`'s dial, unless an earlier source already did.
    pub(crate) fn offer(&self, peer: [u8; 32], label: &'static str) {
        self.winners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(peer)
            .or_insert(label);
    }

    /// Ends the dial for `peer`, counts it and returns the source that resolved it. A dial that
    /// no service answered reused an open connection.
    pub(crate) fn finish(&self, peer: [u8; 32], connect_ms: u64, ok: bool) -> &'static str {
        let via = self
            .winners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&peer)
            .unwrap_or(OPEN);
        let mut stats = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        if ok {
            *stats.by_source.entry(via).or_insert(0) += 1;
            stats.last = Some(DialSample { via, connect_ms });
        } else {
            stats.failed += 1;
        }
        via
    }

    pub(crate) fn snapshot(&self) -> DialStats {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Wraps a lookup service so each dial's first address is recorded in `log`.
#[derive(Debug)]
pub(crate) struct Recorded<T> {
    inner: T,
    label: &'static str,
    log: Arc<DialLog>,
}

impl<T> Recorded<T> {
    pub(crate) fn new(label: &'static str, inner: T, log: Arc<DialLog>) -> Self {
        Self { inner, label, log }
    }
}

impl<T: AddressLookup> AddressLookup for Recorded<T> {
    fn publish(&self, data: &iroh::address_lookup::EndpointData) {
        self.inner.publish(data);
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        let log = self.log.clone();
        let label = self.label;
        let id = *endpoint_id.as_bytes();
        let stream = self.inner.resolve(endpoint_id)?;
        Some(
            stream
                .map(move |item| {
                    if let Ok(found) = &item {
                        let via = KNOWN
                            .into_iter()
                            .find(|known| *known == found.provenance())
                            .unwrap_or(label);
                        log.offer(id, via);
                    }
                    item
                })
                .boxed(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_offer_wins_and_dials_are_counted() {
        let log = DialLog::default();
        let peer = [7u8; 32];
        log.begin(peer);
        log.offer(peer, PEER);
        log.offer(peer, DHT);
        assert_eq!(log.finish(peer, 42, true), PEER);
        let stats = log.snapshot();
        assert_eq!(stats.by_source.get(PEER), Some(&1));
        assert_eq!(stats.by_source.get(DHT), None);
        assert_eq!(stats.last, Some(DialSample { via: PEER, connect_ms: 42 }));
    }

    #[test]
    fn a_dial_with_no_offer_reused_an_open_connection() {
        let log = DialLog::default();
        let peer = [9u8; 32];
        log.begin(peer);
        assert_eq!(log.finish(peer, 1, true), OPEN);
    }

    #[test]
    fn failed_dials_are_counted_apart_from_sources() {
        let log = DialLog::default();
        let peer = [3u8; 32];
        log.begin(peer);
        log.offer(peer, CACHE);
        log.finish(peer, 5, false);
        let stats = log.snapshot();
        assert_eq!(stats.failed, 1);
        assert!(stats.by_source.is_empty());
        assert_eq!(stats.last, None);
    }
}
