//! Community discovery over the Mainline DHT via Pkarr, plus shareable invites.
//!
//! A community's address is its public key ([`CommunityId`]). The community
//! publishes a [`CommunityRecord`] signed by its secret key, so anyone who knows
//! the ID can resolve current peer addresses without a server. An [`Invite`]
//! carries the same peer addresses directly so a joiner can connect without the DHT.

mod contact;
mod dns;
mod invite;
mod record;

use data_encoding::BASE32_NOPAD;
use dissent_identity::{CommunityId, CommunityKey, IdentityError};
use pkarr::dht::{DhtClient, DhtConfig, ResolveError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::{Mutex, PoisonError};
use tracing::{debug, warn};
use url::Url;

pub use contact::ContactCard;
pub use dns::{dns_record_value, normalize_domain, verify_dns_handle};
pub use invite::Invite;
pub use record::{CommunityRecord, MAX_PEERS};

/// A publish that stores on fewer nodes than this is retried; see [`Discovery::publish`].
pub const MIN_STORED_NODES: usize = 8;
/// A DHT node whose size estimate is at least this has joined the network. Cold nodes read single digits.
const WARM_SIZE_ESTIMATE: usize = 10_000;
const WARM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const WARM_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// Nodes started before giving up on warming up. A node that stays cold is replaced by a fresh one.
const MAX_NODE_STARTS: usize = 3;
/// Publish attempts below [`MIN_STORED_NODES`] on one node before that node is replaced.
const ATTEMPTS_PER_NODE: usize = 3;
/// Lookups that find no record are retried this many times in total, RESOLVE_RETRY_DELAY apart.
const RESOLVE_ATTEMPTS: usize = 3;
const RESOLVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);
const PUBLISH_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);
const PUBLISH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("record too large: {0} bytes (DHT packets are limited to 1000 bytes)")]
    RecordTooLarge(usize),
    #[error("encoding: {0}")]
    Encoding(String),
    #[error("pkarr: {0}")]
    Pkarr(String),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("invalid invite: {0}")]
    InvalidInvite(String),
    #[error("invalid contact card: {0}")]
    InvalidContact(String),
    #[error("invalid domain: {0}")]
    InvalidDomain(String),
    #[error("DNS lookup failed: {0}")]
    Dns(String),
}

pub type Result<T> = std::result::Result<T, DiscoveryError>;

/// Encodes `value` as `<prefix><base32 lowercase no-pad of postcard(value)>`.
fn to_link<T: Serialize>(prefix: &str, value: &T) -> Url {
    let bytes =
        postcard::to_allocvec(value).expect("postcard serialization of plain structs cannot fail");
    let payload = BASE32_NOPAD.encode(&bytes).to_lowercase();
    Url::parse(&format!("{prefix}{payload}")).expect("base32 payload is a valid URL")
}

/// Decodes a full link or a bare payload. Errors are plain strings for the caller to wrap.
fn from_link<T: DeserializeOwned>(prefix: &str, s: &str) -> std::result::Result<T, String> {
    let s = s.trim();
    let payload = s.strip_prefix(prefix).unwrap_or(s);
    let bytes = BASE32_NOPAD
        .decode(payload.to_ascii_uppercase().as_bytes())
        .map_err(|e| format!("bad base32: {e}"))?;
    let (value, rest) = postcard::take_from_bytes::<T>(&bytes).map_err(|e| e.to_string())?;
    if !rest.is_empty() {
        return Err("trailing bytes".into());
    }
    Ok(value)
}

/// Publishes and resolves [`CommunityRecord`]s on the Mainline DHT.
pub struct Discovery {
    /// One DHT node for publish, resolve and stats. The app's iroh lookup runs its own node.
    dht: Mutex<DhtClient>,
}

/// DHT network statistics for the status bar. Deliberately omits local and public addresses.
#[derive(Clone, Debug, Serialize)]
pub struct DhtStats {
    /// Estimated number of nodes in the Mainline DHT.
    pub size_estimate: usize,
    /// Confidence in `size_estimate`, from 0.0 to 1.0.
    pub confidence: f64,
    /// True if this node is likely behind a NAT or firewall.
    pub firewalled: bool,
}

impl Discovery {
    /// Starts a Mainline DHT node with pkarr's default configuration.
    pub fn new() -> Result<Self> {
        let dht = DhtClient::build(DhtConfig::default())
            .map_err(|e| DiscoveryError::Pkarr(format!("dht: {e}")))?;
        Ok(Self {
            dht: Mutex::new(dht),
        })
    }

    /// Current DHT size estimate and firewall state, for display.
    pub async fn stats(&self) -> Result<DhtStats> {
        let info = self.dht().info().await;
        let (size_estimate, confidence) = info.dht_size_estimate();
        Ok(DhtStats {
            size_estimate,
            confidence,
            firewalled: info.firewalled(),
        })
    }

    /// Signs `rec` with the community key and stores it on the DHT.
    ///
    /// Waits for the node to warm up first. A fresh node has an empty routing table for its first seconds. A put made then
    /// lands on one or two nodes, and resolvers rarely find it. So a put that stores on
    /// fewer than [`MIN_STORED_NODES`] nodes is retried every [`PUBLISH_RETRY_DELAY`]
    /// until [`PUBLISH_DEADLINE`] passes. Returns the highest count any attempt reached.
    /// It is an error only if no attempt stored on any node.
    pub async fn publish(&self, key: &CommunityKey, secret: &[u8; 32], rec: &CommunityRecord) -> Result<usize> {
        let packet = rec.to_private_packet(key, secret)?;
        self.wait_warm().await;
        let deadline = tokio::time::Instant::now() + PUBLISH_DEADLINE;
        let mut best = 0usize;
        let mut last_err = None;
        let mut short_on_node = 0;
        loop {
            match self.dht().publish(&packet).await {
                Ok(n) => best = best.max(n as usize),
                Err(e) => last_err = Some(e.to_string()),
            }
            debug!(stored_on = best, community = %key.id(), "dht publish attempt");
            if best >= MIN_STORED_NODES || tokio::time::Instant::now() >= deadline {
                break;
            }
            // Repeated short stores on one node mean its lookups are stuck. Start a fresh node.
            short_on_node += 1;
            if short_on_node >= ATTEMPTS_PER_NODE {
                short_on_node = 0;
                self.replace_dht();
            }
            tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
        }
        if best == 0 {
            return Err(DiscoveryError::Pkarr(
                last_err.unwrap_or_else(|| "no DHT node stored the record".into()),
            ));
        }
        debug!(stored_on = best, community = %key.id(), "published community record");
        Ok(best)
    }

    /// The current DHT node. Cheap to clone; all clones share one node.
    fn dht(&self) -> DhtClient {
        self.dht
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Waits until the DHT node's routing table is populated. A cold node's lookups end early
    /// and miss records. A node still cold after [`WARM_DEADLINE`] is replaced by a fresh one,
    /// up to [`MAX_NODE_STARTS`] nodes. After that it logs a warning and proceeds anyway.
    async fn wait_warm(&self) {
        for node in 1..=MAX_NODE_STARTS {
            let deadline = tokio::time::Instant::now() + WARM_DEADLINE;
            while tokio::time::Instant::now() < deadline {
                if self.dht().info().await.dht_size_estimate().0 >= WARM_SIZE_ESTIMATE {
                    return;
                }
                tokio::time::sleep(WARM_POLL_INTERVAL).await;
            }
            if node == MAX_NODE_STARTS {
                warn!(
                    nodes = MAX_NODE_STARTS,
                    "dht still has no peers after fresh nodes; publish and resolve may miss records"
                );
                break;
            }
            debug!(node, "dht node still cold; starting a fresh one");
            if !self.replace_dht() {
                return;
            }
        }
    }

    /// Swaps in a freshly started DHT node. Returns false if it could not be started.
    fn replace_dht(&self) -> bool {
        match DhtClient::build(DhtConfig::default()) {
            Ok(fresh) => {
                *self.dht.lock().unwrap_or_else(PoisonError::into_inner) = fresh;
                true
            }
            Err(e) => {
                debug!(error = %e, "could not start a fresh dht node");
                false
            }
        }
    }

    /// Fetches the most recent record for `id`. Returns `Ok(None)` if the DHT has none.
    ///
    /// Waits for the node to warm up first, then retries a lookup that finds nothing
    /// up to [`RESOLVE_ATTEMPTS`] times. A lookup from a young node can miss the record.
    pub async fn resolve(&self, id: &CommunityId, secret: &[u8; 32]) -> Result<Option<CommunityRecord>> {
        self.wait_warm().await;
        let public_key = record::dht_public_key(secret, id);
        // A lookup can end without reaching the nodes that hold the record. Retry before giving up.
        for attempt in 1..=RESOLVE_ATTEMPTS {
            let outcome = self.dht().resolve(&public_key, None).await.complete().await;
            match outcome.most_recent {
                Ok(packet) => {
                    return CommunityRecord::from_private_packet(&packet, id, secret).map(Some)
                }
                Err(ResolveError::NotFound) if attempt < RESOLVE_ATTEMPTS => {
                    tokio::time::sleep(RESOLVE_RETRY_DELAY).await;
                }
                Err(ResolveError::NotFound) => return Ok(None),
                Err(e) => return Err(DiscoveryError::Pkarr(e.to_string())),
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [7; 32];

    #[test]
    fn a_stranger_with_only_the_id_derives_another_dht_key() {
        let id = CommunityKey::generate().id();
        assert_ne!(
            record::dht_public_key(&SECRET, &id),
            record::dht_public_key(&[8; 32], &id),
            "without the secret the packet is elsewhere on the DHT"
        );
    }

    #[test]
    fn a_rotated_secret_moves_the_record_to_a_new_key() {
        let id = CommunityKey::generate().id();
        let before = record::dht_public_key(&SECRET, &id);
        let after = record::dht_public_key(&[9; 32], &id);
        assert_ne!(before, after);
    }

    /// Needs the Mainline DHT and network access; run with `cargo test -- --ignored`.
    #[ignore]
    #[tokio::test]
    async fn dht_stats_reports_network_size() {
        let discovery = Discovery::new().expect("client");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let stats = discovery.stats().await.expect("stats");
        println!("{stats:?}");
        assert!(stats.size_estimate > 0, "{stats:?}");
    }

    /// Needs the Mainline DHT and network access; run with `cargo test -- --ignored`.
    /// Reproduces the reported failure: publish on a fresh node, then resolve from another fresh node.
    /// Logs one `REPRO` line per run. Run with `--ignored --nocapture`.
    #[ignore]
    #[tokio::test]
    async fn publish_then_fresh_resolve() {
        let key = CommunityKey::generate();
        let rec = CommunityRecord {
            name: "repro".into(),
            peers: vec!["aaaabbbbcccc".into()],
            relays: vec![],
        };
        let publisher = Discovery::new().expect("publisher");
        let t0 = std::time::Instant::now();
        let stored = publisher.publish(&key, &SECRET, &rec).await;
        let publish_ms = t0.elapsed().as_millis();
        let pub_est = publisher.dht().info().await.dht_size_estimate().0;

        let resolver = Discovery::new().expect("resolver");
        let t1 = std::time::Instant::now();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            resolver.resolve(&key.id(), &SECRET),
        )
        .await;
        let resolve_ms = t1.elapsed().as_millis();
        let outcome = match &got {
            Err(_) => "timeout".to_string(),
            Ok(Ok(Some(_))) => "found".to_string(),
            Ok(Ok(None)) => "not_found".to_string(),
            Ok(Err(e)) => format!("error:{e}"),
        };
        println!("REPRO stored={stored:?} publish_ms={publish_ms} pub_size_est={pub_est} resolve_ms={resolve_ms} outcome={outcome}");
        assert!(
            matches!(got, Ok(Ok(Some(ref r))) if *r == rec),
            "resolve failed: {outcome}"
        );
    }

    #[ignore]
    #[tokio::test]
    async fn dht_publish_resolve() {
        let key = CommunityKey::generate();
        let rec = CommunityRecord {
            name: "dht-test".into(),
            peers: vec!["aaaabbbbcccc".into()],
            relays: vec![],
        };
        let discovery = Discovery::new().expect("client");
        discovery.publish(&key, &SECRET, &rec).await.expect("publish");
        let got = discovery.resolve(&key.id(), &SECRET).await.expect("resolve");
        assert_eq!(got, Some(rec));
    }
}

#[cfg(test)]
mod dht_privacy_tests {
    use super::*;

    /// Needs the Mainline DHT and network access. Run it through `scripts/fwtest.sh`.
    #[tokio::test]
    #[ignore]
    async fn a_stranger_with_only_the_id_finds_nothing_and_a_holder_finds_the_record() {
        let key = CommunityKey::generate();
        let id = key.id();
        let secret = [5u8; 32];
        let rec = CommunityRecord {
            name: "Private club".into(),
            peers: vec![],
            relays: vec![],
        };
        let discovery = Discovery::new().expect("dht node");
        discovery
            .publish(&key, &secret, &rec)
            .await
            .expect("publish");
        let holder = discovery.resolve(&id, &secret).await.expect("holder resolves");
        assert_eq!(holder, Some(rec));
        let stranger = discovery.resolve(&id, &[6u8; 32]).await.expect("stranger resolves");
        assert_eq!(stranger, None, "a stranger with only the id finds no record");
    }
}
