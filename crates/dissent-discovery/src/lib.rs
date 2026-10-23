//! Community discovery over the Mainline DHT via Pkarr, plus shareable invites.
//!
//! A community's address is its public key ([`CommunityId`]). The community
//! publishes a [`CommunityRecord`] signed by its secret key, so anyone who knows
//! the ID can resolve current peer addresses without a server. An [`Invite`]
//! carries the same peer addresses directly so a joiner can connect without the DHT.

mod contact;
mod invite;
mod record;

use data_encoding::BASE32_NOPAD;
use dissent_identity::{CommunityId, CommunityKey, IdentityError};
use pkarr::dht::{DhtClient, DhtConfig, ResolveError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::debug;
use url::Url;

pub use contact::ContactCard;
pub use invite::Invite;
pub use record::{CommunityRecord, MAX_PEERS};

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
    dht: DhtClient,
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
        Ok(Self { dht })
    }

    /// Current DHT size estimate and firewall state, for display.
    pub async fn stats(&self) -> Result<DhtStats> {
        let info = self.dht.info().await;
        let (size_estimate, confidence) = info.dht_size_estimate();
        Ok(DhtStats {
            size_estimate,
            confidence,
            firewalled: info.firewalled(),
        })
    }

    /// Signs `rec` with the community key and stores it on the DHT.
    pub async fn publish(&self, key: &CommunityKey, rec: &CommunityRecord) -> Result<()> {
        let packet = rec.to_signed_packet(key)?;
        let stored_on = self
            .dht
            .publish(&packet)
            .await
            .map_err(|e| DiscoveryError::Pkarr(e.to_string()))?;
        debug!(%stored_on, community = %key.id(), "published community record");
        Ok(())
    }

    /// Fetches the most recent record for `id`. Returns `Ok(None)` if the DHT has none.
    pub async fn resolve(&self, id: &CommunityId) -> Result<Option<CommunityRecord>> {
        let public_key = pkarr::PublicKey::try_from(id.as_bytes())
            .map_err(|e| DiscoveryError::Pkarr(e.to_string()))?;
        let outcome = self.dht.resolve(&public_key, None).await.complete().await;
        match outcome.most_recent {
            Ok(packet) => CommunityRecord::from_signed_packet_for(&packet, id).map(Some),
            Err(ResolveError::NotFound) => Ok(None),
            Err(e) => Err(DiscoveryError::Pkarr(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        discovery.publish(&key, &rec).await.expect("publish");
        let got = discovery.resolve(&key.id()).await.expect("resolve");
        assert_eq!(got, Some(rec));
    }
}
