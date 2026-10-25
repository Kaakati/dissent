//! Contact cards: a device's root-signed cert plus a display name and peer
//! addresses, shared as `dissent://contact/...` links.

use dissent_identity::{DeviceCert, RootId};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::record::MAX_PEERS;
use crate::{from_link, to_link, DiscoveryError, Result};

const PREFIX: &str = "dissent://contact/";
/// Maximum display name length, in Unicode scalar values.
pub const MAX_NAME_CHARS: usize = 64;

/// The short contact link for `id`: `dissent://contact/<address>`. Opening it looks the address up
/// in its user record, so it works only while the address is discoverable.
pub fn short_url(id: &RootId) -> Url {
    Url::parse(&format!("{PREFIX}{}", id.address())).expect("an address is a valid URL payload")
}

/// What a contact link names: a full card, or only an address to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactLink {
    /// The long form. The card carries the cert, the name and dial addresses.
    Card(ContactCard),
    /// The short form, or a bare address. The address is resolved through its user record.
    Address(RootId),
}

impl ContactLink {
    /// Parses a long or short contact link, a bare card payload, or a bare address. Whitespace from
    /// wrapping is ignored. Input with a `1` is an address, since base32 payloads never contain one.
    pub fn parse(s: &str) -> Result<Self> {
        let squeezed: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let rest = squeezed.strip_prefix(PREFIX).unwrap_or(&squeezed);
        if rest.contains('1') {
            let id = RootId::parse_address(rest)
                .map_err(|e| DiscoveryError::InvalidContact(e.to_string()))?;
            return Ok(Self::Address(id));
        }
        ContactCard::parse(s).map(Self::Card)
    }
}

/// A shareable identity card. The cert binds the device to the root, so a
/// verified card proves which user owns the device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContactCard {
    pub cert: DeviceCert,
    pub display_name: String,
    pub peers: Vec<String>,
}

impl ContactCard {
    /// Renders the card as a `dissent://contact/...` URL.
    pub fn to_url(&self) -> Url {
        to_link(PREFIX, self)
    }

    /// Parses a full contact URL or the bare payload, and verifies the cert and fields.
    pub fn parse(s: &str) -> Result<Self> {
        let card: Self = from_link(PREFIX, s).map_err(DiscoveryError::InvalidContact)?;
        card.cert
            .verify()
            .map_err(|e| DiscoveryError::InvalidContact(format!("cert: {e}")))?;
        card.check_fields()?;
        Ok(card)
    }

    /// Trims the display name, then checks it and the peer cap.
    pub fn new(cert: DeviceCert, display_name: &str, peers: Vec<String>) -> Result<Self> {
        let card = Self {
            cert,
            display_name: display_name.trim().to_string(),
            peers,
        };
        card.check_fields()?;
        Ok(card)
    }

    fn check_fields(&self) -> Result<()> {
        let name = &self.display_name;
        if name.is_empty() || name.trim() != name {
            return Err(DiscoveryError::InvalidContact(
                "display name must be non-empty and trimmed".into(),
            ));
        }
        let chars = name.chars().count();
        if chars > MAX_NAME_CHARS {
            return Err(DiscoveryError::InvalidContact(format!(
                "display name is {chars} chars, maximum is {MAX_NAME_CHARS}"
            )));
        }
        if self.peers.len() > MAX_PEERS {
            return Err(DiscoveryError::InvalidContact(format!(
                "{} peers exceeds maximum {MAX_PEERS}",
                self.peers.len()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::Identity;

    fn cert() -> DeviceCert {
        Identity::create(1).cert
    }

    fn sample() -> ContactCard {
        ContactCard::new(cert(), "  Alice  ", vec!["peer-a".into()]).unwrap()
    }

    #[test]
    fn new_trims_display_name() {
        assert_eq!(sample().display_name, "Alice");
    }

    #[test]
    fn contact_roundtrip() {
        let card = sample();
        let url = card.to_url();
        assert_eq!(url.scheme(), "dissent");
        assert_eq!(ContactCard::parse(url.as_str()).unwrap(), card);
        let bare = url.as_str().strip_prefix(PREFIX).unwrap();
        assert_eq!(ContactCard::parse(bare).unwrap(), card);
    }

    #[test]
    fn tampered_cert_is_rejected() {
        let mut card = sample();
        card.cert.issued_at_ms += 1;
        let err = ContactCard::parse(card.to_url().as_str()).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidContact(_)), "{err:?}");
    }

    #[test]
    fn oversized_name_is_rejected() {
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        let err = ContactCard::new(cert(), &long, vec![]).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidContact(_)), "{err:?}");

        let mut card = sample();
        card.display_name = long;
        let err = ContactCard::parse(card.to_url().as_str()).unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidContact(_)), "{err:?}");
    }

    #[test]
    fn max_length_name_is_accepted() {
        let name = "é".repeat(MAX_NAME_CHARS);
        let card = ContactCard::new(cert(), &name, vec![]).unwrap();
        assert_eq!(ContactCard::parse(card.to_url().as_str()).unwrap(), card);
    }

    #[test]
    fn short_link_parses_as_an_address() {
        let root = dissent_identity::RootKey::generate().id();
        let url = short_url(&root);
        assert!(url.as_str().starts_with("dissent://contact/dsu1"));
        assert_eq!(ContactLink::parse(url.as_str()).unwrap(), ContactLink::Address(root));
        assert_eq!(ContactLink::parse(&root.address()).unwrap(), ContactLink::Address(root));
    }

    #[test]
    fn long_link_still_parses_as_a_card() {
        let card = sample();
        assert_eq!(ContactLink::parse(card.to_url().as_str()).unwrap(), ContactLink::Card(card));
    }

    #[test]
    fn server_address_and_typo_are_not_contacts() {
        let key = dissent_identity::CommunityKey::generate().id();
        assert!(ContactLink::parse(&key.address()).is_err());
        let root = dissent_identity::RootKey::generate().id().address();
        let typo = format!("{}{}", &root[..root.len() - 1], if root.ends_with('q') { 'p' } else { 'q' });
        assert!(ContactLink::parse(&typo).is_err());
    }

    #[test]
    fn invite_url_is_not_a_contact() {
        let err = ContactCard::parse("dissent://invite/abcd").unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidContact(_)), "{err:?}");
    }
}
