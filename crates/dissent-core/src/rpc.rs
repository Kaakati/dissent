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
    /// A profile picture, sent directly to a contact's device. Appended, see `avatars`.
    Profile(Box<crate::avatars::SignedProfile>),
    /// The device's digest of the address records it may share with the responder, as (device,
    /// seq). The responder replies with its own digest and the records the requester lacks. Appended,
    /// see `peer_exchange`.
    AddrSync { digest: Vec<(DeviceId, u64)> },
    /// Signed address records pushed by a scoped peer. Accepted at most once per `PUSH_GAP_MS`.
    /// Appended, see `peer_exchange`.
    AddrPush { records: Vec<crate::addr_record::AddrRecord> },
    /// The current address record of `device`, for a dial that has no address yet. Answered only to
    /// scoped peers. Appended, see `peer_exchange`.
    AddrLookup { device: DeviceId },
    /// The epoch keys the responder holds, for an active member's device. Under `SinceJoin` only
    /// the epochs from the requester's join. Appended, see `epoch_keys`.
    EpochHistory { community: CommunityId },
    /// The write keys the responder holds, for an active storage member. Appended, see `epoch_keys`.
    WriteKeys { community: CommunityId },
    /// Whether the device supports member-sealed control. Appended, see `epoch_keys`.
    MemberSealedSupport { community: CommunityId },
    /// The contacts this device shares with its contacts, for mutual friends. Answered only to an
    /// accepted, unblocked contact. Appended, see `friends`.
    Friends,
    /// The history keys this device holds, for a joiner. Answered by the rule for `EpochHistory`. Appended, see `history`.
    HistoryKeys { community: CommunityId },
    /// Whether this device can read and write history messages. Appended, see `history`.
    HistorySupport,
    /// Whether this device can be added to a direct conversation after it was made. Appended, see `dm_devices`.
    DmDevicesSupport,
    /// Blobs a message's thumbnails and previews point at, which a seeder should keep. Appended, see `mirror_hint`.
    MirrorHint {
        community: CommunityId,
        blobs: Vec<([u8; 32], u64)>,
    },
    /// History keys an active member sends a device it has just admitted, so the device need not wait for its own
    /// fetch. Answered `Ack`. Appended, see `history`.
    PushHistoryKeys {
        community: CommunityId,
        pairs: Vec<(u64, [u8; 32])>,
    },
    /// The invite preview of a community, for a holder of its invite secret. `token` is the token of
    /// that secret, which the responder checks itself, since the requester is not yet a member.
    /// `invite_id` names the invite used, so the answer can say whether it is live. Appended, see
    /// `invite_preview`. Not in `community()`, so the envelope carries no token for it.
    InvitePreview {
        community: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
    },
    /// `Fetch`, answered with each event as its own bytes, so a requester skips an event it cannot
    /// decode instead of failing the page. Appended, see `sync::decode_events`.
    FetchRaw {
        community: CommunityId,
        ids: Vec<EventId>,
    },
    /// `InvitePreview`, answered with the extra a rename and banner add. A responder from before
    /// renames cannot decode it and answers with an error, and the requester asks `InvitePreview`.
    /// Appended, see `invite_preview`. Not in `community()`, as `InvitePreview` is not.
    InvitePreviewFull {
        community: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
    },
    /// A profile banner, sent directly to a contact's device, as `Profile` is for the picture. A
    /// device from before banners cannot decode it and answers with an error. Appended, see
    /// `profile_banners`.
    ProfileBanner(Box<crate::profile_banners::SignedBanner>),
    /// The requester's signed profile, and a request for ours. Sent once a run to each contact, so a
    /// picture set before the contact existed still arrives. A device from before this cannot decode
    /// it and answers with an error. Appended, see `profile_sync`.
    ProfileSync(Box<crate::avatars::SignedProfile>),
    /// The keys of a private channel, for a reader's device: the reader is this device's root. Appended,
    /// see `private_keys`. Answered only to a reader in the responder's DAG.
    ChannelKeys {
        community: CommunityId,
        channel: dissent_events::ChannelId,
    },
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
    /// The responder's digest and the records the requester lacks. See `Request::AddrSync`. Appended.
    AddrSync {
        digest: Vec<(DeviceId, u64)>,
        records: Vec<crate::addr_record::AddrRecord>,
    },
    /// The record for `Request::AddrLookup`, if the responder holds one it may share. `seeder` is set
    /// when the responder runs as a seeder, so the requester can label the answer. Appended.
    AddrLookup {
        record: Option<crate::addr_record::AddrRecord>,
        seeder: bool,
    },
    /// `(epoch, enc, write)` for each epoch the responder holds. See `Request::EpochHistory`. Appended.
    EpochHistory(Vec<(u64, [u8; 32], [u8; 32])>),
    /// `(epoch, write)` for each epoch the responder holds. See `Request::WriteKeys`. Appended.
    WriteKeys(Vec<(u64, [u8; 32])>),
    /// Whether the responder supports member-sealed control. See `Request::MemberSealedSupport`. Appended.
    MemberSealedSupport(bool),
    /// The signed preview for `Request::InvitePreview`. Appended, see `invite_preview`.
    InvitePreview(Box<crate::invite_preview::SignedPreview>),
    /// The events of a `FetchRaw`, each as the bytes of its `SignedEvent`. Appended.
    EventsRaw(Vec<Vec<u8>>),
    /// The root ids of our contacts, or `None` when we share nothing with the requester. Appended, see
    /// `friends`.
    Friends(Option<Vec<dissent_identity::RootId>>),
    /// The signed preview and extra for `Request::InvitePreviewFull`. Appended, see `invite_preview`.
    InvitePreviewFull(Box<crate::invite_preview::SignedPreviewFull>),
    /// The history keys the responder holds, as `(epoch, key)`. Appended, see `history`.
    HistoryKeys(Vec<(u64, [u8; 32])>),
    /// Whether the responder supports history messages. Appended, see `history`.
    HistorySupport(bool),
    /// Whether the responder can be added to a direct conversation later. Appended, see `dm_devices`.
    DmDevicesSupport(bool),
    /// The answer to `Request::ProfileSync`: the responder's signed profile and banner. Appended.
    ProfileSync {
        profile: Box<crate::avatars::SignedProfile>,
        banner: Option<Box<crate::profile_banners::SignedBanner>>,
    },
    /// The `(generation, key)` pairs of a private channel, for a reader. See `Request::ChannelKeys`. Appended.
    ChannelKeys(Vec<(u64, [u8; 32])>),
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
            | Request::SecretHistory { community }
            | Request::EpochHistory { community }
            | Request::WriteKeys { community }
            | Request::MemberSealedSupport { community }
            | Request::HistoryKeys { community }
            | Request::MirrorHint { community, .. }
            | Request::PushHistoryKeys { community, .. }
            | Request::FetchRaw { community, .. }
            | Request::ChannelKeys { community, .. } => Some(*community),
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
