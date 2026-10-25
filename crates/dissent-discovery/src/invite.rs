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
    /// Random per community. Proves the holder may join its gossip topic and sync with its members.
    pub secret: [u8; 32],
    /// Generation of `secret`. A joiner catches up on later rotations from it.
    pub secret_generation: u64,
    /// Relay servers the community asks members to use: https URLs, at most four.
    pub relays: Vec<String>,
    /// The id of an invite made with `InviteCreate`, which the community can revoke. Links made
    /// before invite ids existed have none, and stay valid until the secret rotates.
    pub invite_id: Option<[u8; 16]>,
}

/// An invite link from before `invite_id`. Postcard has no optional trailing field, so these
/// decode with this shape instead.
#[derive(Serialize, Deserialize)]
struct LegacyInvite {
    genesis: Genesis,
    peers: Vec<String>,
    secret: [u8; 32],
    secret_generation: u64,
    relays: Vec<String>,
}

impl From<LegacyInvite> for Invite {
    fn from(old: LegacyInvite) -> Self {
        Self {
            genesis: old.genesis,
            peers: old.peers,
            secret: old.secret,
            secret_generation: old.secret_generation,
            relays: old.relays,
            invite_id: None,
        }
    }
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
        let invite: Self = match from_link(PREFIX, s) {
            Ok(invite) => invite,
            Err(err) => from_link::<LegacyInvite>(PREFIX, s)
                .map(Self::from)
                .map_err(|_| DiscoveryError::InvalidInvite(err))?,
        };
        invite
            .genesis
            .verify()
            .map_err(|e| DiscoveryError::InvalidInvite(format!("genesis: {e}")))?;
        if invite.relays.len() > 4
            || !invite.relays.iter().all(|r| r.starts_with("https://") && r.len() > 8 && r.len() <= 256)
        {
            return Err(DiscoveryError::InvalidInvite("relays must be at most four https URLs".into()));
        }
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
            secret: [7; 32],
            secret_generation: 0,
            relays: vec!["https://relay.example.org".into()],
            invite_id: Some([9; 16]),
        }
    }

    #[test]
    fn links_from_before_invite_ids_still_parse() {
        let inv = sample();
        let old = LegacyInvite {
            genesis: inv.genesis.clone(),
            peers: inv.peers.clone(),
            secret: inv.secret,
            secret_generation: inv.secret_generation,
            relays: inv.relays.clone(),
        };
        let parsed = Invite::parse(to_link(PREFIX, &old).as_str()).unwrap();
        assert_eq!(parsed.invite_id, None);
        assert_eq!(parsed.secret, inv.secret);
        assert_eq!(parsed.peers, inv.peers);
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
