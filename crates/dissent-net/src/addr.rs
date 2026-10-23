use std::{fmt, net::SocketAddr, str::FromStr};

use dissent_identity::DeviceId;
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{NetError, Result};

/// A dialable peer: device id plus optional relay and direct IP addresses.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PeerAddr {
    pub id: DeviceId,
    pub relay: Option<Url>,
    pub direct: Vec<SocketAddr>,
}

impl PeerAddr {
    /// Converts to iroh's address type. Fails if `id` is not a valid public key.
    pub fn to_iroh(&self) -> Result<EndpointAddr> {
        let id = EndpointId::from_bytes(&self.id.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
        let relay = self.relay.clone().map(|u| TransportAddr::Relay(RelayUrl::from(u)));
        let ips = self.direct.iter().map(|a| TransportAddr::Ip(*a));
        Ok(EndpointAddr::from_parts(id, relay.into_iter().chain(ips)))
    }

    /// Captures an iroh address, keeping the first relay URL and all IP addresses.
    pub fn from_iroh(addr: &EndpointAddr) -> Self {
        Self {
            id: DeviceId(*addr.id.as_bytes()),
            relay: addr.relay_urls().next().map(|r| Url::from(r.clone())),
            direct: addr.ip_addrs().copied().collect(),
        }
    }
}

/// Compact form for invite links: postcard bytes, lowercase base32 without padding.
impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = postcard::to_allocvec(self).map_err(|_| fmt::Error)?;
        f.write_str(&data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase())
    }
}

impl FromStr for PeerAddr {
    type Err = NetError;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.trim().to_ascii_uppercase().as_bytes())
            .map_err(|e| NetError::BadPeer(e.to_string()))?;
        let addr: Self = postcard::from_bytes(&bytes).map_err(|e| NetError::BadPeer(e.to_string()))?;
        addr.to_iroh()?;
        Ok(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_fromstr_roundtrip() {
        let addr = PeerAddr {
            id: dissent_identity::DeviceKey::generate().id(),
            relay: Some("https://relay.example.org/".parse().unwrap()),
            direct: vec!["192.0.2.1:4433".parse().unwrap(), "[2001:db8::1]:4433".parse().unwrap()],
        };
        let s = addr.to_string();
        assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_eq!(s.parse::<PeerAddr>().unwrap(), addr);
        assert_eq!(PeerAddr::from_iroh(&addr.to_iroh().unwrap()), addr);
    }

    #[test]
    fn rejects_garbage() {
        assert!("not base32!".parse::<PeerAddr>().is_err());
    }
}
