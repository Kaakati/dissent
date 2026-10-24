//! A call over an iroh media link. Datagrams travel as QUIC datagrams, and iroh handles NAT
//! traversal and relay fallback.

use std::net::SocketAddr;

use dissent_net::MediaLink;

use crate::error::{MediaError, Result};
use crate::transport::PacketTransport;
use crate::{SYNTHETIC_CALLEE, SYNTHETIC_CALLER};

/// Which side of the call this link serves. It picks the synthetic candidate address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The side that created the offer.
    Caller,
    /// The side that answered it.
    Callee,
}

/// Carries a call's datagrams over one [`MediaLink`].
///
/// The link has exactly one peer, so every datagram is reported from the peer's synthetic
/// address, and every datagram str0m sends goes to that one peer.
pub struct LinkTransport {
    link: MediaLink,
    local: SocketAddr,
    peer: SocketAddr,
}

impl LinkTransport {
    /// Wraps `link` for one side of a call.
    pub fn new(link: MediaLink, side: Side) -> Self {
        let (local, peer) = match side {
            Side::Caller => (SYNTHETIC_CALLER, SYNTHETIC_CALLEE),
            Side::Callee => (SYNTHETIC_CALLEE, SYNTHETIC_CALLER),
        };
        Self { link, local, peer }
    }

    /// The underlying media link, for path and RTT queries.
    pub fn link(&self) -> &MediaLink {
        &self.link
    }
}

impl PacketTransport for LinkTransport {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    fn max_datagram_size(&self) -> Option<usize> {
        self.link.max_datagram_size()
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> Result<()> {
        if to != self.peer {
            return Err(MediaError::BadState(
                "datagram addressed to an unknown peer",
            ));
        }
        self.link.send(bytes.to_vec()).map_err(MediaError::Link)
    }

    async fn recv(&self) -> Result<(SocketAddr, Vec<u8>)> {
        // The only way recv fails is that the link closed, on either side.
        let bytes = self.link.recv().await.map_err(|_| MediaError::Closed)?;
        Ok((self.peer, bytes))
    }
}
