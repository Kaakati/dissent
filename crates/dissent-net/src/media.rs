use std::{fmt, sync::Arc};

use dissent_identity::DeviceId;
use iroh::{
    Endpoint, EndpointId,
    endpoint::{Connection, SendDatagramError},
    protocol::{AcceptError, ProtocolHandler},
};

use crate::{
    error::{NetError, Result},
    ping::{PathKind, path_kind},
    tls::log_key_exchange,
};

pub(crate) const MEDIA_ALPN: &[u8] = b"dissent/media/0";

/// Receives inbound media links, one call per incoming connection.
///
/// Register it with `NodeConfig.media`, the same way as the RPC handler. It runs on the router's
/// task, so it must not block. Keep the link: once every clone is dropped the connection closes.
pub trait MediaHandler: Send + Sync + 'static {
    fn incoming(&self, from: DeviceId, link: MediaLink);
}

/// An unreliable datagram link to one peer, carried over an iroh connection.
///
/// Payloads are never fragmented. Keep each one within [`Self::max_datagram_size`], which is
/// usually about one packet. Datagrams can be lost or reordered.
#[derive(Clone)]
pub struct MediaLink {
    pub peer: DeviceId,
    conn: Connection,
}

impl fmt::Debug for MediaLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaLink").field("peer", &self.peer).finish_non_exhaustive()
    }
}

impl MediaLink {
    /// Queues one datagram. Fails with `FrameTooLarge` if it exceeds the maximum size.
    pub fn send(&self, bytes: Vec<u8>) -> Result<()> {
        let len = bytes.len();
        self.conn.send_datagram(bytes.into()).map_err(|e| match e {
            SendDatagramError::TooLarge => {
                NetError::FrameTooLarge { len, max: self.max_datagram_size().unwrap_or(0) }
            }
            other => NetError::Datagram(other.to_string()),
        })
    }

    /// Waits for the next datagram. Returns an error once the link is closed.
    pub async fn recv(&self) -> Result<Vec<u8>> {
        Ok(self.conn.read_datagram().await?.to_vec())
    }

    /// Largest payload `send` accepts now. `None` if datagrams are unavailable.
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.conn.max_datagram_size()
    }

    /// Whether the peer is reachable directly or only through a relay right now.
    pub fn path(&self) -> PathKind {
        path_kind(&self.conn)
    }

    /// Round-trip time of the selected path in milliseconds. `None` before one is known.
    pub fn rtt_ms(&self) -> Option<u64> {
        self.conn
            .paths()
            .iter()
            .find(|p| p.is_selected())
            .map(|p| u64::try_from(p.rtt().as_millis()).unwrap_or(u64::MAX))
    }

    /// Closes the connection. The peer's pending and later `recv` calls return an error.
    pub fn close(&self) {
        self.conn.close(0u32.into(), b"media closed");
    }
}

pub(crate) struct MediaProtocol(pub(crate) Arc<dyn MediaHandler>);

impl fmt::Debug for MediaProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaProtocol")
    }
}

impl ProtocolHandler for MediaProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let peer = DeviceId(*connection.remote_id().as_bytes());
        log_key_exchange(&connection, peer, "accept");
        self.0.incoming(peer, MediaLink { peer, conn: connection });
        Ok(())
    }
}

pub(crate) async fn dial_media(endpoint: &Endpoint, peer: DeviceId) -> Result<MediaLink> {
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let conn = endpoint.connect(id, MEDIA_ALPN).await?;
    log_key_exchange(&conn, peer, "dial");
    Ok(MediaLink { peer, conn })
}
