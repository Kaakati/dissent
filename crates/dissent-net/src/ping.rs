use std::time::Instant;

use dissent_identity::DeviceId;
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

use crate::{
    error::{NetError, Result, stream},
    tls::{is_pq, key_exchange, log_key_exchange},
};

pub(crate) const PING_ALPN: &[u8] = b"dissent/ping/0";
const PING: &[u8] = b"ping";
const PONG: &[u8] = b"pong";

/// Which network paths are open to the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Only IP (hole-punched or LAN) paths are open.
    Direct,
    /// Only relay paths are open.
    Relayed,
    /// Both IP and relay paths are open.
    Mixed,
    /// No path information is available.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingReport {
    pub rtt_ms: u64,
    pub path: PathKind,
    /// `Some(true)` if the handshake used hybrid X25519 + ML-KEM-768, `Some(false)` for a
    /// classical group, `None` if the handshake data was unavailable.
    pub pq: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct PingHandler;

impl ProtocolHandler for PingHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        log_key_exchange(&connection, DeviceId(*connection.remote_id().as_bytes()), "accept");
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request = recv.read_to_end(PING.len()).await.map_err(AcceptError::from_err)?;
        if request == PING {
            send.write_all(PONG).await.map_err(AcceptError::from_err)?;
        }
        send.finish().map_err(AcceptError::from_err)?;
        // Keep the connection alive until the pinger closes it, so the reply is not cut off.
        connection.closed().await;
        Ok(())
    }
}

pub(crate) async fn run_ping(endpoint: &Endpoint, addr: EndpointAddr) -> Result<PingReport> {
    let started = Instant::now();
    let peer = DeviceId(*addr.id.as_bytes());
    let conn = endpoint.connect(addr, PING_ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await.map_err(stream)?;
    send.finish().map_err(stream)?;
    let reply = recv.read_to_end(PONG.len()).await.map_err(stream)?;
    let rtt_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if reply != PONG {
        return Err(NetError::Protocol(format!("expected pong, got {} bytes", reply.len())));
    }
    let path = path_kind(&conn);
    log_key_exchange(&conn, peer, "dial");
    let pq = is_pq(key_exchange(&conn));
    conn.close(0u32.into(), b"ping done");
    Ok(PingReport { rtt_ms, path, pq })
}

pub(crate) fn path_kind(conn: &Connection) -> PathKind {
    let (mut direct, mut relayed) = (false, false);
    for path in conn.paths().iter() {
        if path.is_relay() {
            relayed = true;
        } else if path.is_ip() {
            direct = true;
        }
    }
    match (direct, relayed) {
        (true, true) => PathKind::Mixed,
        (true, false) => PathKind::Direct,
        (false, true) => PathKind::Relayed,
        (false, false) => PathKind::Unknown,
    }
}
