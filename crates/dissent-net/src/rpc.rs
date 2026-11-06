use std::{fmt, sync::Arc, time::Duration};

use dissent_identity::DeviceId;
use futures_lite::future::Boxed;
use iroh::{
    Endpoint, EndpointId,
    endpoint::{Connection, ConnectionError, VarInt},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    error::{NetError, Result, stream},
    inbound::{BUSY, Inbound, Permit, TIMED_OUT},
    tls::log_key_exchange,
};

pub(crate) const RPC_ALPN: &[u8] = b"dissent/rpc/0";
/// Maximum frame payload, enforced in both directions.
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;
/// Overall budget for one request, dial included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Answers one request. `from` is the remote device that opened the connection.
pub trait RpcHandler: Send + Sync + 'static {
    /// The reply to `req`. An empty reply sends nothing: the connection is closed, as when a device
    /// goes away, so the requester cannot tell the device answered.
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>>;
}

/// Protocol handler registered on the router for [`RPC_ALPN`]. Every connection is admitted through `inbound`,
/// shared with the ping handler, and runs under its deadline.
pub(crate) struct RpcProtocol {
    pub(crate) handler: Arc<dyn RpcHandler>,
    pub(crate) inbound: Arc<Inbound>,
}

impl fmt::Debug for RpcProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RpcProtocol")
    }
}

impl ProtocolHandler for RpcProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let from = DeviceId(*connection.remote_id().as_bytes());
        log_key_exchange(&connection, from, "accept");
        let Some(permit) = self.inbound.admit(from).await else {
            connection.close(BUSY.into(), b"busy");
            return Ok(());
        };
        let served = tokio::select! {
            served = self.serve(&connection, &permit) => served,
            () = permit.cancelled() => {
                connection.close(BUSY.into(), b"busy");
                return Ok(());
            }
        };
        // Hold the connection until the requester closes it, so the reply is not cut off, but not for ever.
        if served.is_ok() {
            let _ = tokio::time::timeout(self.inbound.limits().close_wait, connection.closed()).await;
        }
        served
    }
}

impl RpcProtocol {
    /// Reads the request, answers it and sends the reply, all within the deadline its declared length allows.
    async fn serve(&self, connection: &Connection, permit: &Permit) -> std::result::Result<(), AcceptError> {
        let from = DeviceId(*connection.remote_id().as_bytes());
        let limits = *self.inbound.limits();
        let admitted = tokio::time::Instant::now();
        let header = tokio::time::timeout(limits.deadline_floor, async {
            let (send, mut recv) = connection.accept_bi().await.map_err(AcceptError::from_err)?;
            let len = read_len(&mut recv, MAX_FRAME).await.map_err(AcceptError::from_err)?;
            Ok::<_, AcceptError>((send, recv, len))
        });
        let (mut send, mut recv, len) = match header.await {
            Ok(header) => header?,
            Err(_) => return Err(timed_out(connection)),
        };
        let exchange = async {
            let req = read_payload_counted(&mut recv, len, |n| permit.received(n)).await.map_err(AcceptError::from_err)?;
            permit.done_reading();
            let resp = self.handler.handle(from, req).await;
            if resp.is_empty() {
                connection.close(0u32.into(), b"");
                return Ok(());
            }
            if let Err(e) = write_frame(&mut send, &resp, MAX_FRAME).await {
                // Close the whole connection so the requester fails fast instead of waiting.
                connection.close(1u32.into(), b"response rejected");
                return Err(AcceptError::from_err(e));
            }
            send.finish().map_err(AcceptError::from_err)
        };
        match tokio::time::timeout_at(admitted + limits.deadline(len), exchange).await {
            Ok(done) => done,
            Err(_) => Err(timed_out(connection)),
        }
    }
}

/// Closes a connection that ran out of time, so the peer learns at once.
fn timed_out(connection: &Connection) -> AcceptError {
    connection.close(TIMED_OUT.into(), b"timed out");
    AcceptError::from_err(NetError::Timeout)
}

/// Sends one request over a fresh bi-stream and returns the reply.
pub(crate) async fn run_request(endpoint: &Endpoint, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>> {
    if req.len() > MAX_FRAME {
        return Err(NetError::FrameTooLarge { len: req.len(), max: MAX_FRAME });
    }
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let exchange = async {
        loop {
            let conn = endpoint.connect(id, RPC_ALPN).await?;
            log_key_exchange(&conn, peer, "dial");
            match exchange_once(&conn, &req).await {
                Ok(resp) => {
                    conn.close(0u32.into(), b"done");
                    return Ok(resp);
                }
                // A busy node turned the request away, or gave its slot to another: try again shortly.
                Err(_) if turned_away(&conn) => tokio::time::sleep(BUSY_RETRY).await,
                Err(e) => return Err(e),
            }
        }
    };
    tokio::time::timeout(REQUEST_TIMEOUT, exchange).await.map_err(|_| NetError::Timeout)?
}

/// How long a requester waits before asking again a node that was busy.
const BUSY_RETRY: Duration = Duration::from_millis(250);

async fn exchange_once(conn: &Connection, req: &[u8]) -> Result<Vec<u8>> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req, MAX_FRAME).await?;
    send.finish().map_err(stream)?;
    read_frame(&mut recv, MAX_FRAME).await
}

/// Whether the peer closed `conn` because it was busy.
fn turned_away(conn: &Connection) -> bool {
    matches!(conn.close_reason(), Some(ConnectionError::ApplicationClosed(close)) if close.error_code == VarInt::from_u32(BUSY))
}

/// Reads a `u32` big-endian length, then the payload. Rejects lengths above `max` before allocating.
pub(crate) async fn read_frame<R>(r: &mut R, max: usize) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let len = read_len(r, max).await?;
    read_payload(r, len).await
}

/// Reads a frame's `u32` big-endian length. Rejects lengths above `max`.
pub(crate) async fn read_len<R>(r: &mut R, max: usize) -> Result<usize>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.map_err(stream)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(NetError::FrameTooLarge { len, max });
    }
    Ok(len)
}

/// First allocation for a payload. The buffer grows only as bytes arrive, so a peer that declares a large frame and
/// sends little makes this node hold little.
const FIRST_READ: usize = 64 * 1024;

/// Reads exactly `len` payload bytes, growing the buffer as they arrive.
pub(crate) async fn read_payload<R>(r: &mut R, len: usize) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    read_payload_counted(r, len, |_| {}).await
}

/// As [`read_payload`], and tells `arrived` how many bytes each read brought.
async fn read_payload_counted<R>(r: &mut R, len: usize, mut arrived: impl FnMut(usize)) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(len.min(FIRST_READ));
    let mut chunk = vec![0u8; len.clamp(1, FIRST_READ)];
    while buf.len() < len {
        let want = (len - buf.len()).min(chunk.len());
        let n = r.read(&mut chunk[..want]).await.map_err(stream)?;
        if n == 0 {
            return Err(NetError::Stream(format!("frame ended after {} of {len} bytes", buf.len())));
        }
        buf.extend_from_slice(&chunk[..n]);
        arrived(n);
    }
    Ok(buf)
}

/// Writes a `u32` big-endian length, then the payload. Refuses payloads above `max`.
pub(crate) async fn write_frame<W>(w: &mut W, payload: &[u8], max: usize) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > max {
        return Err(NetError::FrameTooLarge { len: payload.len(), max });
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| NetError::FrameTooLarge { len: payload.len(), max })?;
    w.write_all(&len.to_be_bytes()).await.map_err(stream)?;
    w.write_all(payload).await.map_err(stream)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip_and_cap() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"abc", 8).await.unwrap();
        assert_eq!(wire, [0, 0, 0, 3, b'a', b'b', b'c']);
        assert_eq!(read_frame(&mut &wire[..], 8).await.unwrap(), b"abc");
        // A declared length above the cap is rejected from the header alone.
        assert!(matches!(
            read_frame(&mut &[0, 0, 0, 9][..], 8).await,
            Err(NetError::FrameTooLarge { len: 9, max: 8 })
        ));
        // A frame that ends early is an error, not a short payload.
        assert!(read_frame(&mut &[0, 0, 0, 5, b'a'][..], 8).await.is_err());
        assert!(matches!(
            write_frame(&mut Vec::new(), &[0; 9], 8).await,
            Err(NetError::FrameTooLarge { len: 9, max: 8 })
        ));
    }
}
