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
    /// The device asks to join the community's MLS group. Answered by the preferred committer.
    /// `from` is the joiner's own address, so the committer can fetch its key package.
    MlsJoin { community: CommunityId, from: PeerAddr },
    /// Asks a joining device for key packages from its pool. Each one is used once.
    KeyPackages { community: CommunityId },
    /// Commits applied after epoch `after`, in order, for a device that fell behind.
    MlsCommits { community: CommunityId, after: u64 },
    /// The commit that entered `epoch`, fetched after its announcement on gossip.
    MlsCommit { community: CommunityId, epoch: u64 },
    /// The current community secret, for a member that is behind on rotations and cannot open them.
    Secret { community: CommunityId },
    /// A direct message. `from` is the sender's address, for replies.
    Dm { dm: Box<SignedDm>, from: PeerAddr },
    /// A contact request or an accept, signed by the sender's device.
    Contact(Box<crate::contacts::SignedContact>),
    /// A conversation packet: an invitation, a commit or a sealed message.
    Conv(Box<crate::groups::ConvPacket>),
    /// Envelopes for this device, deposited by a sender while it was offline.
    Deposit(Box<crate::inbox::Envelope>),
    /// The device asks its inbox for envelopes.
    Collect(Box<crate::inbox::Collect>),
    /// The device has processed these envelopes, so the inbox may delete them.
    AckEnvelopes(Box<crate::inbox::AckIds>),
    /// Key packages from one of the owner's devices, for the inbox to hand out.
    DepositKeyPackages(Box<crate::inbox::DepositKeys>),
    /// One key package for `root`, removed as it is handed out.
    TakeKeyPackage { root: dissent_identity::RootId },
    /// One key package from this device, for a direct conversation. Not scoped to a community.
    DmKeyPackage,
    /// One voice call signalling step, for a DM or a voice channel. See `calls`.
    Call(Box<crate::calls::CallMsg>),
    /// The root's guardian list, sent to another device of the same root.
    Guardians(Box<dissent_identity::GuardianSet>),
    /// A presence beacon, sent directly to a contact's device. Community beacons go on the topic.
    Presence(Box<crate::presence::SignedPresence>),
    /// A typing signal, sent directly to a DM or group member's device. Channels use the topic.
    Typing(Box<crate::typing::SignedTyping>),
    /// Every community secret the responder holds, with its generation, for an active member that
    /// joined after a rotation. Appended, so the encoding of earlier variants is unchanged.
    SecretHistory { community: CommunityId },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Response {
    /// `addrs` are signed addresses of the community's members, for the requester to dial.
    Genesis {
        genesis: Genesis,
        addrs: Vec<SignedAddr>,
    },
    /// The responder's next message. `None` means it has nothing further to send. `addrs` as above.
    Reconcile {
        msg: Option<Vec<u8>>,
        addrs: Vec<SignedAddr>,
    },
    Events(Vec<SignedEvent>),
    /// The welcome that lets the joiner enter the MLS group.
    Welcome(Vec<u8>),
    KeyPackages(Vec<Vec<u8>>),
    /// The envelopes an inbox holds for the collecting device.
    Envelopes(Vec<crate::inbox::Envelope>),
    MlsCommits(Vec<Vec<u8>>),
    /// The commit for the requested epoch, if this device has it.
    MlsCommit(Option<Vec<u8>>),
    /// The newest community secret and its generation, for an active member.
    Secret(Option<(u64, [u8; 32], [u8; 32])>),
    Ack,
    /// Answer to a voice call step. See `calls`.
    Call(crate::calls::CallReply),
    /// Policy refused the request. The reason is for the sender to show.
    Refused(String),
    Error(String),
    /// The `(generation, secret)` pairs the responder holds. See `Request::SecretHistory`.
    SecretHistory(Vec<(u64, [u8; 32])>),
}

impl Request {
    /// The community a request concerns, if it concerns one. Such requests carry a token, and the
    /// responder refuses them unless the token proves knowledge of the community secret.
    pub(crate) fn community(&self) -> Option<CommunityId> {
        match self {
            Request::Genesis { community }
            | Request::Reconcile { community, .. }
            | Request::Fetch { community, .. }
            | Request::Push { community, .. }
            | Request::MlsJoin { community, .. }

            | Request::MlsCommits { community, .. }
            | Request::MlsCommit { community, .. }
            | Request::Secret { community }
            | Request::SecretHistory { community } => Some(*community),
            _ => None,
        }
    }
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
                Some(inner) => crate::engine::Engine { inner }.handle_request(from, &req).await,
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
