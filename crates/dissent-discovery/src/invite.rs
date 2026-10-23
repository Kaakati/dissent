//! Shareable invite links that carry a signed [`Genesis`] and peer addresses,
//! so joining skips the DHT and the joiner still learns the true owner and name.

use dissent_identity::{CommunityId, Genesis};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::record::MAX_PEERS;
use crate::{from_link, to_link, DiscoveryError, Result};

const PREFIX: &str = "dissent://invite/";

/// A community invite. Shared as `dissent://invite/<base32 of postcard(self)>`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub genesis: Genesis,
    /// Opaque peer addresses. At most [`MAX_PEERS`].
    pub peers: Vec<String>,
    pub secret: Option<[u8; 16]>,
}

impl Invite {
    /// The community this invite joins.
    pub fn community(&self) -> CommunityId {
        self.genesis.community
    }

    /// The community name, as signed in the genesis.
    pub fn name(&self) -> &str {
        &self.genesis.name
    }

    /// Renders the invite as a `dissent://invite/...` URL.
    pub fn to_url(&self) -> Url {
        to_link(PREFIX, self)
    }

    /// Parses a full invite URL or the bare base32 payload, and verifies the genesis.
    pub fn parse(s: &str) -> Result<Self> {
        let invite: Self = from_link(PREFIX, s).map_err(DiscoveryError::InvalidInvite)?;
        invite
            .genesis
            .verify()
            .map_err(|e| DiscoveryError::InvalidInvite(format!("genesis: {e}")))?;
        if invite.peers.len() > MAX_PEERS {
            return Err(DiscoveryError::InvalidInvite(format!(
                "{} peers exceeds maximum {MAX_PEERS}",
                invite.peers.len()
            )));
        }
        Ok(invite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::{CommunityKey, RootKey};

    fn sample() -> Invite {
        let key = CommunityKey::generate();
        let genesis = Genesis::create(&key, RootKey::generate().id(), "Rustaceans".into(), 1);
        Invite {
            genesis,
            peers: vec!["a".repeat(90), "b".repeat(90)],
            secret: Some([7; 16]),
        }
    }

    #[test]
    fn url_roundtrip() {
        let inv = sample();
        let url = inv.to_url();
        assert_eq!(url.scheme(), "dissent");
        let parsed = Invite::parse(url.as_str()).unwrap();
        assert_eq!(parsed, inv);
        assert_eq!(parsed.name(), "Rustaceans");
        assert_eq!(parsed.community(), inv.genesis.community);
    }

    #[test]
    fn bare_payload_parses() {
        let inv = sample();
        let url = inv.to_url();
        let bare = url.as_str().strip_prefix(PREFIX).unwrap();
        assert_eq!(Invite::parse(bare).unwrap(), inv);
    }

    #[test]
    fn tampered_genesis_name_is_rejected() {
        let mut inv = sample();
        inv.genesis.name = "Evil".into();
        let err = Invite::parse(inv.to_url().as_str()).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidInvite(_)), "{err:?}");
    }

    #[test]
    fn too_many_peers_is_rejected() {
        let mut inv = sample();
        inv.peers = vec!["p".into(); MAX_PEERS + 1];
        let err = Invite::parse(inv.to_url().as_str()).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidInvite(_)), "{err:?}");
    }

    #[test]
    fn garbage_is_invalid() {
        for s in [
            "",
            "not an invite!!",
            "dissent://invite/AAAA",
            "dissent://invite/!!!",
        ] {
            let err = Invite::parse(s).unwrap_err();
            assert!(
                matches!(err, DiscoveryError::InvalidInvite(_)),
                "{s}: {err:?}"
            );
        }
    }
}
