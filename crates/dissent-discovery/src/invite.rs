//! Shareable invite links that carry a signed [`Genesis`] and peer addresses,
//! so joining skips the DHT and the joiner still learns the true owner and name.

use data_encoding::BASE32_NOPAD;
use dissent_identity::{CommunityId, Genesis};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
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

/// A short invite: `dissent://invite/<dss1 address>.<secret>.<generation>[.<invite id>]`, about 160
/// characters. It names the community by address and carries the secret. It has no genesis or peers:
/// a joiner finds them in the community's DHT record, which only holders of the secret can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortInvite {
    pub community: CommunityId,
    /// Random per community, as in [`Invite::secret`]. Also the key of the DHT record.
    pub secret: [u8; 32],
    /// Generation of `secret`, as in [`Invite::secret_generation`].
    pub secret_generation: u64,
    /// The id of an invite made with `InviteCreate`, as in [`Invite::invite_id`].
    pub invite_id: Option<[u8; 16]>,
}

impl ShortInvite {
    /// Renders the short invite as a `dissent://invite/...` URL.
    pub fn to_url(&self) -> Url {
        let mut payload = format!(
            "{}.{}.{}",
            self.community.address(),
            BASE32_NOPAD.encode(&self.secret).to_lowercase(),
            self.secret_generation
        );
        if let Some(id) = self.invite_id {
            payload.push('.');
            payload.push_str(&BASE32_NOPAD.encode(&id).to_lowercase());
        }
        Url::parse(&format!("{PREFIX}{payload}")).expect("an address and base32 are valid URL text")
    }

    /// Parses a short invite, as a full URL or a bare payload. Whitespace from wrapping is ignored.
    pub fn parse(s: &str) -> Result<Self> {
        let squeezed: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let payload = squeezed.strip_prefix(PREFIX).unwrap_or(&squeezed);
        let bad = |why: &str| DiscoveryError::InvalidInvite(why.to_string());
        let parts: Vec<&str> = payload.split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return Err(bad("a short invite has two to four dot-separated parts"));
        }
        let community = CommunityId::from_str(parts[0]).map_err(|e| bad(&format!("community address: {e}")))?;
        let secret = decode_b32::<32>(parts[1]).ok_or_else(|| bad("the secret is not 32 bytes"))?;
        let secret_generation = match parts.get(2) {
            Some(g) => g.parse::<u64>().map_err(|_| bad("the generation is not a number"))?,
            None => 0,
        };
        let invite_id = match parts.get(3) {
            Some(id) => Some(decode_b32::<16>(id).ok_or_else(|| bad("the invite id is not 16 bytes"))?),
            None => None,
        };
        Ok(Self {
            community,
            secret,
            secret_generation,
            invite_id,
        })
    }
}

/// Decodes unpadded base32 into exactly `N` bytes.
fn decode_b32<const N: usize>(s: &str) -> Option<[u8; N]> {
    BASE32_NOPAD.decode(s.to_ascii_uppercase().as_bytes()).ok()?.try_into().ok()
}

/// Either invite form. A short link has a `.` after its payload; a long one never does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InviteLink {
    Full(Invite),
    Short(ShortInvite),
}

impl InviteLink {
    /// Parses either form, as a URL or a bare payload.
    pub fn parse(s: &str) -> Result<Self> {
        let squeezed: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let payload = squeezed.strip_prefix(PREFIX).unwrap_or(&squeezed);
        if payload.contains('.') {
            ShortInvite::parse(s).map(Self::Short)
        } else {
            Invite::parse(s).map(Self::Full)
        }
    }

    /// The community this invite joins.
    pub fn community(&self) -> CommunityId {
        match self {
            Self::Full(invite) => invite.community(),
            Self::Short(short) => short.community,
        }
    }

    /// The invite secret, the key the community's DHT record is sealed with.
    pub fn secret(&self) -> [u8; 32] {
        match self {
            Self::Full(invite) => invite.secret,
            Self::Short(short) => short.secret,
        }
    }

    /// The generation of the invite secret.
    pub fn secret_generation(&self) -> u64 {
        match self {
            Self::Full(invite) => invite.secret_generation,
            Self::Short(short) => short.secret_generation,
        }
    }

    /// The invite id, if the link has one.
    pub fn invite_id(&self) -> Option<[u8; 16]> {
        match self {
            Self::Full(invite) => invite.invite_id,
            Self::Short(short) => short.invite_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::{CommunityKey, RootKey};

    fn short_sample(invite_id: Option<[u8; 16]>) -> ShortInvite {
        ShortInvite {
            community: CommunityKey::generate().id(),
            secret: [7; 32],
            secret_generation: 3,
            invite_id,
        }
    }

    #[test]
    fn short_invite_round_trips_with_and_without_its_id() {
        for short in [short_sample(Some([9; 16])), short_sample(None)] {
            let url = short.to_url();
            assert_eq!(url.scheme(), "dissent");
            assert_eq!(ShortInvite::parse(url.as_str()).unwrap(), short);
            let bare = url.as_str().strip_prefix(PREFIX).unwrap();
            assert_eq!(ShortInvite::parse(bare).unwrap(), short);
        }
    }

    /// The invite the README's join badge links to. It must parse with the real parser, and render back
    /// to the same URL.
    #[test]
    fn the_readme_invite_parses_and_renders_back() {
        let url = "dissent://invite/dss1pwq48a5vtxxfqgp8q23ztvvyx4wcnc7qewvpf7z3zex00sg66dnq0hf6cs.cmyq6evdjk6kl64a73qtdvqdkxhl646nu2mp3gggjhsxjbc56giq.0.gl4mkxwx4mqhyhjqhakel2j47u";
        let short = ShortInvite::parse(url).expect("the README invite parses");
        assert_eq!(short.secret_generation, 0);
        assert!(short.invite_id.is_some());
        assert_eq!(short.to_url().as_str(), url);
    }

    #[test]
    fn short_invite_is_far_shorter_than_the_long_form() {
        let short = short_sample(Some([9; 16])).to_url();
        assert!(short.as_str().len() <= 170, "{} chars", short.as_str().len());
        assert!(short.as_str().len() < sample().to_url().as_str().len() / 4);
    }

    #[test]
    fn short_invite_survives_wrapping() {
        let short = short_sample(Some([9; 16]));
        let url = short.to_url().to_string();
        let (a, b) = url.split_at(url.len() / 2);
        let wrapped = format!("  {a}\r\n {b}\n");
        assert_eq!(InviteLink::parse(&wrapped).unwrap(), InviteLink::Short(short));
    }

    #[test]
    fn invite_link_names_each_form() {
        let long = sample();
        assert!(matches!(InviteLink::parse(long.to_url().as_str()).unwrap(), InviteLink::Full(_)));
        let short = short_sample(Some([9; 16]));
        let parsed = InviteLink::parse(short.to_url().as_str()).unwrap();
        assert_eq!(parsed, InviteLink::Short(short.clone()));
        assert_eq!(parsed.community(), short.community);
        assert_eq!(parsed.secret(), [7; 32]);
        assert_eq!(parsed.invite_id(), Some([9; 16]));
    }

    #[test]
    fn short_invite_refuses_malformed_parts() {
        let good = short_sample(Some([9; 16])).to_url().to_string();
        let bad = [
            format!("{good}.1.2.3.4"),
            good.replace('.', ""),
            good.replacen(".3.", ".x.", 1),
            "dissent://invite/dss1.abc".to_string(),
        ];
        for s in bad {
            let err = InviteLink::parse(&s).unwrap_err();
            assert!(matches!(err, DiscoveryError::InvalidInvite(_)), "{s}: {err:?}");
        }
    }

    #[test]
    fn invite_link_still_parses_the_long_form() {
        let inv = sample();
        let parsed = InviteLink::parse(inv.to_url().as_str()).unwrap();
        assert_eq!(parsed, InviteLink::Full(inv));
    }

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
