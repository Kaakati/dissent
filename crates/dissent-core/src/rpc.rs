//! Request and reply types for the direct RPC channel, and the handler that serves them.

use crate::engine::Inner;
use crate::error::{CoreError, Result};
use crate::wire::SignedAddr;
use dissent_events::{EventId, SignedDm, SignedEvent};
use dissent_identity::{CommunityId, DeviceId, Genesis};
use dissent_net::{PeerAddr, RpcHandler};
use futures_lite::future::Boxed;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::{Arc, Weak};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Request {
    /// The community's genesis. A joiner checks it against the invite.
    Genesis { community: CommunityId },
    /// One round of set reconciliation. `msg` is the initiator's negentropy message, and
    /// `from` is the requester's own address, so the responder can dial back.
    Reconcile {
        community: CommunityId,
        msg: Vec<u8>,
        from: PeerAddr,
    },
    /// Stored events with these ids, in causal order. Unknown ids are skipped.
    Fetch {
        community: CommunityId,
        ids: Vec<EventId>,
    },
    /// Events for the responder to verify and store, as if they had arrived by gossip.
    Push {
        community: CommunityId,
        events: Vec<SignedEvent>,
    },
    /// A device's signed address, sent when it starts or changes address.
    Addr(Box<SignedAddr>),
    /// A direct message. `from` is the sender's address, for replies.
    Dm { dm: Box<SignedDm>, from: PeerAddr },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Response {
    Genesis(Genesis),
    /// The responder's next message. `None` means it has nothing further to send.
    Reconcile { msg: Option<Vec<u8>> },
    Events(Vec<SignedEvent>),
    Ack,
    Error(String),
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(value).map_err(|e| CoreError::Encoding(e.to_string()))
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    postcard::from_bytes(bytes).map_err(|e| CoreError::Encoding(e.to_string()))
}

/// Serves incoming requests. Holds a weak reference, because the node that owns this handler
/// is itself stored in `Inner`.
pub(crate) struct Handler(pub Weak<Inner>);

impl RpcHandler for Handler {
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>> {
        let weak = self.0.clone();
        Box::pin(async move {
            let resp = match weak.upgrade() {
                Some(inner) => crate::engine::Engine { inner }.handle_request(from, &req),
                None => Response::Error("node is shutting down".into()),
            };
            encode(&resp).unwrap_or_default()
        })
    }
}

/// Keeps the handler's `Arc` usable by the engine module without exposing `Inner`.
pub(crate) fn handler_for(inner: &Arc<Inner>) -> Arc<dyn RpcHandler> {
    Arc::new(Handler(Arc::downgrade(inner)))
}
