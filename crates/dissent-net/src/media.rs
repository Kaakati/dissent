use std::{fmt, sync::Arc, time::Duration};

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
/// Media links that name the offer they belong to. The dialler's first datagram is its link token. A peer whose
/// call `Features` say it takes these is dialled here; any other on [`MEDIA_ALPN`].
pub(crate) const MEDIA_TOKEN_ALPN: &[u8] = b"dissent/media/1";

/// Names one media link, so the offer that names it takes that link and no other. Random per offer.
pub type LinkToken = [u8; 16];

/// Prefix of the datagram that carries a link's token.
const LINK_TOKEN_DOMAIN: &[u8] = b"dissent/link-token/v1";
/// How long an accepted link may take to send its token. A link without one by then is closed.
pub const LINK_TOKEN_WAIT: Duration = Duration::from_secs(10);
/// Datagrams can be lost, so the dialler sends its token this many times, `TOKEN_RESEND` apart. The peer takes the
/// first and reads the rest as stray datagrams on the call, which drops them.
const TOKEN_SENDS: u32 = 5;
const TOKEN_RESEND: Duration = Duration::from_millis(100);

/// Receives inbound media links, one call per incoming connection.
///
/// Register it with `NodeConfig.media`, the same way as the RPC handler. It runs on the router's
/// task, so it must not block. Keep the link: once every clone is dropped the connection closes.
pub trait MediaHandler: Send + Sync + 'static {
    fn incoming(&self, from: DeviceId, link: MediaLink);

    /// A link dialled with a token, see [`MEDIA_TOKEN_ALPN`]. `from` is the dialler's authenticated device id.
    fn incoming_tokened(&self, from: DeviceId, token: LinkToken, link: MediaLink) {
        let _ = token;
        self.incoming(from, link);
    }
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

    /// The connection's id, stable for its life and unique among this endpoint's connections, for diagnostics.
    pub fn id(&self) -> usize {
        self.conn.stable_id()
    }

    /// Whether the connection has closed, on either side.
    pub fn is_closed(&self) -> bool {
        self.conn.close_reason().is_some()
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

/// Accepts links dialled with a token: waits up to [`LINK_TOKEN_WAIT`] for the token datagram, then hands the link on
/// under it. A link whose first datagram is not a token is closed.
pub(crate) struct MediaTokenProtocol(pub(crate) Arc<dyn MediaHandler>);

impl fmt::Debug for MediaTokenProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaTokenProtocol")
    }
}

impl ProtocolHandler for MediaTokenProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let peer = DeviceId(*connection.remote_id().as_bytes());
        log_key_exchange(&connection, peer, "accept");
        let link = MediaLink { peer, conn: connection };
        match tokio::time::timeout(LINK_TOKEN_WAIT, link.recv()).await {
            Ok(Ok(first)) => match parse_token(&first) {
                Some(token) => self.0.incoming_tokened(peer, token, link),
                None => link.close(),
            },
            _ => link.close(),
        }
        Ok(())
    }
}

/// The datagram that carries `token`.
fn token_datagram(token: &LinkToken) -> Vec<u8> {
    [LINK_TOKEN_DOMAIN, token.as_slice()].concat()
}

/// The token in `datagram`, if it is a token datagram.
fn parse_token(datagram: &[u8]) -> Option<LinkToken> {
    datagram.strip_prefix(LINK_TOKEN_DOMAIN)?.try_into().ok()
}

/// Dials `peer` on [`MEDIA_TOKEN_ALPN`] and sends `token` on the new link, [`TOKEN_SENDS`] times.
pub(crate) async fn dial_media_tokened(endpoint: &Endpoint, peer: DeviceId, token: LinkToken) -> Result<MediaLink> {
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let conn = endpoint.connect(id, MEDIA_TOKEN_ALPN).await?;
    log_key_exchange(&conn, peer, "dial");
    let link = MediaLink { peer, conn };
    link.send(token_datagram(&token))?;
    let resend = link.clone();
    tokio::spawn(async move {
        for _ in 1..TOKEN_SENDS {
            tokio::time::sleep(TOKEN_RESEND).await;
            if resend.is_closed() || resend.send(token_datagram(&token)).is_err() {
                return;
            }
        }
    });
    Ok(link)
}

/// Test hook: dials `peer` on [`MEDIA_TOKEN_ALPN`] and sends nothing, as a peer that never names its link would.
#[cfg(feature = "test-hooks")]
pub(crate) async fn dial_media_untokened(endpoint: &Endpoint, peer: DeviceId) -> Result<MediaLink> {
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let conn = endpoint.connect(id, MEDIA_TOKEN_ALPN).await?;
    Ok(MediaLink { peer, conn })
}

pub(crate) async fn dial_media(endpoint: &Endpoint, peer: DeviceId) -> Result<MediaLink> {
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let conn = endpoint.connect(id, MEDIA_ALPN).await?;
    log_key_exchange(&conn, peer, "dial");
    Ok(MediaLink { peer, conn })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_datagram_carries_its_token_behind_the_domain() {
        assert_eq!(LINK_TOKEN_DOMAIN, b"dissent/link-token/v1");
        assert_eq!(MEDIA_TOKEN_ALPN, b"dissent/media/1");
        let token = [7u8; 16];
        let datagram = token_datagram(&token);
        assert!(datagram.starts_with(LINK_TOKEN_DOMAIN));
        assert_eq!(parse_token(&datagram), Some(token));
    }

    #[test]
    fn anything_else_is_not_a_token() {
        let token = [7u8; 16];
        let mut short = token_datagram(&token);
        short.pop();
        assert_eq!(parse_token(&short), None, "one byte short");
        assert_eq!(parse_token(&[token_datagram(&token), vec![0]].concat()), None, "one byte long");
        assert_eq!(parse_token(&[0x00, 0x01, 0x00, 0x00]), None, "a STUN header");
        assert_eq!(parse_token(&token), None, "a token without its domain");
    }
}
