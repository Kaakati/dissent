//! Extra protocol handlers mounted on the node's router next to the built-in ones.

use std::sync::Arc;

use iroh::{
    endpoint::{Accepting, Connection},
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler},
};
use iroh_gossip::ALPN as GOSSIP_ALPN;

use crate::{
    error::{NetError, Result},
    media::MEDIA_ALPN,
    ping::PING_ALPN,
    rpc::RPC_ALPN,
};

/// ALPNs the node registers itself. Extra handlers cannot reuse them, whether or not the
/// optional handlers are set.
const RESERVED_ALPNS: [&[u8]; 4] = [PING_ALPN, GOSSIP_ALPN, RPC_ALPN, MEDIA_ALPN];

/// Rejects a reserved or repeated ALPN before the endpoint binds.
pub(crate) fn validate(extra: &[(Vec<u8>, Arc<dyn DynProtocolHandler>)]) -> Result<()> {
    let mut seen: Vec<&[u8]> = RESERVED_ALPNS.to_vec();
    for (alpn, _) in extra {
        if seen.contains(&alpn.as_slice()) {
            return Err(NetError::Config(format!(
                "ALPN {} is already registered",
                String::from_utf8_lossy(alpn)
            )));
        }
        seen.push(alpn);
    }
    Ok(())
}

/// Lets the router own a shared handler. Forwards every hook to the inner handler.
#[derive(Debug)]
pub(crate) struct Shared(pub(crate) Arc<dyn DynProtocolHandler>);

impl ProtocolHandler for Shared {
    async fn on_accepting(&self, accepting: Accepting) -> std::result::Result<Connection, AcceptError> {
        self.0.on_accepting(accepting).await
    }

    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.0.accept(connection).await
    }

    async fn shutdown(&self) {
        self.0.shutdown().await;
    }
}
