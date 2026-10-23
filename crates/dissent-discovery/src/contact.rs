//! Contact cards: a device's root-signed cert plus a display name and peer
//! addresses, shared as `dissent://contact/...` links.

use dissent_identity::DeviceCert;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::record::MAX_PEERS;
use crate::{from_link, to_link, DiscoveryError, Result};

const PREFIX: &str = "dissent://contact/";
/// Maximum display name length, in Unicode scalar values.
pub const MAX_NAME_CHARS: usize = 64;

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
    fn invite_url_is_not_a_contact() {
        let err = ContactCard::parse("dissent://invite/abcd").unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidContact(_)), "{err:?}");
    }
}
