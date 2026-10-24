use std::{fmt, sync::Arc, time::Duration};

use dissent_identity::DeviceId;
use futures_lite::future::Boxed;
use iroh::{
    Endpoint, EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    error::{NetError, Result, stream},
    tls::log_key_exchange,
};

pub(crate) const RPC_ALPN: &[u8] = b"dissent/rpc/0";
/// Maximum frame payload, enforced in both directions.
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;
/// Overall budget for one request, dial included.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Answers one request. `from` is the remote device that opened the connection.
pub trait RpcHandler: Send + Sync + 'static {
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>>;
}

/// Protocol handler registered on the router for [`RPC_ALPN`].
pub(crate) struct RpcProtocol(pub(crate) Arc<dyn RpcHandler>);

impl fmt::Debug for RpcProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RpcProtocol")
    }
}

impl ProtocolHandler for RpcProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let from = DeviceId(*connection.remote_id().as_bytes());
        log_key_exchange(&connection, from, "accept");
        let (mut send, mut recv) = connection.accept_bi().await?;
        let req = read_frame(&mut recv, MAX_FRAME).await.map_err(AcceptError::from_err)?;
        let resp = self.0.handle(from, req).await;
        if let Err(e) = write_frame(&mut send, &resp, MAX_FRAME).await {
            // Close the whole connection so the requester fails fast instead of waiting.
            connection.close(1u32.into(), b"response rejected");
            return Err(AcceptError::from_err(e));
        }
        send.finish().map_err(AcceptError::from_err)?;
        // Hold the connection until the requester closes it, so the reply is not cut off.
        connection.closed().await;
        Ok(())
    }
}

/// Sends one request over a fresh bi-stream and returns the reply.
pub(crate) async fn run_request(endpoint: &Endpoint, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>> {
    if req.len() > MAX_FRAME {
        return Err(NetError::FrameTooLarge { len: req.len(), max: MAX_FRAME });
    }
    let id = EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?;
    let exchange = async {
        let conn = endpoint.connect(id, RPC_ALPN).await?;
        log_key_exchange(&conn, peer, "dial");
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(&mut send, &req, MAX_FRAME).await?;
        send.finish().map_err(stream)?;
        let resp = read_frame(&mut recv, MAX_FRAME).await?;
        conn.close(0u32.into(), b"done");
        Ok(resp)
    };
    tokio::time::timeout(REQUEST_TIMEOUT, exchange).await.map_err(|_| NetError::Timeout)?
}

/// Reads a `u32` big-endian length, then the payload. Rejects lengths above `max` before allocating.
pub(crate) async fn read_frame<R>(r: &mut R, max: usize) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.map_err(stream)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(NetError::FrameTooLarge { len, max });
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.map_err(stream)?;
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
        assert!(matches!(
            write_frame(&mut Vec::new(), &[0; 9], 8).await,
            Err(NetError::FrameTooLarge { len: 9, max: 8 })
        ));
    }
}
