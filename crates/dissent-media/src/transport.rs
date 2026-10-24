//! Packet I/O under a [`Call`](crate::Call).
//!
//! A call only needs to move datagrams. UDP is one way to do that. A call over iroh moves them
//! over QUIC datagrams instead, and the call logic does not change.

use std::future::Future;
use std::io::ErrorKind;
use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::error::Result;

/// Moves datagrams for one call.
///
/// The trait gives no ordering, reliability, or delivery guarantee. str0m copes with loss and
/// reordering, the way it does on UDP.
///
/// Addresses are the ones str0m sees. A transport with its own paths (such as iroh) reports
/// fixed synthetic addresses, see [`crate::SYNTHETIC_CALLER`] and [`crate::SYNTHETIC_CALLEE`].
pub trait PacketTransport: Send + Sync {
    /// The address this side advertises as its host candidate.
    fn local_addr(&self) -> SocketAddr;

    /// The largest datagram this transport carries, if it has a limit. The call sizes str0m's
    /// MTU to it, so str0m never emits a packet the transport refuses.
    fn max_datagram_size(&self) -> Option<usize> {
        None
    }

    /// Sends one datagram. `to` is the destination str0m chose for it. A transport with a single
    /// peer ignores it.
    fn send(&self, to: SocketAddr, bytes: &[u8]) -> impl Future<Output = Result<()>> + Send;

    /// Waits for one datagram and returns its source address with its bytes.
    fn recv(&self) -> impl Future<Output = Result<(SocketAddr, Vec<u8>)>> + Send;
}

/// Largest datagram read from a UDP socket. Above the MTU, so nothing is truncated.
const RECV_BUF: usize = 2048;

/// A plain UDP socket, the transport for loopback tests and direct IP paths.
pub struct UdpTransport {
    socket: UdpSocket,
    local: SocketAddr,
}

impl UdpTransport {
    /// Binds a UDP socket to `bind`.
    pub async fn bind(bind: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind(bind).await?;
        let local = socket.local_addr()?;
        Ok(Self { socket, local })
    }
}

impl PacketTransport for UdpTransport {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> Result<()> {
        match self.socket.send_to(bytes, to).await {
            Ok(_) => Ok(()),
            // Windows reports an earlier ICMP port-unreachable as ConnectionReset on send.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => {
                tracing::debug!(error = %e, "dropped outgoing datagram");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn recv(&self) -> Result<(SocketAddr, Vec<u8>)> {
        let mut buf = vec![0u8; RECV_BUF];
        let (len, source) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        Ok((source, buf))
    }
}
