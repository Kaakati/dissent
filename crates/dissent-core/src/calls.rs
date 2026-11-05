//! Voice calls: a direct DM call, or a voice channel call.
//!
//! A call is a set of pairwise media links. Each pair of participating devices has one link.
//! The device with the lower sender id makes the offer and the higher one accepts. Signalling
//! travels over the authenticated device RPC, so a call needs no gossip. Media is SFrame
//! encrypted and each sender signs its frame digests (see `dissent-media`).
//!
//! The channel key comes from the community's MLS group, under the RFC 9605 label. A DM call
//! has no group yet, so the caller picks a random key and sends it over the authenticated RPC.
//! The call rekeys when the group epoch changes.

use base64::Engine as _;

use crate::call_video::{CaptureKind, Encoded, Preview, RemoteVideo, Sending};
use dissent_media::VideoMeta;
use crate::engine::{now_ms, Engine, Inner};
use crate::groups::{dm_conv_id, ConvId};
use crate::error::{CoreError, Result};
use crate::rpc::{decode, Request, Response};
use crate::timings::{millis, Timings};
use crate::views::{EngineEvent, VoiceChannelView, VoiceParticipantView};
use dissent_events::state::ChannelSettings;
use dissent_events::{ChannelId, Payload};
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId};
use dissent_audio::{OpusEncoder, SenderConfig, SenderPacket, VoiceReceiver, VoiceSender};
use dissent_media::{
    assign_forwarders, elect, plan as cascade_plan, Auth, Candidate, LinkTransport, MediaKey, Mesh, Mode, Participant, ReceivedFrame, Sfu,
    PacketTransport, AUDIO_STREAM_KBPS, MAX_SENDER_ID, SYNTHETIC_CALLEE, SYNTHETIC_CALLER, VIDEO_720P_KBPS,
    Side, Signal, FRAME_SAMPLES,
};
use std::collections::VecDeque;
use dissent_net::{MediaHandler, MediaLink, PathKind, PeerAddr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Mutex as AsyncMutex};
use tracing::{debug, warn};

pub(crate) mod speaking;
pub(crate) mod volunteers;

/// Identifies a call. Both sides compute it the same way.
pub(crate) type RoomId = [u8; 32];

/// The RFC 9605 export label for the SFrame base key.
const KEY_LABEL: &str = "SFrame 1.0 Base Key";
/// One synthetic audio frame every 20 ms, 160 bytes.
const FRAME_MS: u64 = 20;
/// A call loop tick that fires this much after it was due counts as late: two frames of audio.
const LATE_TICK: Duration = Duration::from_millis(40);
const FRAME_BYTES: usize = 160;
/// How long the accepting side waits for the media link that the offering side dials.
const LINK_WAIT: Duration = Duration::from_secs(10);
/// How long the current room's call loop may go without a tick before `watch_driver` warns.
const DRIVER_STALL: Duration = Duration::from_secs(5);
/// How long one signalling request may take.
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
/// The driver rechecks rekeying and bans this many frames apart, about every half second.
const MEMBERSHIP_EVERY: u32 = 25;
const ROOM_DOMAIN: &[u8] = b"dissent/call-room/v1";

/// Where a call lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Scope {
    /// A direct call between two contacts.
    Dm,
    /// A voice channel in a community.
    Channel {
        community: CommunityId,
        channel: ChannelId,
    },
    /// A call among the members of a group conversation.
    Group { conv: crate::groups::ConvId },
}

impl Scope {
    /// The MLS group a channel or group call is keyed by. A direct call has none.
    pub(crate) fn group_id(&self) -> Option<[u8; 32]> {
        match self {
            Scope::Dm => None,
            Scope::Channel { community, .. } => Some(community.0),
            Scope::Group { conv } => Some(conv.0),
        }
    }
}

/// The body of a join. `key` is set for DM rooms only. `from` is the sender's address, so the
/// other side can dial back.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JoinMsg {
    pub room: RoomId,
    pub scope: Scope,
    pub cert: DeviceCert,
    pub key: Option<[u8; 32]>,
    pub muted: bool,
    /// A voice-channel joiner's own MLS leaf index, which is its sender id. `None` for a direct call.
    pub leaf: Option<u64>,
    /// Asks a device that is not in the call to ring for it. Only a group call sends this.
    pub ring: bool,
    pub from: PeerAddr,
}

/// Signalling messages, sent over the device RPC.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum CallMsg {
    /// The sender joins the room. Answered with `CallReply::Here` by a device in the room.
    Join(Box<JoinMsg>),
    /// The lower-id side's offer for one pair of devices. Answered with `CallReply::Answer`.
    Offer {
        room: RoomId,
        /// The generation of the room the offer is for.
        gen: RoomId,
        signal: Vec<u8>,
    },
    /// The sender's mute state.
    Mute { room: RoomId, muted: bool },
    /// The caller gave up before the call was answered, or the callee declined it.
    Decline { room: RoomId },
    /// Another device changed the call's tier. Verified, then followed if the roster matches.
    Tier(Box<TierMsg>),
    /// A device's measured upload capacity, signed by its device key.
    Capacity {
        room: RoomId,
        kbps: u32,
        sig: dissent_identity::Signature,
    },
    /// The call was answered on another device of the same person. Stop ringing here.
    AnsweredElsewhere { room: RoomId },
    /// The sender has left the room and drops its link to us.
    Leave { room: RoomId },
    /// The sender's account of the DM or group call it is in. Sent to the people who could join
    /// it, so they can show it as live. An empty roster means the sender has left.
    Presence(Box<PresenceMsg>),
    /// The call features the sender supports, as `FEATURE_*` bits. Appended after the released
    /// variants: a released device cannot decode it and answers it as a bad request, which is how
    /// the sender learns that device is released. See `docs/design/13-forwarded-calls.md`.
    Features { room: RoomId, bits: u64 },
    /// The simulcast layers the sender can send of its camera or screen, smallest first, as
    /// (width in pixels, kbps). Sent to its forwarder. See `call_layers`.
    VideoLayers { room: RoomId, screen: bool, layers: Vec<(u32, u32)> },
    /// How wide the sender draws each other sender's video, as (sender id, screen, width in
    /// pixels). Sent to its forwarder, which then counts it as a viewer that knows layers.
    TileWidths { room: RoomId, tiles: Vec<(u64, bool, u32)> },
    /// The layers a forwarder wants of the receiver's camera or screen, bit 0 for layer 0. Sent
    /// by the forwarder to the sender, which then encodes only those.
    WantedLayers { room: RoomId, screen: bool, mask: u8 },
    /// A member who is not in the call offers to forward it, or withdraws the offer. Sent by a
    /// volunteer to the call's devices. See `volunteers`.
    ForwardOffer(Box<volunteers::ForwardOffer>),
    /// The sender, a leaf of a volunteer forwarder, has heard no audio through it for
    /// `volunteers::STALL_WINDOW`. `spoke` says whether it sent audio itself in that time. Sent to
    /// every live device, which together exclude a stalled volunteer. See `volunteers`.
    ForwarderStalled { room: RoomId, forwarder: u64, spoke: bool },
    /// A leaf tells its volunteer forwarder its sender id in generation `gen`, before it offers its
    /// link. A seeder is outside the MLS group, so it cannot read the leaves' ids itself. See
    /// `volunteers`.
    RelayLeaf { room: RoomId, gen: RoomId, sid: u64 },
    /// The streams a forwarder is not sending the receiver now, because the receiver's budget cannot
    /// fit them even on their lowest layer, as (sender id, screen). Sent by the forwarder to each
    /// viewer when the set changes, so the viewer shows those streams as paused, not frozen. See
    /// `call_layers`.
    PausedVideo { room: RoomId, streams: Vec<(u64, bool)> },
    /// The sender has too few working links in its room of this call and asks which room the receiver is in, to
    /// rejoin it. Answered with `CallReply::There`. Appended after the released variants: a released device answers it
    /// as a bad request, which counts as no answer. See `rejoin`.
    WhereIs { room: RoomId },
}

/// `CallMsg::Features` bit: the sender keeps a call as a mesh up to its server's boosted limit.
pub(crate) const FEATURE_BOOST_TIERS: u64 = 1;
/// `CallMsg::Features` bit: the sender takes `VideoLayers`, `TileWidths` and `WantedLayers`.
pub(crate) const FEATURE_VIDEO_LAYERS: u64 = 2;
/// `CallMsg::Features` bit: the sender takes `ForwardOffer`, and can be a leaf of a forwarder that
/// is not in the call. A call uses volunteers only while every device in it sent this bit.
pub(crate) const FEATURE_VOLUNTEERS: u64 = 4;
/// `CallMsg::Features` bit: the sender takes a seeder as a volunteer forwarder, and tells it its
/// sender id with `RelayLeaf`. A call uses a seeder only while every device in it sent this bit.
pub(crate) const FEATURE_SEEDER_FORWARDERS: u64 = 8;
/// `CallMsg::Features` bit: the sender takes `PausedVideo`.
pub(crate) const FEATURE_PAUSED_VIDEO: u64 = 16;
/// `CallMsg::Features` bit: the sender elects no member outside the call, only a seeder. It no longer sends
/// `FEATURE_VOLUNTEERS`, so a client from before this elects nobody outside a call that has one of these in it. See
/// `volunteers`.
pub(crate) const FEATURE_SEEDERS_ONLY: u64 = 32;
/// Every call feature this device supports.
const FEATURES: u64 =
    FEATURE_BOOST_TIERS | FEATURE_VIDEO_LAYERS | FEATURE_SEEDERS_ONLY | FEATURE_SEEDER_FORWARDERS | FEATURE_PAUSED_VIDEO;

/// A device's roster for the DM or group call it is in.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PresenceMsg {
    pub room: RoomId,
    pub scope: Scope,
    /// When the call started on the sender's device, in milliseconds.
    pub started_ms: u64,
    /// Everyone in the call on the sender's device, the sender included.
    pub members: Vec<PresenceMember>,
}

/// One person in a call, as a device sees them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresenceMember {
    pub root: RootId,
    pub muted: bool,
    /// Whether this person has deafened. Only the device of that person says it for certain.
    pub deafened: bool,
}

/// A call another device said it is in. Kept per device, since a device is in one call at a time.
#[derive(Clone, Debug)]
pub(crate) struct Heard {
    pub room: RoomId,
    pub scope: Scope,
    pub started_ms: u64,
    /// The person whose device sent this.
    pub from: RootId,
    pub members: Vec<PresenceMember>,
    /// When this device last heard the roster, in milliseconds.
    pub seen_ms: u64,
}

impl Heard {
    /// The sender's own entry: the one part of a roster its device vouches for first-hand.
    pub(crate) fn own_entry(&self) -> Option<&PresenceMember> {
        self.members.iter().find(|m| m.root == self.from)
    }
}

/// The most people a presence message may list. Groups are capped well below this.
const MAX_PRESENCE_MEMBERS: usize = 64;
/// Answers to a [`CallMsg`].
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum CallReply {
    /// The device is in the room. `cert` lets the other side authenticate it.
    Here { cert: DeviceCert, muted: bool },
    /// The call is ringing on the device. The user has not answered yet.
    Ringing,
    /// The device will not join, or is not in the room.
    Refused(String),
    /// The accepting side's answer to an offer.
    Answer(Vec<u8>),
    Ack,
    /// The room the replier is in for the asked call: its tier, generation and forwarder, and whether it has enough
    /// working links there to be worth joining. Only a device that asked `WhereIs` reads it.
    There { tier: Tier, gen: RoomId, forwarder: Option<u64>, settled: bool },
}

/// A call's audio, both directions. The synthetic one sends constant frames and plays nothing,
/// for tests. The voice one runs the Opus pipeline from `dissent-audio`. The desktop app owns
/// the device threads, so it feeds captured audio in and pulls playout out.
pub(crate) trait CallAudio: Send {
    /// Frames ready to send, as (RTP timestamp, bytes). Appends to `out`.
    fn outgoing(&mut self, out: &mut Vec<(u32, Vec<u8>)>);
    /// A frame that arrived from a peer.
    fn incoming(&mut self, frame: &ReceivedFrame, now_ms: u64);
    /// Fills `out` with the next 20 ms of mixed playout.
    fn playout(&mut self, now_ms: u64, out: &mut [f32]);
    /// Adds captured samples. A synthetic source ignores them.
    fn capture(&mut self, samples: &[f32]) -> Result<()>;
    /// Playout slots lost, concealed by PLC and silenced, summed over the senders. A synthetic source has none.
    fn loss(&self) -> (u64, u64, u64) {
        (0, 0, 0)
    }
}

/// Constant frames, one per tick. Enough to exercise the media path end to end. The first byte
/// is the audio level, 0 for full scale, so a forwarder treats these frames as speech.
#[derive(Default)]
struct SyntheticAudio {
    seq: u64,
}

impl CallAudio for SyntheticAudio {
    fn outgoing(&mut self, out: &mut Vec<(u32, Vec<u8>)>) {
        self.seq += 1;
        let timestamp = (self.seq * FRAME_SAMPLES as u64) as u32;
        out.push((timestamp, vec![0u8; FRAME_BYTES]));
    }

    fn incoming(&mut self, _frame: &ReceivedFrame, _now_ms: u64) {}

    fn playout(&mut self, _now_ms: u64, out: &mut [f32]) {
        out.fill(0.0);
    }

    fn capture(&mut self, _samples: &[f32]) -> Result<()> {
        Ok(())
    }
}

/// The Opus pipeline: encodes captured audio, and mixes what the peers send.
pub(crate) struct VoiceAudio {
    sender: VoiceSender<OpusEncoder>,
    receiver: VoiceReceiver,
    pending: VecDeque<SenderPacket>,
}

/// How many speakers are mixed into one playout frame.
const MAX_MIXED: usize = 8;

impl VoiceAudio {
    pub(crate) fn new(config: SenderConfig) -> Result<Self> {
        let sender = VoiceSender::opus(config).map_err(call_err)?;
        Ok(Self {
            sender,
            receiver: VoiceReceiver::opus(MAX_MIXED),
            pending: VecDeque::new(),
        })
    }
}

impl CallAudio for VoiceAudio {
    fn outgoing(&mut self, out: &mut Vec<(u32, Vec<u8>)>) {
        for packet in self.pending.drain(..) {
            out.push((packet.timestamp, packet.to_media_frame()));
        }
    }

    fn incoming(&mut self, frame: &ReceivedFrame, now_ms: u64) {
        // The payload is the sender's `to_media_frame` bytes: the audio-level byte, then Opus.
        if let Err(e) = self.receiver.push_media_frame(
            frame.sender,
            frame.seq,
            frame.timestamp,
            now_ms,
            &frame.payload,
        ) {
            debug!(error = %e, "audio frame not played");
        }
    }

    fn playout(&mut self, now_ms: u64, out: &mut [f32]) {
        if self.receiver.pop_mix(now_ms, out).is_err() {
            out.fill(0.0);
        }
    }

    fn loss(&self) -> (u64, u64, u64) {
        self.receiver
            .stats()
            .values()
            .fold((0, 0, 0), |(lost, concealed, silenced), j| (lost + j.lost, concealed + j.concealed, silenced + j.silenced))
    }

    fn capture(&mut self, samples: &[f32]) -> Result<()> {
        let packets = self.sender.push_capture(samples).map_err(call_err)?;
        self.pending
            .extend(packets.into_iter().filter(|p| p.voiced));
        Ok(())
    }
}


/// The forwarded tier. One elected device relays audio between the others. Every other device
/// links to it, and it links to none of them directly.
impl Forwarding {
    pub(crate) fn role(&self) -> Role {
        self.role
    }

    pub(crate) fn forwarder_sid(&self) -> u64 {
        self.forwarder
    }

    pub(crate) fn sfu(&self) -> &AsyncMutex<Option<Sfu<CallLink>>> {
        &self.sfu
    }

    pub(crate) fn leaf(&self) -> &AsyncMutex<Option<Participant<CallLink>>> {
        &self.leaf
    }

    /// Whether this is a volunteer's relay for a call it is not in. It has no leaf side.
    pub(crate) fn is_volunteer_relay(&self) -> bool {
        self.serving
    }
}

pub(crate) struct Forwarding {
    role: Role,
    /// The sender id this device links to for media. A leaf's is its forwarder. A child's is the
    /// root, which it links to upstream. A root's is its own id.
    forwarder: u64,
    /// On a cascade root: its child forwarders. Empty for a single forwarder.
    children: Vec<u64>,
    /// On a cascade child: the standby root it is linked to, which takes over if the root is lost.
    standby: Option<u64>,
    /// On a cascade child: how many leaves it serves.
    served: usize,
    /// On a forwarder: relays between the leaves and any upstream root.
    sfu: AsyncMutex<Option<Sfu<CallLink>>>,
    /// Every device: its leaf link, carrying its own speech out and the relay's out to it.
    leaf: AsyncMutex<Option<Participant<CallLink>>>,
    /// On a leaf: the device and person of its forwarder when that is a volunteer, a member who is
    /// not in the call. It is never listed as a participant. See `volunteers`.
    volunteer: Option<(DeviceId, RootId)>,
    /// On a volunteer: this is its relay for a call it is not in. It has no leaf and no key.
    serving: bool,
}

/// A device's place in a forwarded or cascaded call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The top forwarder. In a single forwarder call, the one forwarder.
    Root,
    /// A cascade child, serving its share of the leaves and linked up to the root.
    Child,
    /// The cascade standby. Children are linked to it already, and it takes over relaying if the
    /// root is lost.
    Standby,
    /// An ordinary participant, linked to its forwarder only.
    Leaf,
}

/// Builds one device's forwarded-tier state. Every device has a leaf link to its forwarder. A
/// forwarder also has a relay, which carries its own speech in process.
#[allow(clippy::too_many_arguments)]
fn build_forwarding(
    role: Role,
    forwarder: u64,
    children: Vec<u64>,
    standby: Option<u64>,
    served: usize,
    my_sid: u64,
    participants: usize,
    key: MediaKey,
    auth: Auth,
) -> Result<Forwarding> {
    let participant = Participant::new_authenticated(
        key,
        my_sid,
        vec![candidate(forwarder, DEFAULT_KBPS)],
        participants,
        Mode::AudioTop3,
        auth,
    )
    .map_err(call_err)?;
    let sfu = (role != Role::Leaf).then(|| Sfu::new(Mode::AudioTop3));
    Ok(Forwarding {
        role,
        forwarder,
        children,
        standby,
        served,
        sfu: AsyncMutex::new(sfu),
        leaf: AsyncMutex::new(Some(participant)),
        volunteer: None,
        serving: false,
    })
}

/// Child forwarders in a cascade of `participants` devices. About the square root, at least two.
fn cascade_child_count(participants: usize) -> usize {
    ((participants as f64).sqrt().ceil() as usize).max(2)
}

/// One end of an in-process link. The forwarder's own speech goes through one of these into its
/// own relay, so its audio reaches the leaves like any other participant's.
pub(crate) struct MemoryTransport {
    local: SocketAddr,
    peer: SocketAddr,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    rx: AsyncMutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl PacketTransport for MemoryTransport {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    async fn send(&self, _to: SocketAddr, bytes: &[u8]) -> dissent_media::Result<()> {
        self.tx
            .send(bytes.to_vec())
            .map_err(|_| dissent_media::MediaError::Closed)
    }

    async fn recv(&self) -> dissent_media::Result<(SocketAddr, Vec<u8>)> {
        let bytes = self
            .rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(dissent_media::MediaError::Closed)?;
        Ok((self.peer, bytes))
    }
}

/// A link of the forwarded tier: over the network, or in process for the forwarder's own speech.
pub(crate) enum CallLink {
    Net(LinkTransport),
    Memory(MemoryTransport),
}

impl PacketTransport for CallLink {
    fn local_addr(&self) -> SocketAddr {
        match self {
            CallLink::Net(t) => t.local_addr(),
            CallLink::Memory(t) => t.local_addr(),
        }
    }

    fn max_datagram_size(&self) -> Option<usize> {
        match self {
            CallLink::Net(t) => t.max_datagram_size(),
            CallLink::Memory(t) => t.max_datagram_size(),
        }
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> dissent_media::Result<()> {
        match self {
            CallLink::Net(t) => t.send(to, bytes).await,
            CallLink::Memory(t) => t.send(to, bytes).await,
        }
    }

    async fn recv(&self) -> dissent_media::Result<(SocketAddr, Vec<u8>)> {
        match self {
            CallLink::Net(t) => t.recv().await,
            CallLink::Memory(t) => t.recv().await,
        }
    }
}

/// Two connected ends: the leaf's and the relay's.
pub(crate) fn memory_pair() -> (MemoryTransport, MemoryTransport) {
    let (a_tx, b_rx) = tokio::sync::mpsc::unbounded_channel();
    let (b_tx, a_rx) = tokio::sync::mpsc::unbounded_channel();
    let a = MemoryTransport {
        local: SYNTHETIC_CALLER,
        peer: SYNTHETIC_CALLEE,
        tx: a_tx,
        rx: AsyncMutex::new(a_rx),
    };
    let b = MemoryTransport {
        local: SYNTHETIC_CALLEE,
        peer: SYNTHETIC_CALLER,
        tx: b_tx,
        rx: AsyncMutex::new(b_rx),
    };
    (a, b)
}

/// Upload assumed for a device whose capacity is not known yet.
const DEFAULT_KBPS: u32 = 10_000;

/// A candidate forwarder with a measured capacity, in kbps. Devices without one count as the
/// default.
fn candidate(peer: u64, kbps: u32) -> Candidate {
    Candidate {
        peer,
        upload_kbps: kbps,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 50,
    }
}

/// The forwarder for a call of `participants` devices, chosen by dissent-media's election over
/// the announced capacities. Devices with no announced capacity count as the default.
fn elect_forwarder(sids: &[u64], participants: usize, capacities: &BTreeMap<u64, u32>) -> Option<u64> {
    let candidates: Vec<Candidate> = sids
        .iter()
        .map(|s| candidate(*s, capacities.get(s).copied().unwrap_or(DEFAULT_KBPS)))
        .collect();
    elect(&candidates, participants, Mode::AudioTop3).map(|e| e.primary)
}

/// One remote participant of a call.
struct Peer {
    device: DeviceId,
    root: RootId,
    muted: bool,
    last_frame: Option<Instant>,
    last_arrival: Option<Instant>,
    /// Smoothed difference between frame gaps and the 20 ms frame interval.
    jitter_ms: f64,
}

/// Measurements the driver stores for the view, which cannot await the mesh.
#[derive(Default)]
struct Quality {
    rtt_ms: Option<f64>,
    loss_pct: f64,
    frames_received: u64,
    flagged: u64,
    deaf: DeafWatch,
}

/// How long frames arrive and none opens before a call says so.
const DEAF_AFTER: Duration = Duration::from_secs(5);

/// Notices a call whose frames arrive and do not decrypt, which looks like a normal call to its user. It watches the
/// whole link, so a call that opens some senders' frames does not count.
#[derive(Default)]
struct DeafWatch {
    undecryptable: u64,
    received: u64,
    since: Option<Instant>,
    warned: bool,
}

impl DeafWatch {
    /// Takes the links' totals. Returns true the first time the call has been deaf for `DEAF_AFTER`.
    fn note(&mut self, undecryptable: u64, received: u64, now: Instant) -> bool {
        if received > self.received {
            self.since = None;
            self.warned = false;
        } else if undecryptable > self.undecryptable {
            self.since.get_or_insert(now);
        }
        self.undecryptable = undecryptable;
        self.received = received;
        if self.is_deaf(now) && !self.warned {
            self.warned = true;
            return true;
        }
        false
    }

    fn is_deaf(&self, now: Instant) -> bool {
        self.since.is_some_and(|since| now.duration_since(since) >= DEAF_AFTER)
    }
}

/// A call in progress on this device.
pub(crate) struct Room {
    id: RoomId,
    scope: Scope,
    my_sid: u64,
    /// When the call started on this device, in milliseconds.
    started_ms: u64,
    /// The other person of a direct call.
    peer: Option<RootId>,
    deafened: AtomicBool,
    /// Devices this device has invited, so leaving can cancel their ringing.
    targets: Mutex<Vec<DeviceId>>,
    /// Channel rooms: the MLS leaf index of each member device, which is its sender id.
    leaves: Mutex<BTreeMap<DeviceId, u64>>,
    /// Set for a forwarded call. Media then goes through one elected forwarder, not a mesh.
    forwarding: Option<Forwarding>,
    muted: AtomicBool,
    /// Channel rooms: the MLS epoch of the key the mesh uses. DM rooms stay at 0.
    epoch: AtomicU64,
    mesh: AsyncMutex<Mesh<LinkTransport>>,
    peers: Mutex<BTreeMap<u64, Peer>>,
    quality: Mutex<Quality>,
    audio: Mutex<Box<dyn CallAudio>>,
    /// Which generation of the call this room is. A migrated room has its own, so an offer can
    /// name the room it belongs to. The first room of a call uses its base id.
    gen: RoomId,
    /// The tier this room runs at.
    tier: Tier,
    /// Channel rooms: the devices in the call now, by sender id. Drives tier decisions.
    live: Mutex<BTreeMap<u64, DeviceId>>,
    /// When this room first wanted a different tier. Cleared when it no longer does.
    tier_since: Mutex<Option<Instant>>,
    /// True while a migration to another tier is building.
    migrating: AtomicBool,
    /// Capacities the other devices announced, by sender id, with when each arrived.
    capacities: Mutex<BTreeMap<u64, (u32, Instant)>>,
    /// This device's own measured capacity in kbps. Zero until measured. A migrated room starts from the old one's.
    own_kbps: AtomicU32,
    /// When this room was swapped in, `None` for a room that never was. See `Timings::settle_after_swap`.
    swapped_at: Mutex<Option<Instant>>,
    /// Ticks since the last capacity measurement and announcement.
    capacity_clock: AtomicU64,
    /// When a better forwarder first appeared. Cleared when it no longer does.
    forwarder_since: Mutex<Option<Instant>>,
    /// The `Features` bits other devices sent, by sender id, with when each arrived.
    features: Mutex<BTreeMap<u64, (u64, Instant)>>,
    /// When each live sender id was first seen live, for `Timings::unheard_grace`.
    seen_live: Mutex<BTreeMap<u64, Instant>>,
    /// Since when this room has had too few working links with peers still live, and when it last asked them where
    /// they are. See `rejoin`.
    lonely: Mutex<(Option<Instant>, Option<Instant>)>,
    /// The engine's count of rooms not yet dropped, for a test that looks for one kept alive. See `Drop for Room`.
    alive: Arc<AtomicUsize>,
    /// Devices this room has sent its own `Features` to, by sender id.
    told: Mutex<std::collections::BTreeSet<u64>>,
    /// Offers to forward this call from members who are not in it, by sender id. See `volunteers`.
    offers: Mutex<BTreeMap<u64, volunteers::VolunteerOffer>>,
    /// On a leaf of a volunteer: what the stall check reads and has heard. See `volunteers`.
    stall: Mutex<volunteers::StallWatch>,
    /// Simulcast signalling state of this room. See `call_layers`.
    layer_signals: Mutex<crate::call_layers::LayerSignals>,
    /// On a forwarder: the (sender id, screen) streams whose senders announced layers.
    layered: Mutex<std::collections::BTreeSet<(u64, bool)>>,
    /// False on a staged room, which receives but does not send until it is swapped in.
    sending: AtomicBool,
    /// Media links this room has established. Counts links, not presence entries.
    links: AtomicUsize,
    /// The media link to each device this room has one with, read live for the path a participant
    /// shows. A path can change after the link opens, as a relayed start goes direct.
    media_links: Mutex<BTreeMap<DeviceId, MediaLink>>,
    /// True once this room has a presence clock, so that it has only one.
    clocked: AtomicBool,
    /// This device's camera while it is on. Dropping it stops the capture thread.
    camera: Mutex<Option<Sending>>,
    /// This device's shared screen while it is on.
    screen: Mutex<Option<Sending>>,
    /// The decoders of the peers' cameras and screens.
    remote_video: Mutex<RemoteVideo>,
    /// How the call loop keeps time: ticks run, ticks that fired over `LATE_TICK` late, the worst lateness in
    /// microseconds, and audio frames received. For tests and diagnostics.
    loop_stats: [AtomicU64; 4],
    /// Whether a call loop (`drive`) runs this room. Set when the loop is spawned, cleared when it ends.
    driven: AtomicBool,
    /// The link's last bandwidth estimate, in kbps. Zero before the first. For tests and diagnostics.
    link_estimate: AtomicU32,
    /// For tests: an estimate that replaces the link's when above zero.
    forced_estimate: AtomicU32,
    /// For tests: set, the encoders' time per frame is not read, so a busy machine does not step video down.
    #[cfg(feature = "test-hooks")]
    cpu_held: AtomicBool,
    /// The sound of everyone's shared screens, for the playout.
    screen_sound: Mutex<crate::call_screen_audio::ScreenAudioMix>,
    /// Set by a viewer that needs a keyframe. The pump asks the peers, then clears it.
    keyframe_wanted: AtomicBool,
    /// Who speaks: this device's last sent frame, and the speakers last shown. See `speaking`.
    speaking: Mutex<speaking::Speaking>,
    /// Audio sends that went out and those that did not, for a test that timed out.
    sends: Mutex<SendCounts>,
    /// The step the call loop is at, when it got there, and which code last took the mesh lock: atomics only, so a
    /// watchdog can read them while the loop is stuck. See `Engine::call_breadcrumbs`.
    step: AtomicU32,
    step_at: AtomicU64,
    mesh_by: AtomicU32,
}

/// What the call loop did with this device's audio, for a test that timed out.
#[derive(Clone, Default)]
struct SendCounts {
    /// Frames the mesh took.
    sent: u64,
    /// Frames it refused, by error.
    failed: BTreeMap<String, u64>,
    /// Ticks with no audio to send, because the source gave nothing.
    empty: u64,
    /// Ticks that took no audio because the room was not sending.
    not_sending: u64,
}

/// Which code last took a room's mesh lock, for `Room::mesh_by`.
mod mesh_site {
    pub const DRIVE: u32 = 1;
    pub const OFFER: u32 = 3;
    pub const ACCEPT: u32 = 4;
    pub const LEAVE: u32 = 5;
    pub const LEARN_MEMBER: u32 = 6;
    pub const NAMES: [&str; 7] = ["none", "drive", "?", "offer", "accept", "leave", "learn_member"];
}

/// Call-loop steps, for `Room::step`.
mod step {
    pub const TICK: u32 = 1;
    pub const MESH_LOCK: u32 = 2;
    pub const MEMBERSHIP: u32 = 3;
    pub const FORWARDED: u32 = 4;
    pub const RECEIVE: u32 = 5;
    pub const SEND: u32 = 6;
    pub const VIDEO: u32 = 7;
    pub const SPEAKING: u32 = 8;
    pub const NAMES: [&str; 9] = ["none", "tick", "mesh_lock", "membership", "forwarded", "receive", "send", "video", "speaking"];
}

/// A received camera or screen frame, still encoded, for a window that decodes it itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedVideo {
    /// The sender's root id, as the window keys its tiles.
    pub root_id: String,
    /// Whether the frame is a shared screen rather than a camera.
    pub screen: bool,
    /// Whether the frame is AV1, else H.264.
    pub av1: bool,
    /// Whether the frame is a keyframe, which a decoder can start from.
    pub keyframe: bool,
    /// The sender's frame counter for this stream.
    pub frame_id: u32,
    /// The codec bitstream of the frame, decrypted.
    pub payload: Vec<u8>,
}

/// Where encoded frames go. It returns true when it took the frame, and false to have the frame previewed as a JPEG
/// instead. See `Engine::set_encoded_video_sink`.
pub type EncodedSink = Arc<dyn Fn(EncodedVideo) -> bool + Send + Sync>;

/// An incoming direct call the user has not answered yet. No media exists until they accept.
pub(crate) struct Ringing {
    pub(crate) id: RoomId,
    from: DeviceId,
    /// The person calling.
    peer: RootId,
    cert: DeviceCert,
    /// The call's scope: a direct call, or a group call the caller started.
    scope: Scope,
    /// The media key a direct call carries. A group call's key comes from its MLS group.
    key: Option<[u8; 32]>,
}

/// Calls on this device, the media links the transport has delivered, and any call ringing.
pub(crate) struct CallHub {
    /// The pixel width each remote tile is drawn at, by the peer's root id and whether it is a
    /// screen. A preview is made that wide, so a small tile costs little.
    pub(crate) tiles: Mutex<HashMap<(String, bool), u32>>,
    /// Where received frames go when a window decodes them itself. `None` for the JPEG previews.
    encoded_sink: Mutex<Option<EncodedSink>>,
    /// Where this engine's camera, lobby preview and screen take their pictures from. A device, unless a test set it.
    test_capture: Mutex<crate::call_video::TestCapture>,
    /// The tallest picture and highest frame rate chosen for a screen share on this device. `None` is no limit.
    screen_quality: Mutex<(Option<u32>, Option<u32>)>,
    /// Whether this device's share includes the computer's sound. Off until the sharer turns it on.
    screen_sound_on: AtomicBool,
    /// The volume this viewer chose for each sharer's sound, by root id, in percent.
    screen_volumes: Mutex<std::collections::BTreeMap<String, u32>>,
    /// The bitrate chosen for a screen share on this device, in kbps.
    screen_kbps: std::sync::atomic::AtomicU32,
    /// This device's camera while the lobby previews it, before a call. A call takes the camera
    /// over, so the preview is dropped when the call's camera turns on.
    lobby_camera: Mutex<Option<Sending>>,
    pub(crate) current: Mutex<Option<Arc<Room>>>,
    /// A migrated room being built. It carries audio and offers only once it is swapped in.
    staged: Mutex<Option<Arc<Room>>>,
    /// Live participant count at which a forwarded call grows into a cascade.
    cascade_at: AtomicUsize,
    /// Test override of this device's measured capacity in kbps. Zero means measure.
    capacity_override: AtomicU32,
    /// The last tier moves this device made or followed, and why, newest last, for a test that timed out.
    tier_log: Mutex<VecDeque<(Instant, String)>>,
    /// What the current call's tier moves left behind, for the way back from forwarded. See `TierMemory`.
    tier_memory: Mutex<Option<(RoomId, TierMemory)>>,
    /// Test hook: the next tier announcement goes to only this many peers.
    #[cfg(feature = "test-hooks")]
    tier_announce_reach: Mutex<Option<usize>>,
    /// Test hook: the next tier move swaps in whatever links it made. Taken by that move.
    #[cfg(feature = "test-hooks")]
    swap_without_links: AtomicBool,
    /// Test hook: this device refuses every tier announcement, as one that sees another roster does.
    #[cfg(feature = "test-hooks")]
    refuse_tier_moves: AtomicBool,
    /// Test hook: how long each of the next mesh connects holds the answer it got before applying it, in order.
    #[cfg(feature = "test-hooks")]
    answer_holds: Mutex<VecDeque<Duration>>,
    /// When each device's last `WhereIs` was answered, so a peer cannot keep this device busy answering.
    where_answered: Mutex<BTreeMap<DeviceId, Instant>>,
    /// Rooms made and not yet dropped. A room swapped out of a call should go within a tick.
    pub(crate) rooms_alive: Arc<AtomicUsize>,
    /// Test hook: no `Features` leave this device before then, as when its first one is slow to arrive.
    #[cfg(feature = "test-hooks")]
    features_held_until: Mutex<Option<Instant>>,
    /// Media links dialled to this device, by the device that dialled, with when each arrived.
    incoming: Mutex<BTreeMap<DeviceId, (MediaLink, Instant)>>,
    arrived: Notify,
    pub(crate) ringing: Mutex<Option<Ringing>>,
    pub(crate) ring_timeout: Mutex<Duration>,
    /// Calls this device answers as unanswered because of its call setting, by caller device. See `call_privacy`.
    pub(crate) quiet: Mutex<BTreeMap<RoomId, DeviceId>>,
    /// DM and group calls other devices said they are in, by device.
    heard: Mutex<BTreeMap<DeviceId, Heard>>,
    /// Whether new calls use the voice pipeline instead of synthetic frames.
    voice: AtomicBool,
    /// Capture settings for the voice pipeline. Defaults include noise suppression and AGC.
    sender_config: Mutex<SenderConfig>,
    /// This device's offers to forward calls it is not in, and its relays for them.
    volunteering: volunteers::Volunteering,
}

impl CallHub {
    pub(crate) fn new(timings: &Timings) -> Self {
        Self {
            tiles: Mutex::new(HashMap::new()),
            test_capture: Mutex::new(crate::call_video::TestCapture::Device),
            screen_quality: Mutex::new((None, None)),
            screen_sound_on: AtomicBool::new(false),
            screen_volumes: Mutex::new(Default::default()),
            screen_kbps: std::sync::atomic::AtomicU32::new(dissent_video::adapt::SCREEN_DEFAULT_KBPS),
            encoded_sink: Mutex::new(None),
            lobby_camera: Mutex::new(None),
            current: Mutex::new(None),
            staged: Mutex::new(None),
            cascade_at: AtomicUsize::new(CASCADE_AT),
            capacity_override: AtomicU32::new(0),
            tier_log: Mutex::new(VecDeque::new()),
            tier_memory: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            tier_announce_reach: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            swap_without_links: AtomicBool::new(false),
            #[cfg(feature = "test-hooks")]
            refuse_tier_moves: AtomicBool::new(false),
            #[cfg(feature = "test-hooks")]
            answer_holds: Mutex::new(VecDeque::new()),
            where_answered: Mutex::new(BTreeMap::new()),
            rooms_alive: Arc::new(AtomicUsize::new(0)),
            #[cfg(feature = "test-hooks")]
            features_held_until: Mutex::new(None),
            incoming: Mutex::new(BTreeMap::new()),
            arrived: Notify::new(),
            ringing: Mutex::new(None),
            ring_timeout: Mutex::new(timings.ring_timeout),
            quiet: Mutex::new(BTreeMap::new()),
            heard: Mutex::new(BTreeMap::new()),
            voice: AtomicBool::new(false),
            sender_config: Mutex::new(SenderConfig::default()),
            volunteering: volunteers::Volunteering::default(),
        }
    }
}

/// Receives the media links that other devices dial. Each one waits in the hub until the call
/// that expects it takes it.
pub(crate) struct MediaSink(Weak<Inner>);

impl MediaHandler for MediaSink {
    fn incoming(&self, from: DeviceId, link: MediaLink) {
        if let Some(inner) = self.0.upgrade() {
            lock(&inner.calls.incoming).insert(from, (link, Instant::now()));
            inner.calls.arrived.notify_waiters();
        }
    }
}

/// The media handler the engine installs when the caller did not bring one.
pub(crate) fn media_sink(inner: &Arc<Inner>) -> Arc<dyn MediaHandler> {
    Arc::new(MediaSink(Arc::downgrade(inner)))
}

/// One remote participant, in the shape the call panel reads.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ParticipantView {
    /// Base32 root id.
    pub root_id: String,
    pub display_name: String,
    pub muted: bool,
    /// What the person's own device last said about deafening. False when it has not said.
    pub deafened: bool,
    /// Video is not built yet, so this is always false.
    pub video: bool,
    pub speaking: bool,
    /// True when this device relays for others. Not used yet, so always false.
    pub forwarder: bool,
    pub path: PathView,
    /// This person's camera is on, but the forwarder pauses it for this device to fit its bandwidth.
    /// The window shows the last frame with a "paused" badge.
    pub video_paused: bool,
    /// The same for their shared screen.
    pub screen_paused: bool,
    /// Their screen is reaching this device, or is paused for it.
    pub screen: bool,
    /// Their screen's sound is reaching this device.
    pub screen_audio: bool,
}

/// Whether a participant's link goes straight, or through a relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathView {
    Direct,
    Relayed,
}

/// The shape of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topology {
    /// Two participants.
    P2p,
    /// Three to six participants, with a link between every pair.
    Mesh,
    /// Reserved. Not produced yet: larger calls currently use a mesh too.
    Forwarded,
    /// Reserved. Not produced yet.
    Cascade,
}

/// Link quality, averaged over the call's links.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct CallQuality {
    pub rtt_ms: Option<f64>,
    pub loss_pct: f64,
    pub jitter_ms: f64,
    /// Frames whose sender's signature did not match its digest. Each one mutes that sender for
    /// the rest of the epoch, so a healthy call shows zero.
    pub flagged: u64,
}

/// A short sentence the engine writes about how the call is routed. The UI shows it verbatim.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CallRoute {
    pub kind: String,
    pub title: String,
    pub detail: String,
}

/// Whether the call is a direct call or a voice channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallScope {
    Dm,
    Channel,
    /// A call among the members of a group conversation.
    Group,
}

/// The state of the call on this device. `call_state` returns `None` when the device is not in
/// a call. Serialised with the field names of `docs/design/07-visual-system.md` part 6.3.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CallView {
    /// Hex of the room id.
    pub id: String,
    pub scope: CallScope,
    /// Base32 root id of the other person, for a direct call.
    pub peer: Option<String>,
    /// This device's mute state.
    pub muted: bool,
    /// Not built yet, so always false.
    pub deafened: bool,
    /// Not built yet, so always false.
    pub camera: bool,
    /// When this device joined, in milliseconds.
    pub started_ms: u64,
    pub route: CallRoute,
    /// The other participants, not this device.
    pub participants: Vec<ParticipantView>,
    pub topology: Topology,
    pub quality: CallQuality,
    /// The MLS epoch the call key comes from. Changes when the group changes. Zero for direct calls.
    pub key_epoch: u64,
    /// Frames decrypted and delivered so far, for diagnostics.
    pub frames_received: u64,
    /// Frames have been arriving and none has decrypted for a while, so this device hears nothing.
    pub cannot_decrypt: bool,
    /// The display name of a member who forwards the call without being in it, as the call
    /// screen's "Forwarded by" names them. `None` when a participant forwards, or nobody does.
    pub forwarded_by: Option<String>,
    /// Whether this device's person is speaking now: it sent audio within `speaking::SPEAKING_HOLD`
    /// and is not muted.
    pub self_speaking: bool,
    /// For a voice channel call: the channel's and the server's names, for the rail's "Connected to #name".
    pub channel_name: Option<String>,
    pub community_name: Option<String>,
}

/// The sender id of a device in a direct call. A direct call has no MLS group, so each device
/// takes an id from its device id. Ids are 24-bit, so collisions are rare among a few devices.
pub(crate) fn device_sid(device: &DeviceId) -> u64 {
    let hash = blake3::hash(&device.0);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    let id = u64::from_le_bytes(bytes) & MAX_SENDER_ID;
    if id == 0 {
        1
    } else {
        id
    }
}

fn room_id(parts: &[&[u8]]) -> RoomId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ROOM_DOMAIN);
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn call_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("call failed: {e}"))
}

/// The route sentence for the call panel. The engine writes it, the UI shows it verbatim.
fn route_for(
    scope: CallScope,
    topology: Topology,
    participants: &[ParticipantView],
    relayed: bool,
    forwarded_by: Option<&str>,
) -> CallRoute {
    let via = if relayed {
        " Some links go through a relay."
    } else {
        ""
    };
    match (topology, participants.first()) {
        // A voice channel or group with one other person in it is not a direct call, though its
        // audio takes the same straight path.
        (Topology::P2p, Some(p)) if scope != CallScope::Dm => CallRoute {
            kind: "p2p".into(),
            title: "Voice connected".into(),
            detail: format!("You and {} are here. Audio goes straight between your devices.{via}", p.display_name),
        },
        (Topology::P2p, Some(p)) => CallRoute {
            kind: "p2p".into(),
            title: "Direct call".into(),
            detail: format!("Audio goes straight between you and {}.{via}", p.display_name),
        },
        (Topology::P2p, None) => CallRoute {
            kind: "p2p".into(),
            title: "Waiting for others".into(),
            detail: "Nobody else has connected yet.".into(),
        },
        (Topology::Cascade, _) => CallRoute {
            kind: "cascade".into(),
            title: "Cascaded call".into(),
            detail: format!(
                "Forwarders relay audio between the {} people in this call.{via}",
                participants.len() + 1
            ),
        },
        (Topology::Forwarded, _) => CallRoute {
            kind: "forwarded".into(),
            title: "Forwarded call".into(),
            detail: match forwarded_by {
                Some(name) => format!(
                    "{name}, who is not in this call, relays audio between the {} people in it.{via}",
                    participants.len() + 1
                ),
                None => format!(
                    "A forwarder relays audio between the {} people in this call.{via}",
                    participants.len() + 1
                ),
            },
        },
        _ => CallRoute {
            kind: "mesh".into(),
            title: "Mesh call".into(),
            detail: format!(
                "Each of the {} people links to every other.{via}",
                participants.len() + 1
            ),
        },
    }
}

/// Parses the hex call id that events and `accept_call` carry.
fn parse_call_id(s: &str) -> Result<RoomId> {
    let bad = || CoreError::Invalid("bad call id".into());
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(bad)?;
    bytes.try_into().map_err(|_| bad())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn path_is_direct(link: &MediaLink) -> bool {
    matches!(link.path(), PathKind::Direct | PathKind::Mixed)
}

impl Drop for Room {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Room {
    fn at_step(&self, step: u32) {
        self.step.store(step, Ordering::Relaxed);
        self.step_at.store(now_ms(), Ordering::Relaxed);
    }

    fn new(
        id: RoomId,
        scope: Scope,
        mesh: Mesh<LinkTransport>,
        my_sid: u64,
        epoch: u64,
        peer: Option<RootId>,
        alive: &Arc<AtomicUsize>,
    ) -> Self {
        alive.fetch_add(1, Ordering::Relaxed);
        Self {
            alive: alive.clone(),
            id,
            scope,
            my_sid,
            started_ms: now_ms(),
            peer,
            deafened: AtomicBool::new(false),
            targets: Mutex::new(Vec::new()),
            leaves: Mutex::new(BTreeMap::new()),
            forwarding: None,
            muted: AtomicBool::new(false),
            epoch: AtomicU64::new(epoch),
            mesh: AsyncMutex::new(mesh),
            peers: Mutex::new(BTreeMap::new()),
            quality: Mutex::new(Quality::default()),
            audio: Mutex::new(Box::new(SyntheticAudio::default())),
            gen: id,
            tier: Tier::Mesh,
            live: Mutex::new(BTreeMap::new()),
            tier_since: Mutex::new(None),
            migrating: AtomicBool::new(false),
            capacities: Mutex::new(BTreeMap::new()),
            own_kbps: AtomicU32::new(0),
            swapped_at: Mutex::new(None),
            capacity_clock: AtomicU64::new(0),
            forwarder_since: Mutex::new(None),
            features: Mutex::new(BTreeMap::new()),
            seen_live: Mutex::new(BTreeMap::new()),
            lonely: Mutex::new((None, None)),
            told: Mutex::new(std::collections::BTreeSet::new()),
            offers: Mutex::new(BTreeMap::new()),
            stall: Mutex::new(volunteers::StallWatch::new()),
            layer_signals: Mutex::new(crate::call_layers::LayerSignals::default()),
            layered: Mutex::new(std::collections::BTreeSet::new()),
            sending: AtomicBool::new(true),
            links: AtomicUsize::new(0),
            media_links: Mutex::new(BTreeMap::new()),
            clocked: AtomicBool::new(false),
            camera: Mutex::new(None),
            screen: Mutex::new(None),
            remote_video: Mutex::new(RemoteVideo::default()),
            loop_stats: Default::default(),
            driven: AtomicBool::new(false),
            link_estimate: AtomicU32::new(0),
            forced_estimate: AtomicU32::new(0),
            #[cfg(feature = "test-hooks")]
            cpu_held: AtomicBool::new(false),
            screen_sound: Mutex::new(Default::default()),
            keyframe_wanted: AtomicBool::new(false),
            speaking: Mutex::new(speaking::Speaking::default()),
            sends: Mutex::new(SendCounts::default()),
            step: AtomicU32::new(0),
            step_at: AtomicU64::new(0),
            mesh_by: AtomicU32::new(0),
        }
    }

    /// Keeps `link` as this room's link to `device`, replacing an older one.
    fn note_link(&self, device: DeviceId, link: &MediaLink) {
        lock(&self.media_links).insert(device, link.clone());
    }

    /// The device this device links to for media in a forwarded call: its forwarder, or its parent in
    /// a cascade. `None` for a mesh, or on the root forwarder itself.
    fn forwarder_device(&self) -> Option<DeviceId> {
        let forwarding = self.forwarding.as_ref()?;
        (forwarding.forwarder != self.my_sid).then(|| self.signal_device(forwarding.forwarder))?
    }

    /// The path a participant's media takes to this device, read from the live link now: their own
    /// link if this room has one, else the link to `forwarder` (see `forwarder_device`), which
    /// carries their media in a forwarded call. With neither, nothing reaches this device straight
    /// from them, so it is relayed.
    fn path_of(&self, device: &DeviceId, forwarder: Option<&DeviceId>) -> PathView {
        let links = lock(&self.media_links);
        let via = links.get(device).or_else(|| links.get(forwarder?));
        match via {
            Some(link) if path_is_direct(link) => PathView::Direct,
            _ => PathView::Relayed,
        }
    }

    fn add_peer(&self, sid: u64, device: DeviceId, root: RootId) {
        lock(&self.peers).insert(
            sid,
            Peer {
                device,
                root,
                muted: false,
                last_frame: None,
                last_arrival: None,
                jitter_ms: 0.0,
            },
        );
    }

    /// Records one decrypted frame from `sid` and updates that peer's jitter.
    fn note_frame(&self, sid: u64, now: Instant) {
        self.loop_stats[3].fetch_add(1, Ordering::Relaxed);
        let mut peers = lock(&self.peers);
        if let Some(peer) = peers.get_mut(&sid) {
            if let Some(prev) = peer.last_arrival {
                let gap_ms = now.duration_since(prev).as_secs_f64() * 1000.0;
                let deviation = (gap_ms - FRAME_MS as f64).abs();
                peer.jitter_ms += (deviation - peer.jitter_ms) / 16.0;
            }
            peer.last_arrival = Some(now);
            peer.last_frame = Some(now);
        }
    }

    pub(crate) fn id(&self) -> RoomId {
        self.id
    }

    pub(crate) fn my_sid(&self) -> u64 {
        self.my_sid
    }

    pub(crate) fn forwarding(&self) -> Option<&Forwarding> {
        self.forwarding.as_ref()
    }

    /// The sender id a live device has in this call.
    pub(crate) fn live_sid(&self, device: &DeviceId) -> Option<u64> {
        lock(&self.live).iter().find(|(_, d)| *d == device).map(|(sid, _)| *sid)
    }

    /// The live device behind sender id `sid`.
    pub(crate) fn device_of(&self, sid: u64) -> Option<DeviceId> {
        lock(&self.live).get(&sid).copied()
    }

    /// The device to send signalling for sender id `sid` to: a live one, or this leaf's volunteer
    /// forwarder, which is not live.
    pub(crate) fn signal_device(&self, sid: u64) -> Option<DeviceId> {
        self.device_of(sid).or_else(|| {
            let forwarding = self.forwarding.as_ref()?;
            let (device, _) = forwarding.volunteer?;
            (forwarding.forwarder == sid).then_some(device)
        })
    }

    /// Whether `device` is this device's forwarder, a participant or a volunteer.
    pub(crate) fn is_forwarder_device(&self, device: &DeviceId) -> bool {
        self.forwarding
            .as_ref()
            .is_some_and(|f| self.sid_of(device) == Some(f.forwarder))
    }

    /// The sender id of a person in this call, by their root id as the window names them.
    pub(crate) fn sid_of_root(&self, root: &str) -> Option<u64> {
        lock(&self.peers).iter().find(|(_, peer)| peer.root.to_string() == root).map(|(sid, _)| *sid)
    }

    /// The root id of the person behind sender id `sid`, as the window names them.
    pub(crate) fn root_of_sid(&self, sid: u64) -> Option<String> {
        lock(&self.peers).get(&sid).map(|peer| peer.root.to_string())
    }

    /// Whether the device with sender id `sid` sent `bit` in its `Features`.
    pub(crate) fn peer_supports(&self, sid: u64, bit: u64) -> bool {
        lock(&self.features).get(&sid).is_some_and(|(bits, _)| bits & bit != 0)
    }

    pub(crate) fn with_layers<R>(&self, f: impl FnOnce(&mut crate::call_layers::LayerSignals) -> R) -> R {
        f(&mut lock(&self.layer_signals))
    }

    pub(crate) fn note_layered_stream(&self, sid: u64, screen: bool) {
        lock(&self.layered).insert((sid, screen));
    }

    pub(crate) fn layered_streams(&self) -> Vec<(u64, bool)> {
        lock(&self.layered).iter().copied().collect()
    }

    /// The layer table this device's camera or screen can send, as (width, kbps), smallest first.
    /// `None` while that capture is off.
    pub(crate) fn sending_layers(&self, screen: bool) -> Option<Vec<(u32, u32)>> {
        let slot = if screen { &self.screen } else { &self.camera };
        lock(slot).as_ref().map(|sending| sending.layers().into_iter().map(|l| (l.width, l.kbps)).collect())
    }

    /// What this device's screen sends now, for tests: width, height, fps and kbps.
    pub(crate) fn screen_plan(&self) -> Option<(u32, u32, u32, u32)> {
        lock(&self.screen).as_ref().map(|sending| {
            let plan = sending.applied_plan();
            (plan.width, plan.height, plan.fps, plan.kbps)
        })
    }

    /// The screen encoder's last share of its frame interval, for tests.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn screen_cpu_share(&self) -> Option<f64> {
        lock(&self.screen).as_ref().and_then(|sending| sending.last_share)
    }

    /// The bitrate this device's screen may reach now, for tests.
    pub(crate) fn screen_target(&self) -> Option<u32> {
        lock(&self.screen).as_ref().map(|sending| sending.budget.top_kbps())
    }

    /// The layers this device's camera or screen was asked for, `None` for one stream.
    pub(crate) fn wanted_layers(&self, screen: bool) -> Option<u8> {
        let slot = if screen { &self.screen } else { &self.camera };
        lock(slot).as_ref().and_then(|sending| sending.wanted_layers())
    }

    /// Hands the layers a forwarder asked for to this device's camera or screen. `None` is one
    /// adaptive stream, as in a mesh.
    pub(crate) fn set_wanted_layers(&self, screen: bool, mask: Option<u8>) {
        let slot = if screen { &self.screen } else { &self.camera };
        if let Some(sending) = lock(slot).as_mut() {
            sending.set_wanted_layers(mask);
        }
    }

    /// The sender id of `device` in this call, or `None` if it is not a participant. A direct call
    /// has two: the caller is 0 and the callee is 1. A channel call uses the MLS leaf index.
    fn sid_of(&self, device: &DeviceId) -> Option<u64> {
        match self.scope {
            Scope::Dm => lock(&self.leaves).get(device).copied(),
            Scope::Channel { .. } | Scope::Group { .. } => lock(&self.leaves).get(device).copied(),
        }
    }

    /// Whether this device makes the offer for its link to `device`. In a mesh the lower id
    /// offers. In a forwarded call each leaf offers to the forwarder, and no two leaves link.
    fn offers_to(&self, device: &DeviceId) -> bool {
        match &self.forwarding {
            Some(f) => {
                let sid = self.sid_of(device);
                match f.role {
                    Role::Root | Role::Standby => false,
                    Role::Child => sid == Some(f.forwarder) || (sid.is_some() && sid == f.standby),
                    Role::Leaf => sid == Some(f.forwarder),
                }
            }
            None => self.sid_of(device).is_some_and(|sid| self.my_sid < sid),
        }
    }

    fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }
}

/// True if `room` is still the call this device is in.
/// Empties the staged slot if it still holds `room`. A newer move's room there is left alone.
fn clear_staged(inner: &Inner, room: &Arc<Room>) {
    let mut staged = lock(&inner.calls.staged);
    if staged.as_ref().is_some_and(|s| Arc::ptr_eq(s, room)) {
        *staged = None;
    }
}

fn is_current(inner: &Inner, room: &Arc<Room>) -> bool {
    let matches = |c: &Arc<Room>| Arc::ptr_eq(c, room);
    lock(&inner.calls.current).as_ref().is_some_and(matches)
        || lock(&inner.calls.staged).as_ref().is_some_and(matches)
}

/// Forwarded calls. The forwarder relays between the leaves. A leaf receives from its forwarder
/// and sends through it. A forwarder does not send its own audio yet, since it has no leaf link.
async fn pump_forwarded(engine: &Engine, room: &Room, forwarding: &Forwarding) {
    {
        let mut sfu = forwarding.sfu.lock().await;
        if let Some(sfu) = sfu.as_mut() {
            while let Ok(Ok(true)) = tokio::time::timeout(Duration::from_millis(1), sfu.step()).await {}
        }
    }
    let mut leaf = forwarding.leaf.lock().await;
    let Some(participant) = leaf.as_mut() else {
        return;
    };
    let (undecryptable, received) = participant.link_stats().values().fold((0, 0), |(u, r), s| (u + s.frames_undecryptable, r + s.frames_received));
    note_deaf(&mut lock(&room.quality), room, undecryptable, received);
    while let Ok(Ok(Some(frame))) =
        tokio::time::timeout(Duration::from_millis(1), participant.next_received_frame()).await
    {
        room.note_frame(frame.sender, Instant::now());
        lock(&room.quality).frames_received += 1;
        lock(&room.stall).heard = Some(Instant::now());
        lock(&room.audio).incoming(&frame, now_ms());
    }
    let mut outgoing = Vec::new();
    if room.sending.load(Ordering::SeqCst) {
        lock(&room.audio).outgoing(&mut outgoing);
    }
    if !room.is_muted() {
        for (timestamp, bytes) in outgoing {
            // The first byte of a frame is its audio level, which the forwarder ranks speakers by.
            let level = bytes.first().copied().unwrap_or(127);
            match participant.send_audio_frame_at(&bytes, timestamp, level).await {
                Ok(()) => {
                    lock(&room.stall).note_spoke(Instant::now());
                    room.note_sent_audio();
                }
                Err(e) => debug!(error = %e, "call frame not sent"),
            }
        }
    }
    pump_video_leaf(engine, room, participant).await;
}

/// Takes the links' frame totals, and warns once when the call has stopped decrypting anything.
fn note_deaf(quality: &mut Quality, room: &Room, undecryptable: u64, received: u64) {
    if quality.deaf.note(undecryptable, received, Instant::now()) {
        warn!(epoch = room.epoch.load(Ordering::Relaxed), undecryptable, "frames arrive in this call and none decrypts");
    }
}

/// Fallback audio target for the budget, in kbps, before the call has measured one.
const DEFAULT_AUDIO_KBPS: u32 = 32;

/// Takes the frames that the sending captures have ready, each with whether it is a screen. A
/// paused capture's frames are dropped, because the link cannot carry them.
fn take_encoded(room: &Room) -> Vec<(Encoded, bool)> {
    let mut out = Vec::new();
    for (slot, screen) in [(&room.camera, false), (&room.screen, true)] {
        let sending = lock(slot);
        if let Some(sending) = sending.as_ref() {
            while let Some(encoded) = sending.capture.try_next() {
                if !sending.paused {
                    out.push((encoded, screen));
                }
            }
            // Sound goes on while video is paused: it is the smaller stream, and the sharer's words may depend on it.
            while let Some(encoded) = sending.sound.as_ref().and_then(|sound| sound.try_next()) {
                out.push((encoded, screen));
            }
        }
    }
    out
}

/// Sizes every sending capture to the link's estimate, minus the audio reservation.
fn fit_sending(room: &Room, available_kbps: Option<u32>, audio_kbps: u32) {
    if let Some(kbps) = available_kbps {
        room.link_estimate.store(kbps, Ordering::Relaxed);
    }
    // A test can hold the estimate, so it checks what follows from a rate rather than the machine's load.
    let forced = room.forced_estimate.load(Ordering::Relaxed);
    let available_kbps = if forced > 0 { Some(forced) } else { available_kbps };
    #[cfg(feature = "test-hooks")]
    let cpu_held = room.cpu_held.load(Ordering::Relaxed);
    #[cfg(not(feature = "test-hooks"))]
    let cpu_held = false;
    for slot in [&room.camera, &room.screen] {
        if let Some(sending) = lock(slot).as_mut() {
            sending.fit(available_kbps, audio_kbps, cpu_held);
        }
    }
}

/// Tells each sending capture whether every peer agreed to AV1.
fn set_av1(room: &Room, on: bool) {
    for slot in [&room.camera, &room.screen] {
        if let Some(sending) = lock(slot).as_mut() {
            sending.set_av1(on);
        }
    }
}

/// Asks each sending capture for a keyframe, because a peer needs one.
fn request_keyframes(room: &Room) {
    for slot in [&room.camera, &room.screen] {
        if let Some(sending) = lock(slot).as_ref() {
            sending.capture.request_keyframe();
        }
    }
}

/// Shows this device's own camera and screen in the window, as the self tile.
fn show_own_previews(engine: &Engine, room: &Room) {
    let root_id = engine.inner.identity.root_id().to_string();
    for slot in [&room.camera, &room.screen] {
        let sending = lock(slot);
        let Some(sending) = sending.as_ref() else {
            continue;
        };
        if let Some(preview) = sending.capture.try_preview() {
            let screen = sending.capture.kind() == CaptureKind::Screen;
            emit_preview(engine, preview.sender, &root_id, screen, &preview);
        }
    }
}

/// Emits one preview as a window event, with the picture as base64 JPEG.
fn emit_preview(engine: &Engine, sender: u64, root_id: &str, screen: bool, preview: &Preview) {
    engine.emit(EngineEvent::VideoFrame {
        sender,
        root_id: root_id.to_string(),
        screen,
        av1: preview.av1,
        width: preview.width,
        height: preview.height,
        jpeg: base64::engine::general_purpose::STANDARD.encode(&preview.jpeg),
    });
}

/// Shows a peer's frame, and tells the window about the picture it decodes to. The preview is as
/// wide as the window draws the tile, when the window has said so, and the default otherwise.
fn show_video(engine: &Engine, room: &Room, frame: &dissent_media::VideoFrame) {
    use crate::call_video::{LARGE_PREVIEW_INTERVAL, PREVIEW_INTERVAL};
    if frame.codec == dissent_media::VideoCodec::Opus {
        let root = lock(&room.peers).get(&frame.sender).map(|peer| peer.root.to_string()).unwrap_or_default();
        let before = lock(&room.screen_sound).sounding().len();
        let mut mix = lock(&room.screen_sound);
        mix.receive(frame.sender, &root, &frame.payload);
        let now = mix.sounding().len();
        drop(mix);
        if now != before {
            engine.emit(EngineEvent::CallUpdated);
        }
        return;
    }
    room.with_layers(|l| {
        l.received.insert((frame.sender, frame.screen), frame.layer);
        let count = l.received_at.get(&(frame.sender, frame.screen)).map_or(0, |(_, n)| *n) + 1;
        l.received_at.insert((frame.sender, frame.screen), (Instant::now(), count));
    });
    room.with_layers(|l| l.video_at = Some(Instant::now()));
    if frame.screen && room.with_layers(|l| l.screen_seen.insert(frame.sender, Instant::now()).is_none()) {
        engine.emit(EngineEvent::CallUpdated);
    }
    let root_id = lock(&room.peers)
        .get(&frame.sender)
        .map(|peer| peer.root.to_string())
        .unwrap_or_default();
    // A window that decodes the frame itself gets it as it was sent: no decode here, and no JPEG.
    let sink = lock(&engine.inner.calls.encoded_sink).clone();
    if let Some(sink) = sink {
        let taken = sink(EncodedVideo {
            root_id: root_id.clone(),
            screen: frame.screen,
            av1: frame.codec == dissent_media::VideoCodec::Av1,
            keyframe: frame.keyframe,
            frame_id: frame.frame_id,
            payload: frame.payload.clone(),
        });
        if taken {
            return;
        }
    }
    let tile = (root_id.clone(), frame.screen);
    let (width, largest) = {
        let tiles = lock(&engine.inner.calls.tiles);
        // The widest tile is the largest. A tie goes to the lower key, so exactly one tile is largest.
        let largest = tiles
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(key, _)| key.clone());
        (tiles.get(&tile).copied().unwrap_or(crate::call_video::PREVIEW_WIDTH), largest)
    };
    let interval = if largest.as_ref() == Some(&tile) { LARGE_PREVIEW_INTERVAL } else { PREVIEW_INTERVAL };
    // Decoded on the stream's own thread: this runs in the call loop, which also carries the audio.
    let job = crate::call_video::DecodeJob { frame: frame.clone(), width, interval };
    let sink = || -> crate::call_video::PreviewSink {
        // Weak, so a decoder thread never keeps an engine that shut down alive.
        let weak = std::sync::Arc::downgrade(&engine.inner);
        std::sync::Arc::new(move |preview: Preview| {
            if let Some(inner) = weak.upgrade() {
                emit_preview(&Engine { inner }, preview.sender, &root_id, preview.screen, &preview);
            }
        })
    };
    if lock(&room.remote_video).push(job, sink) {
        room.keyframe_wanted.store(true, Ordering::Relaxed);
    }
}

/// How long each 20 ms tick works the links for video while any flows. Packets are only read and sent while the
/// tick waits here, so this sets what a call can carry: at 1 ms a call topped out near 2 Mbps, and the frames
/// beyond that queued for seconds. The rest of the tick stays free for audio.
const VIDEO_PUMP: Duration = Duration::from_millis(10);
/// The same with no video, which only needs to notice the first frame.
const IDLE_PUMP: Duration = Duration::from_millis(1);
/// How long after the last received frame video counts as flowing.
const VIDEO_FLOWING: Duration = Duration::from_secs(2);

fn video_pump_window(room: &Room) -> Duration {
    let sending = lock(&room.camera).is_some() || lock(&room.screen).is_some();
    let receiving = room.with_layers(|l| l.video_at.is_some_and(|at| at.elapsed() < VIDEO_FLOWING));
    if sending || receiving {
        VIDEO_PUMP
    } else {
        IDLE_PUMP
    }
}

/// How long after a screen's last frame the share counts as over. Longer than a large keyframe takes on a thin link.
const SCREEN_GONE: Duration = Duration::from_secs(8);

/// Forgets a peer's share once its frames have stopped for `SCREEN_GONE`, and tells the window.
fn expire_screens(engine: &Engine, room: &Room) {
    let gone = room.with_layers(|l| {
        let before = l.screen_seen.len();
        l.screen_seen.retain(|_, seen| seen.elapsed() < SCREEN_GONE);
        l.screen_seen.len() != before
    });
    if gone {
        engine.emit(EngineEvent::CallUpdated);
    }
}

/// Sends this device's camera and screen through the mesh, and shows the peers' frames.
async fn pump_video_mesh(engine: &Engine, room: &Room, mesh: &mut Mesh<LinkTransport>) {
    expire_screens(engine, room);
    if room.keyframe_wanted.swap(false, Ordering::Relaxed) {
        mesh.request_keyframes();
    }
    let audio = mesh
        .stats()
        .values()
        .map(|stats| stats.target_kbps)
        .min()
        .unwrap_or(DEFAULT_AUDIO_KBPS);
    fit_sending(room, mesh.video_available_kbps(), audio);
    set_av1(room, mesh.all_peers_have_av1());
    for (encoded, screen) in take_encoded(room) {
        // A mesh sends one stream, which the capture tags layer 0.
        let meta = VideoMeta {
            frame_id: encoded.frame_id,
            keyframe: encoded.keyframe,
            codec: encoded.codec,
            screen,
            layer: encoded.layer,
        };
        if let Err(e) = mesh.send_video(&encoded.data, meta).await {
            debug!(error = %e, "video frame not sent");
        }
    }
    if mesh.take_keyframe_request() {
        request_keyframes(room);
    }
    let until = tokio::time::Instant::now() + video_pump_window(room);
    while let Ok(Ok(Some(frame))) = tokio::time::timeout_at(until, mesh.next_video_frame()).await {
        lock(&room.quality).frames_received += 1;
        show_video(engine, room, &frame);
    }
    show_own_previews(engine, room);
}

/// Sends this device's camera and screen to its forwarder, and shows the frames the forwarder
/// relays.
async fn pump_video_leaf(engine: &Engine, room: &Room, participant: &mut Participant<CallLink>) {
    expire_screens(engine, room);
    if room.keyframe_wanted.swap(false, Ordering::Relaxed) {
        participant.request_keyframes();
    }
    let audio = participant
        .link_stats()
        .values()
        .map(|stats| stats.target_kbps)
        .min()
        .unwrap_or(DEFAULT_AUDIO_KBPS);
    fit_sending(room, participant.video_available_kbps(), audio);
    set_av1(room, participant.peer_has_av1());
    for (encoded, screen) in take_encoded(room) {
        // A forwarded call can carry simulcast layers. The capture sends them only after the
        // forwarder asked (`call_layers`), and tags its single stream layer 0 until then.
        let meta = VideoMeta {
            frame_id: encoded.frame_id,
            keyframe: encoded.keyframe,
            codec: encoded.codec,
            screen,
            layer: encoded.layer,
        };
        if let Err(e) = participant.send_video(&encoded.data, meta).await {
            debug!(error = %e, "video frame not sent");
        }
    }
    if participant.take_keyframe_request() {
        request_keyframes(room);
    }
    let until = tokio::time::Instant::now() + video_pump_window(room);
    while let Ok(Ok(Some(frame))) = tokio::time::timeout_at(until, participant.next_video_frame()).await {
        lock(&room.quality).frames_received += 1;
        show_video(engine, room, &frame);
    }
    show_own_previews(engine, room);
}

/// Runs one call: sends a synthetic frame every 20 ms, drains arriving frames, and every half
/// second rekeys a channel call and drops banned participants. Stops when the call ends.
async fn drive(weak: Weak<Inner>, room: Weak<Room>) {
    drive_loop(weak, room.clone()).await;
    if let Some(room) = room.upgrade() {
        room.driven.store(false, Ordering::SeqCst);
    }
}

/// Spawns the call loop for `room`.
fn spawn_drive(engine: &Engine, room: &Arc<Room>) {
    room.driven.store(true, Ordering::SeqCst);
    engine.spawn_task(drive(Arc::downgrade(&engine.inner), Arc::downgrade(room)));
}

async fn drive_loop(weak: Weak<Inner>, room: Weak<Room>) {
    let mut tick = tokio::time::interval(Duration::from_millis(FRAME_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut since_check: u32 = 0;
    loop {
        if let Some(room) = room.upgrade() {
            room.at_step(step::TICK);
        }
        let due = tick.tick().await;
        let (Some(inner), Some(room)) = (weak.upgrade(), room.upgrade()) else {
            break;
        };
        let late = due.elapsed();
        room.loop_stats[0].fetch_add(1, Ordering::Relaxed);
        if late > LATE_TICK {
            room.loop_stats[1].fetch_add(1, Ordering::Relaxed);
        }
        room.loop_stats[2].fetch_max(late.as_micros() as u64, Ordering::Relaxed);
        if !is_current(&inner, &room) {
            break;
        }
        let engine = Engine { inner };
        room.at_step(step::MESH_LOCK);
        let mut mesh = room.mesh.lock().await;
        room.mesh_by.store(mesh_site::DRIVE, Ordering::Relaxed);
        #[cfg(feature = "test-hooks")]
        mesh.set_drop_percent(engine.inner.mesh_drop_percent.load(Ordering::Relaxed));
        since_check += 1;
        if since_check >= MEMBERSHIP_EVERY {
            since_check = 0;
            room.at_step(step::MEMBERSHIP);
            engine.check_membership(&room, &mut mesh).await;
            engine.refresh_quality(&room, &mesh);
        }
        if let Some(forwarding) = room.forwarding.as_ref() {
            drop(mesh);
            room.at_step(step::FORWARDED);
            pump_forwarded(&engine, &room, forwarding).await;
            room.at_step(step::SPEAKING);
            engine.check_speaking(&room);
            continue;
        }
        room.at_step(step::RECEIVE);
        while let Ok(Ok(Some(frame))) =
            tokio::time::timeout(Duration::from_millis(1), mesh.next_received_frame()).await
        {
            room.note_frame(frame.sender, Instant::now());
            lock(&room.quality).frames_received += 1;
            lock(&room.audio).incoming(&frame, now_ms());
        }
        let mut outgoing = Vec::new();
        if room.sending.load(Ordering::SeqCst) {
            lock(&room.audio).outgoing(&mut outgoing);
        }
        if !room.sending.load(Ordering::SeqCst) {
            lock(&room.sends).not_sending += 1;
        } else if outgoing.is_empty() {
            lock(&room.sends).empty += 1;
        }
        room.at_step(step::SEND);
        if !room.is_muted() {
            for (timestamp, bytes) in outgoing {
                match mesh.send_audio_frame_at(&bytes, timestamp).await {
                    Ok(()) => {
                        lock(&room.sends).sent += 1;
                        room.note_sent_audio()
                    }
                    Err(e) => {
                        *lock(&room.sends).failed.entry(e.to_string()).or_default() += 1;
                        debug!(error = %e, "call frame not sent")
                    }
                }
            }
        }
        room.at_step(step::SPEAKING);
        engine.check_speaking(&room);
        room.at_step(step::VIDEO);
        pump_video_mesh(&engine, &room, &mut mesh).await;
    }
}

impl Engine {
    /// The forwarded-tier state for this device, with `forwarder` elected. The forwarder gets a
    /// relay. Every other device gets a leaf link to it. `volunteer` names a forwarder that is not
    /// in the call.
    #[allow(clippy::too_many_arguments)]
    fn forwarding_for(
        &self,
        sids: &[u64],
        my_sid: u64,
        key: MediaKey,
        auth: Auth,
        forwarder: u64,
        volunteer: Option<(DeviceId, RootId)>,
    ) -> Result<Forwarding> {
        let role = if forwarder == my_sid {
            Role::Root
        } else {
            Role::Leaf
        };
        let mut forwarding = build_forwarding(role, forwarder, Vec::new(), None, 0, my_sid, sids.len(), key, auth)?;
        forwarding.volunteer = volunteer;
        Ok(forwarding)
    }

    /// The cascade tier: a root, child forwarders under it, and every other device a leaf of one
    /// child. Every device computes the same tree from the same roster.
    fn cascade_forwarding(
        &self,
        sids: &[u64],
        my_sid: u64,
        key: MediaKey,
        auth: Auth,
    ) -> Result<Forwarding> {
        let candidates: Vec<Candidate> = sids.iter().map(|s| candidate(*s, DEFAULT_KBPS)).collect();
        let child_count = cascade_child_count(sids.len());
        let tree = cascade_plan(&candidates, sids.len(), child_count, Mode::AudioTop3)
            .ok_or_else(|| call_err("too few devices for a cascade"))?;
        let standby_sid = tree.standby_root;
        let leaves: Vec<u64> = sids
            .iter()
            .copied()
            .filter(|s| *s != tree.root && *s != standby_sid && !tree.children.contains(s))
            .collect();
        let assignment = assign_forwarders(&leaves, &tree.children);
        let served = assignment
            .values()
            .filter(|(primary, _)| *primary == my_sid)
            .count();
        let (role, forwarder, children, standby, served) = if my_sid == tree.root {
            (Role::Root, my_sid, tree.children.clone(), None, 0)
        } else if my_sid == standby_sid {
            (Role::Standby, my_sid, tree.children.clone(), None, 0)
        } else if tree.children.contains(&my_sid) {
            (Role::Child, tree.root, Vec::new(), Some(standby_sid), served)
        } else {
            let primary = assignment
                .get(&my_sid)
                .map(|(primary, _)| *primary)
                .ok_or_else(|| call_err("this device has no forwarder"))?;
            (Role::Leaf, primary, Vec::new(), None, 0)
        };
        build_forwarding(role, forwarder, children, standby, served, my_sid, sids.len(), key, auth)
    }

    /// A cascade child's upstream link to the root. The root accepts it as a downlink of its relay.
    async fn connect_upstream(&self, room: Arc<Room>, device: DeviceId) -> Result<()> {
        let Some(forwarding) = room.forwarding.as_ref() else {
            return Err(call_err("this call is not forwarded"));
        };
        let root = room
            .sid_of(&device)
            .ok_or_else(|| call_err("this device is not a parent in this call"))?;
        let link = self.node().open_media(device).await?;
        room.note_link(device, &link);
        let transport = LinkTransport::new(link, Side::Caller);
        let offer = {
            let mut sfu = forwarding.sfu.lock().await;
            let relay = sfu
                .as_mut()
                .ok_or_else(|| call_err("this device relays nothing"))?;
            relay
                .offer_upstream(root, CallLink::Net(transport))
                .await
                .map_err(call_err)?
        };
        let reply = self
            .call_rpc(
                device,
                CallMsg::Offer {
                    room: room.id,
                    gen: room.gen,
                    signal: offer.to_bytes().map_err(call_err)?,
                },
            )
            .await?;
        match reply {
            CallReply::Answer(bytes) => {
                let answer = Signal::from_bytes(&bytes).map_err(call_err)?;
                {
                    let mut sfu = forwarding.sfu.lock().await;
                    if let Some(relay) = sfu.as_mut() {
                        relay.apply_upstream(root, answer).map_err(call_err)?;
                    }
                }
                room.add_peer(root, device, self.root_of_device(&device));
                room.links.fetch_add(1, Ordering::SeqCst);
                self.emit(EngineEvent::CallUpdated);
                Ok(())
            }
            CallReply::Refused(why) => Err(call_err(why)),
            other => Err(call_err(format!("unexpected reply {other:?}"))),
        }
    }

    /// The forwarder's own speech: a link from its leaf side to its own relay, in process.
    async fn connect_self(&self, room: &Arc<Room>) -> Result<()> {
        let Some(forwarding) = room.forwarding.as_ref() else {
            return Ok(());
        };
        let sid = room.my_sid;
        let (leaf_end, relay_end) = memory_pair();
        let offer = {
            let mut leaf = forwarding.leaf.lock().await;
            let participant = leaf
                .as_mut()
                .ok_or_else(|| call_err("no leaf side on the forwarder"))?;
            participant
                .offer_sfu(sid, CallLink::Memory(leaf_end))
                .await
                .map_err(call_err)?
        };
        let answer = {
            let mut sfu = forwarding.sfu.lock().await;
            let relay = sfu
                .as_mut()
                .ok_or_else(|| call_err("this device is not the forwarder"))?;
            relay
                .accept(sid, CallLink::Memory(relay_end), offer)
                .await
                .map_err(call_err)?
        };
        let mut leaf = forwarding.leaf.lock().await;
        if let Some(participant) = leaf.as_mut() {
            participant.apply_sfu(sid, answer).map_err(call_err)?;
            participant
                .assign_forwarders(sid, None)
                .map_err(call_err)?;
        }
        Ok(())
    }

    /// Leaf side of a forwarded call: links to the forwarder, and sends and receives through it.
    async fn connect_leaf(&self, room: Arc<Room>, device: DeviceId) -> Result<()> {
        let Some(forwarding) = room.forwarding.as_ref() else {
            return Err(call_err("this call is not forwarded"));
        };
        let sid = forwarding.forwarder;
        // A volunteer learns this leaf's sender id from the leaf, since a seeder has no group to read.
        if forwarding.volunteer.is_some() {
            self.tell_relay_leaf(&room, device).await?;
        }
        let link = self.node().open_media(device).await?;
        room.note_link(device, &link);
        let transport = LinkTransport::new(link, Side::Caller);
        let offer = {
            let mut leaf = forwarding.leaf.lock().await;
            let participant = leaf
                .as_mut()
                .ok_or_else(|| call_err("this device is the forwarder"))?;
            participant.offer_sfu(sid, CallLink::Net(transport)).await.map_err(call_err)?
        };
        let reply = self
            .call_rpc(
                device,
                CallMsg::Offer {
                    room: room.id,
                    gen: room.gen,
                    signal: offer.to_bytes().map_err(call_err)?,
                },
            )
            .await?;
        match reply {
            CallReply::Answer(bytes) => {
                let answer = Signal::from_bytes(&bytes).map_err(call_err)?;
                {
                    let mut leaf = forwarding.leaf.lock().await;
                    if let Some(participant) = leaf.as_mut() {
                        participant.apply_sfu(sid, answer).map_err(call_err)?;
                        participant
                            .assign_forwarders(sid, None)
                            .map_err(call_err)?;
                    }
                }
                // A volunteer forwarder is not in the call, so it is not listed as a participant.
                if forwarding.volunteer.is_none() {
                    room.add_peer(sid, device, self.root_of_device(&device));
                }
                room.links.fetch_add(1, Ordering::SeqCst);
                self.emit(EngineEvent::CallUpdated);
                Ok(())
            }
            CallReply::Refused(why) => Err(call_err(why)),
            other => Err(call_err(format!("unexpected reply {other:?}"))),
        }
    }

    /// Members of a community's group with their MLS leaf indices, as sender ids.
    fn indexed_members(&self, community: CommunityId) -> Result<Vec<(u64, DeviceId, DeviceCert)>> {
        let members = self
            .with_mls_read(|mls| mls.indexed_members(&community.0))
            .map_err(call_err)?;
        Ok(members
            .into_iter()
            .map(|(index, cert)| (u64::from(index), cert.device, cert))
            .collect())
    }

    /// The auth bundle for a mesh: our device key and the cert of every device that may join.
    fn auth_for(&self, certs: &[(u64, DeviceCert)]) -> Auth {
        let map: HashMap<u64, DeviceCert> =
            certs.iter().map(|(sid, c)| (*sid, c.clone())).collect();
        let device = DeviceKey::from_seed(self.inner.identity.device.0.seed());
        Auth::new(device, map)
    }

    /// The channel key for `community` and the MLS epoch it belongs to.
    fn channel_key(&self, community: CommunityId) -> Result<(MediaKey, u64)> {
        let group = community.0;
        let (epoch, secret) = self.with_mls_read(|mls| -> dissent_mls::Result<(u64, Vec<u8>)> {
            Ok((
                mls.epoch(&group)?,
                mls.export_secret(&group, KEY_LABEL, 32)?,
            ))
        })
        .map_err(call_err)?;
        let bytes: [u8; 32] = secret
            .try_into()
            .map_err(|_| call_err("exported key is not 32 bytes"))?;
        Ok((MediaKey::new(bytes), epoch))
    }

    fn my_cert(&self) -> DeviceCert {
        self.inner.identity.cert.clone()
    }

    fn my_addr(&self) -> Result<PeerAddr> {
        self.read()
            .my_addr
            .clone()
            .ok_or_else(|| CoreError::Invalid("this device has no address yet".into()))
    }

    /// Fails if this device is already in a different call. Returns the room if it is in the
    /// same one.
    fn check_free(&self, id: RoomId) -> Result<Option<Arc<Room>>> {
        match lock(&self.inner.calls.current).clone() {
            Some(room) if room.id == id => Ok(Some(room)),
            Some(_) => Err(CoreError::Invalid("already in a call".into())),
            None => Ok(None),
        }
    }

    /// Makes `room` the current call and starts driving it.
    fn install(&self, room: Arc<Room>) {
        if self.inner.calls.voice.load(Ordering::Relaxed) {
            let mut config = *lock(&self.inner.calls.sender_config);
            if let Scope::Channel { community, channel } = room.scope {
                let w = self.read();
                if let Some(c) = w.communities.get(&community) {
                    // The channel's setting, held to the server's boost ceiling. See `boosts`.
                    let ceiling = crate::boosts::ceilings(w.boost_level(c, crate::engine::now_ms())).voice_kbps;
                    let kbps = c.state().settings(&channel).bitrate_kbps.min(ceiling);
                    config.bitrate_bps = config.bitrate_bps.min(kbps.saturating_mul(1000));
                }
            }
            match VoiceAudio::new(config) {
                Ok(voice) => *lock(&room.audio) = Box::new(voice),
                Err(e) => warn!(error = %e, "voice audio unavailable; this call is silent"),
            }
        }
        *lock(&self.inner.calls.current) = Some(room.clone());
        spawn_drive(self, &room);
        self.watch_driver(&room);
        // A device in a call of its own forwards no other call as a volunteer.
        let engine = self.clone();
        self.spawn_task(async move { engine.withdraw_forward_offers(None).await });
    }

    /// Makes new calls use the voice pipeline (Opus, jitter buffer, DSP) instead of synthetic
    /// frames. The desktop app calls this once at start. Then it feeds captured audio with
    /// [`Engine::push_capture`] and plays [`Engine::pull_playout`]. Takes effect for calls that
    /// start after it.
    pub fn set_voice_audio(&self, enabled: bool) {
        self.inner.calls.voice.store(enabled, Ordering::Relaxed);
    }

    /// Capture settings for the voice pipeline: bitrate, expected loss, noise suppression, AGC.
    /// Takes effect for calls that start after it.
    pub fn set_voice_sender_config(&self, config: SenderConfig) {
        *lock(&self.inner.calls.sender_config) = config;
    }

    /// Adds captured microphone samples to the current call. Samples come in 960-sample frames
    /// at 48 kHz. Does nothing with synthetic audio.
    pub fn push_capture(&self, samples: &[f32]) -> Result<()> {
        let room = self.current_or_err()?;
        let result = lock(&room.audio).capture(samples);
        result
    }

    /// Fills `out` with the next 960 samples of the call's mixed playout, at 48 kHz. Fills with
    /// silence when there is no call, the device is deafened, or the audio is synthetic.
    pub fn pull_playout(&self, out: &mut [f32]) {
        match lock(&self.inner.calls.current).clone() {
            Some(room) if !room.deafened.load(Ordering::Relaxed) => {
                lock(&room.audio).playout(now_ms(), out);
                let volumes = lock(&self.inner.calls.screen_volumes);
                lock(&room.screen_sound).mix(out, &volumes);
            }
            _ => out.fill(0.0),
        }
    }

    /// The playout loss counts of the current call, `(lost, concealed, silenced)`. Zero with no call.
    pub fn playout_loss(&self) -> (u64, u64, u64) {
        match lock(&self.inner.calls.current).clone() {
            Some(room) => lock(&room.audio).loss(),
            None => (0, 0, 0),
        }
    }

    /// Makes a voice channel in `community`. Its calls work like text channels, with the media
    /// keys taken from the community's group.
    pub async fn create_voice_channel(
        &self,
        community: CommunityId,
        name: String,
    ) -> Result<crate::views::ChannelInfo> {
        let name = crate::engine::clean_text(&name, "channel name")?;
        let payload = Payload::VoiceChannelCreate { name: name.clone() };
        let se = self
            .author_publish(community, ChannelId::CONTROL, payload)
            .await?;
        Ok(crate::views::ChannelInfo::new(
            ChannelId(se.event.id.0),
            name,
            true,
            &ChannelSettings::default(),
        ))
    }

    /// Joins the call in a voice channel, creating it if nobody is in it yet.
    pub async fn join_call(&self, community: CommunityId, channel: ChannelId) -> Result<()> {
        let scope = Scope::Channel { community, channel };
        let room_id = room_id(&[&community.0[..], &channel.0[..]]);
        if self.check_free(room_id)?.is_some() {
            return Ok(());
        }
        let devices = {
            let w = self.read();
            let c = w
                .communities
                .get(&community)
                .ok_or_else(|| crate::engine::not_found(&community))?;
            if !c.state().voice_channels.contains(&channel) {
                return Err(CoreError::Invalid("not a voice channel".into()));
            }
            if c.state().banned.contains(&self.inner.me) {
                return Err(CoreError::Rejected("Banned".into()));
            }
            // A seeder is never in a call, so it is not told who joins: a join carries the joiner's addresses.
            channel_call_devices(&w, c, false)
                .into_iter()
                .filter(|d| *d != self.inner.identity.device_id())
                .collect::<Vec<DeviceId>>()
        };
        // A full channel admits no one else. The count is what this device has heard, so it is the
        // best this device can check, and the community's other devices check it the same way.
        let limit = self
            .read()
            .communities
            .get(&community)
            .map(|c| c.state().settings(&channel).user_limit)
            .unwrap_or(0);
        if limit > 0 && self.voice_headcount(community, channel) >= limit as usize {
            return Err(CoreError::Rejected("This voice channel is full".into()));
        }
        self.wait_until_sealable(community).await;
        if self.banned_device_in_group(community) {
            return Err(CoreError::Invalid("a banned member could not be removed from this server yet, so the call did not start: try again in a moment".into()));
        }
        let (key, epoch) = self.channel_key(community)?;
        let indexed = self.indexed_members(community)?;
        let my_device = self.inner.identity.device_id();
        let my_sid = indexed
            .iter()
            .find(|(_, d, _)| *d == my_device)
            .map(|(sid, _, _)| *sid)
            .ok_or_else(|| CoreError::Invalid("this device is not in the group yet".into()))?;
        let certs: Vec<(u64, DeviceCert)> = indexed.iter().map(|(s, _, c)| (*s, c.clone())).collect();
        let auth = self.auth_for(&certs);
        let mesh = Mesh::new_authenticated(key.clone(), my_sid, auth.clone()).map_err(call_err)?;
        // A call starts as a mesh. The live roster moves it to a forwarded tier when it grows.
        let room = Room::new(room_id, scope, mesh, my_sid, epoch, None, &self.inner.calls.rooms_alive);
        lock(&room.live).insert(my_sid, self.inner.identity.device_id());
        let room = Arc::new(room);
        lock(&room.leaves).extend(indexed.iter().map(|(s, d, _)| (*d, *s)));
        self.install(room.clone());
        // The window shows the call from here: reaching the others can take seconds.
        self.emit(EngineEvent::CallUpdated);
        if let Err(e) = self.connect_self(&room).await {
            warn!(error = %e, "forwarder's own link not made");
        }
        self.join_devices(&room, &devices, None).await;
        self.share_call(&room);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Calls a contact directly. The contact's first device rings. Nothing connects until it
    /// accepts. A direct call has no MLS group, so its two participants take sender ids 0 for the
    /// caller and 1 for the callee.
    pub async fn call_dm(&self, peer: RootId) -> Result<()> {
        let me = self.inner.me;
        // Every certified device of the contact rings. The first one to accept wins.
        let (devices, certs) = {
            let w = self.read();
            if !w.is_contact(&peer) {
                return Err(CoreError::Invalid("can only call a contact".into()));
            }
            let devices = w.directory.devices_of(&peer);
            if devices.is_empty() {
                return Err(CoreError::NotFound("no known device for that contact".into()));
            }
            let certs: Vec<DeviceCert> = devices
                .iter()
                .filter_map(|d| w.directory.cert_of(d))
                .collect();
            (devices, certs)
        };
        let (a, b) = if me <= peer { (me, peer) } else { (peer, me) };
        let room_id = room_id(&[&a.0[..], &b.0[..]]);
        if self.check_free(room_id)?.is_some() {
            return Ok(());
        }
        // chisle: a DM call has no MLS group yet. The caller picks the key, and it reaches the
        // peer only inside the authenticated device RPC. Switch to the DM group when it exists.
        let key = DeviceKey::generate().0.seed();
        let my_sid = device_sid(&self.inner.identity.device_id());
        let mut auth_certs = vec![(my_sid, self.my_cert())];
        auth_certs.extend(certs.iter().map(|c| (device_sid(&c.device), c.clone())));
        let mesh = Mesh::new_authenticated(MediaKey::new(key), my_sid, self.auth_for(&auth_certs))
            .map_err(call_err)?;
        let room = Room::new(room_id, Scope::Dm, mesh, my_sid, 0, Some(peer), &self.inner.calls.rooms_alive);
        lock(&room.leaves).extend(devices.iter().map(|d| (*d, device_sid(d))));
        let room = Arc::new(room);
        self.install(room.clone());
        if self.join_devices(&room, &devices, Some(key)).await == 0 {
            *lock(&self.inner.calls.current) = None;
            self.emit(EngineEvent::CallUpdated);
            return Err(CoreError::Rejected(
                "that person cannot be called right now".into(),
            ));
        }
        self.share_call(&room);
        self.end_if_unanswered(room.id);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Returns how many devices answered, either in the call or ringing.
    async fn join_devices(&self, room: &Arc<Room>, devices: &[DeviceId], key: Option<[u8; 32]>) -> usize {
        self.join_devices_ring(room, devices, key, false).await
    }

    /// As `join_devices`. `ring` asks devices that are not in the call to ring for it.
    async fn join_devices_ring(&self, room: &Arc<Room>, devices: &[DeviceId], key: Option<[u8; 32]>, ring: bool) -> usize {
        let muted = room.is_muted();
        *lock(&room.targets) = devices.to_vec();
        let from = match self.my_addr() {
            Ok(addr) => addr,
            Err(e) => {
                warn!(error = %e, "call not joined: no address");
                return 0;
            }
        };
        // Every device is asked at once. One after another, each offline device held up the rest for the request
        // timeout, so joining a channel with members away took that long times their number.
        let mut asks = tokio::task::JoinSet::new();
        for device in devices.iter().copied() {
            let msg = CallMsg::Join(Box::new(JoinMsg {
                ring,
                room: room.id,
                scope: room.scope,
                cert: self.my_cert(),
                key,
                muted,
                leaf: match room.scope {
                    Scope::Channel { .. } | Scope::Group { .. } => Some(room.my_sid),
                    Scope::Dm => None,
                },
                from: from.clone(),
            }));
            let (engine, room) = (self.clone(), room.clone());
            asks.spawn(async move { engine.join_one(room, device, msg).await });
        }
        let mut answered = 0;
        while let Some(result) = asks.join_next().await {
            answered += usize::from(result.unwrap_or(false));
        }
        answered
    }

    /// Asks one device to join, and links to it if it answers from the call. True when it answered.
    async fn join_one(&self, room: Arc<Room>, device: DeviceId, msg: CallMsg) -> bool {
        match self.call_rpc(device, msg).await {
            Ok(CallReply::Here { cert, .. }) if cert.device == device => {
                if let Some(sid) = room.sid_of(&device) {
                    lock(&room.live).insert(sid, device);
                }
                // A forwarded call shows everyone who is in it, even without a direct link.
                if let (Some(_), Some(sid)) = (&room.forwarding, room.sid_of(&device)) {
                    room.add_peer(sid, device, cert.root);
                }
                if room.offers_to(&device) {
                    if let Err(e) = self.connect(room.clone(), device).await {
                        warn!(device = %device, error = %e, "call link not made");
                        self.note_tier(format!("link to {} not made: {e}", &device.to_string()[..8]));
                    }
                }
                true
            }
            Ok(CallReply::Ringing) => true,
            Ok(reply) => {
                debug!(device = %device, ?reply, "device is not in the call");
                false
            }
            Err(e) => {
                debug!(device = %device, error = %e, "call join not delivered");
                false
            }
        }
    }

    /// Offering side of one pair: dials the link, offers, and applies the answer.
    async fn connect(&self, room: Arc<Room>, device: DeviceId) -> Result<()> {
        match room.forwarding.as_ref().map(|f| f.role) {
            Some(Role::Child) => return self.connect_upstream(room, device).await,
            Some(_) => return self.connect_leaf(room, device).await,
            None => {}
        }
        let sid = room
            .sid_of(&device)
            .ok_or_else(|| call_err("device is not in this call"))?;
        let link = self.node().open_media(device).await?;
        room.note_link(device, &link);
        let transport = LinkTransport::new(link, Side::Caller);
        let (offer, token) = {
            let mut mesh = room.mesh.lock().await;
            room.mesh_by.store(mesh_site::OFFER, Ordering::Relaxed);
            // A call already there is replaced only when it no longer works, as when the peer came back from another
            // room. A working one stays, and so does one still in its handshake, so crossing or duplicate offers cannot
            // tear it down.
            mesh.replace_if_stale(sid, STALE_LINK, self.inner.timings.connect_grace).map_err(call_err)?;
            mesh.offer(sid, transport).await.map_err(call_err)?
        };
        let reply = self
            .call_rpc(
                device,
                CallMsg::Offer {
                    room: room.id,
                    gen: room.gen,
                    signal: offer.to_bytes().map_err(call_err)?,
                },
            )
            .await?;
        match reply {
            CallReply::Answer(bytes) => {
                let answer = Signal::from_bytes(&bytes).map_err(call_err)?;
                #[cfg(feature = "test-hooks")]
                {
                    let hold = lock(&self.inner.calls.answer_holds).pop_front();
                    if let Some(hold) = hold {
                        tokio::time::sleep(hold).await;
                    }
                }
                // The token ties the answer to this offer's call. If another connect replaced that call meanwhile,
                // the answer is refused rather than applied to a call that never offered it.
                room.mesh.lock().await.apply(sid, token, answer).map_err(call_err)?;
                room.add_peer(sid, device, self.root_of_device(&device));
                room.links.fetch_add(1, Ordering::SeqCst);
                if matches!(room.scope, Scope::Dm) {
                    self.cancel_other_targets(&room, device);
                }
                self.emit(EngineEvent::CallUpdated);
                Ok(())
            }
            CallReply::Refused(why) => Err(call_err(why)),
            other => Err(call_err(format!("unexpected reply {other:?}"))),
        }
    }

    fn root_of_device(&self, device: &DeviceId) -> RootId {
        self.read()
            .directory
            .root_of(device)
            .unwrap_or(self.inner.me)
    }

    /// Waits for the media link that `from` dials to us.
    async fn wait_incoming(&self, from: DeviceId) -> Result<MediaLink> {
        let deadline = tokio::time::Instant::now() + LINK_WAIT;
        loop {
            // Created before the check, so a link that lands in between still wakes us.
            let arrived = self.inner.calls.arrived.notified();
            match lock(&self.inner.calls.incoming).remove(&from) {
                // A link that waited longer than any offer takes to follow its link belongs to an earlier offer that
                // never came, or was refused. Taking it would bind this offer's call to a link its sender no longer
                // reads, so it is dropped, and this offer waits for its own.
                Some((_, at)) if at.elapsed() > STALE_INCOMING => {
                    self.note_tier(format!("dropped a media link from {} that waited {:?}", &from.to_string()[..8], at.elapsed()));
                }
                Some((link, _)) => return Ok(link),
                None => {}
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                return Err(call_err("no media link from the peer"));
            }
        }
    }

    /// Sends one signalling message and waits for its reply.
    pub(crate) async fn call_rpc(&self, device: DeviceId, msg: CallMsg) -> Result<CallReply> {
        let bytes = self.request_bytes(&Request::Call(Box::new(msg)))?;
        let raw = tokio::time::timeout(RPC_TIMEOUT, self.node().request(device, bytes))
            .await
            .map_err(|_| call_err("call request timed out"))??;
        match decode::<Response>(&raw)? {
            Response::Call(reply) => Ok(reply),
            Response::Error(e) => Err(call_err(e)),
            other => Err(call_err(format!("unexpected reply {other:?}"))),
        }
    }

    /// Answers a call signalling message from `from`. Runs under the RPC handler.
    pub(crate) async fn handle_call(&self, from: DeviceId, msg: CallMsg) -> CallReply {
        match msg {
            CallMsg::Join(join) => {
                let JoinMsg {
                    room,
                    scope,
                    cert,
                    key,
                    muted,
                    leaf,
                    ring,
                    from: addr,
                } = *join;
                if let (Scope::Channel { .. } | Scope::Group { .. }, Some(sid)) = (scope, leaf) {
                    self.admit_leaf(room, from, sid, &cert).await;
                }
                self.on_join(from, room, scope, cert, key, muted, addr, ring)
            }
            CallMsg::Offer { room, gen, signal } => self.on_offer(from, room, gen, signal).await,
            CallMsg::Tier(msg) => self.on_tier(from, *msg).await,
            CallMsg::Capacity { room, kbps, sig } => self.on_capacity(from, room, kbps, sig),
            CallMsg::Mute { room, muted } => {
                if let Some(current) = self.current_room(&room) {
                    let Some(sid) = current.sid_of(&from) else {
                        return CallReply::Refused("not in this call".into());
                    };
                    if let Some(peer) = lock(&current.peers).get_mut(&sid) {
                        peer.muted = muted;
                    }
                    self.emit(EngineEvent::CallUpdated);
                    self.announce_call(&current, self.roster_of(&current));
                }
                CallReply::Ack
            }
            CallMsg::AnsweredElsewhere { room } => {
                let cancelled = {
                    let mut ringing = lock(&self.inner.calls.ringing);
                    if ringing.as_ref().is_some_and(|r| r.id == room && r.from == from) {
                        ringing.take()
                    } else {
                        None
                    }
                };
                if cancelled.is_some() {
                    self.emit(EngineEvent::CallAnsweredElsewhere {
                        call_id: hex(&room),
                    });
                }
                CallReply::Ack
            }
            CallMsg::Decline { room } => {
                // The other side refused, or gave up before anyone joined. A direct call with no one in it ends.
                // A group call keeps going: one member declining says nothing about the others.
                let unanswered = self
                    .current_room(&room)
                    .is_some_and(|r| matches!(r.scope, Scope::Dm) && lock(&r.peers).is_empty());
                if unanswered {
                    *lock(&self.inner.calls.current) = None;
                    self.emit(EngineEvent::CallUpdated);
                }
                CallReply::Ack
            }
            CallMsg::Leave { room } if self.end_quiet_ring(&room, &from) => CallReply::Ack,
            CallMsg::Leave { room } => {
                // The caller gave up while the call rang here. Stop ringing and report it missed.
                let cancelled = {
                    let mut ringing = lock(&self.inner.calls.ringing);
                    if ringing.as_ref().is_some_and(|r| r.id == room && r.from == from) {
                        ringing.take()
                    } else {
                        None
                    }
                };
                if let Some(r) = cancelled {
                    self.emit(EngineEvent::CallMissed {
                        call_id: hex(&room),
                        from: r.peer.to_string(),
                    });
                }
                if self.current_room(&room).is_none() {
                    // A leaf of a call this device forwards as a volunteer.
                    self.drop_relay_leaf(&room, &from).await;
                }
                if let Some(current) = self.current_room(&room) {
                    let Some(sid) = current.sid_of(&from) else {
                        return CallReply::Refused("not in this call".into());
                    };
                    current.mesh.lock().await.remove_peer(sid);
                    lock(&current.peers).remove(&sid);
                    lock(&current.remote_video).forget(sid);
                    lock(&current.live).remove(&sid);
                    self.emit(EngineEvent::CallUpdated);
                    self.announce_call(&current, self.roster_of(&current));
                }
                CallReply::Ack
            }
            CallMsg::Presence(msg) => self.on_presence(from, *msg),
            CallMsg::Features { room, bits } => self.on_features(from, room, bits),
            CallMsg::VideoLayers { room, screen, layers } => self.on_video_layers(from, room, screen, layers).await,
            CallMsg::TileWidths { room, tiles } => self.on_tile_widths(from, room, tiles).await,
            CallMsg::WantedLayers { room, screen, mask } => self.on_wanted_layers(from, room, screen, mask),
            CallMsg::ForwardOffer(offer) => self.on_forward_offer(from, *offer),
            CallMsg::ForwarderStalled { room, forwarder, spoke } => self.on_forwarder_stalled(from, room, forwarder, spoke),
            CallMsg::RelayLeaf { room, gen, sid } => self.on_relay_leaf(from, room, gen, sid).await,
            CallMsg::PausedVideo { room, streams } => self.on_paused_video(from, room, streams),
            CallMsg::WhereIs { room } => self.on_where_is(from, room).await,
        }
    }

    /// Records what a device says about its DM or group call. Only a contact, or a member of the
    /// group, may say so. An empty roster means that device has left its call.
    fn on_presence(&self, from: DeviceId, msg: PresenceMsg) -> CallReply {
        let Some(root) = self.read().directory.root_of(&from) else {
            return CallReply::Refused("unknown device".into());
        };
        if let Err(why) = self.may_join(&msg.scope, root) {
            return CallReply::Refused(why);
        }
        if msg.members.len() > MAX_PRESENCE_MEMBERS {
            return CallReply::Refused("roster too long".into());
        }
        {
            let mut heard = lock(&self.inner.calls.heard);
            if msg.members.is_empty() {
                heard.remove(&from);
            } else {
                heard.insert(
                    from,
                    Heard {
                        room: msg.room,
                        scope: msg.scope,
                        started_ms: msg.started_ms,
                        from: root,
                        members: msg.members,
                        seen_ms: now_ms(),
                    },
                );
            }
        }
        match msg.scope {
            Scope::Channel { community, .. } => self.emit(EngineEvent::CommunityUpdated {
                community_id: community.to_string(),
            }),
            _ => self.emit_conversation(msg.scope, Some(root)),
        }
        CallReply::Ack
    }

    /// Announces a call that has just connected, and keeps its roster re-sent while it lasts.
    fn share_call(&self, room: &Arc<Room>) {
        self.announce_call(room, self.roster_of(room));
        self.start_presence_clock(room);
    }

    /// Re-sends the roster every `PRESENCE_EVERY` while the room is the current call. Only one
    /// clock runs per room. It holds the room weakly, so it does not keep the room alive.
    fn start_presence_clock(&self, room: &Arc<Room>) {
        if room.clocked.swap(true, Ordering::Relaxed) {
            return;
        }
        let engine = self.clone();
        let room = Arc::downgrade(room);
        self.spawn_task(async move {
            loop {
                tokio::time::sleep(engine.inner.timings.call_presence_every).await;
                let Some(room) = room.upgrade() else { break };
                let current = lock(&engine.inner.calls.current).clone();
                if !current.is_some_and(|c| Arc::ptr_eq(&c, &room)) {
                    break;
                }
                engine.announce_call(&room, engine.roster_of(&room));
            }
        });
    }

    /// Tells the people who could join a DM or group call what its roster is now: every group
    /// member's device, or the peer's devices for a DM. Also tells this device's own conversation
    /// list that the call changed.
    fn announce_call(&self, room: &Arc<Room>, members: Vec<PresenceMember>) {
        let me = self.inner.identity.device_id();
        let devices = match room.scope {
            Scope::Group { conv } => self.group_devices(conv).unwrap_or_default(),
            Scope::Dm => room
                .peer
                .map(|peer| self.read().directory.devices_of(&peer))
                .unwrap_or_default(),
            Scope::Channel { community, .. } => {
                let w = self.read();
                w.communities
                    .get(&community)
                    .map(|c| channel_call_devices(&w, c, volunteers::seeders_hear_call(c.state().seeder_forwards_calls, members.len())))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| *d != me)
                    .collect()
            }
        };
        let seeders = self.storage_devices(room.scope);
        for device in devices {
            // A seeder hears a call only to offer to forward it, so it gets who is in it and nothing more.
            let to_seeder = seeders.contains(&device);
            let msg = PresenceMsg {
                room: room.id,
                scope: room.scope,
                started_ms: if to_seeder { 0 } else { room.started_ms },
                members: if to_seeder { volunteers::for_seeder(&members) } else { members.clone() },
            };
            let engine = self.clone();
            self.spawn_task(async move {
                if let Err(e) = engine.call_rpc(device, CallMsg::Presence(Box::new(msg))).await {
                    debug!(device = %device, error = %e, "call presence not sent");
                }
            });
        }
        match room.scope {
            Scope::Channel { community, .. } => self.emit(EngineEvent::CommunityUpdated {
                community_id: community.to_string(),
            }),
            _ => self.emit_conversation(room.scope, room.peer),
        }
    }

    /// This device's roster for a room: itself, and each person whose device has joined.
    fn roster_of(&self, room: &Room) -> Vec<PresenceMember> {
        let me = self.inner.identity.device_id();
        let w = self.read();
        let peers = lock(&room.peers);
        let mut members = vec![PresenceMember {
            root: self.inner.me,
            muted: room.is_muted(),
            deafened: room.deafened.load(Ordering::Relaxed),
        }];
        for device in lock(&room.live).values().filter(|d| **d != me) {
            let Some(root) = w.directory.root_of(device) else {
                continue;
            };
            if members.iter().any(|m| m.root == root) {
                continue;
            }
            let muted = peers.values().any(|p| p.device == *device && p.muted);
            members.push(PresenceMember { root, muted, deafened: false });
        }
        members
    }

    /// Each person's deafen state as their own device last reported it. Only that device's
    /// account counts, so a relayed roster cannot change it.
    fn self_reported_deafen(&self) -> BTreeMap<RootId, bool> {
        let heard: Vec<Heard> = lock(&self.inner.calls.heard).values().cloned().collect();
        let mut out = BTreeMap::new();
        for h in &heard {
            for m in h.members.iter().filter(|m| m.root == h.from) {
                out.insert(m.root, m.deafened);
            }
        }
        out
    }

    /// The people in each voice channel of `community`, as this device has heard them within the
    /// presence TTL. A person's own device reports their mute and deafen state. This device's
    /// own call is included too.
    pub(crate) fn voice_channel_views(&self, community: CommunityId) -> Vec<VoiceChannelView> {
        let now = now_ms();
        let ttl = millis(self.inner.timings.call_presence_ttl);
        let heard: Vec<Heard> = lock(&self.inner.calls.heard)
            .values()
            .filter(|h| now.saturating_sub(h.seen_ms) < ttl)
            .cloned()
            .collect();
        let mine = lock(&self.inner.calls.current).clone();
        let me = self.inner.me;
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (channel, name) in &c.state().channels {
            if !c.state().voice_channels.contains(channel) {
                continue;
            }
            let here = Scope::Channel { community, channel: *channel };
            let mut people: BTreeMap<RootId, VoiceParticipantView> = BTreeMap::new();
            // Each person is shown only from their own device's report: a roster's other entries are
            // relayed, and anyone could list people who are not there (privacy audit P-30).
            for m in heard.iter().filter(|h| h.scope == here).filter_map(Heard::own_entry) {
                if m.root == me || c.state().banned.contains(&m.root) || !c.is_member(&m.root) {
                    continue;
                }
                people.entry(m.root).or_insert_with(|| VoiceParticipantView {
                    root_id: m.root.to_string(),
                    display_name: w.display_name(&m.root),
                    muted: m.muted,
                    deafened: m.deafened,
                    speaking: false,
                });
            }
            // The people this device's own call has live media links with are first-hand too, so they show before their
            // own roster arrives.
            if let Some(room) = mine.as_ref().filter(|r| r.scope == here) {
                let my_device = self.inner.identity.device_id();
                let peers = lock(&room.peers);
                for device in lock(&room.live).values().filter(|d| **d != my_device) {
                    let Some(root) = w.directory.root_of(device) else { continue };
                    if root == me || c.state().banned.contains(&root) || !c.is_member(&root) {
                        continue;
                    }
                    people.entry(root).or_insert_with(|| VoiceParticipantView {
                        root_id: root.to_string(),
                        display_name: w.display_name(&root),
                        muted: peers.values().any(|p| p.device == *device && p.muted),
                        deafened: false,
                        speaking: false,
                    });
                }
            }
            // This device's own call: its person, once, and who speaks in it now.
            if let Some(room) = mine.as_ref().filter(|r| r.scope == here) {
                let now = Instant::now();
                let speaking = room.speaking_roots(now);
                for person in people.values_mut() {
                    person.speaking = speaking.contains(&person.root_id);
                }
                people.insert(
                    me,
                    VoiceParticipantView {
                        root_id: me.to_string(),
                        display_name: w.display_name(&me),
                        muted: room.is_muted(),
                        deafened: room.deafened.load(Ordering::Relaxed),
                        speaking: room.self_speaking(now),
                    },
                );
            }
            out.push(VoiceChannelView {
                id: channel.to_string(),
                name: name.clone(),
                participants: people.into_values().collect(),
            });
        }
        out
    }

    /// How many people other than this one are heard in a voice channel.
    pub(crate) fn voice_headcount(&self, community: CommunityId, channel: ChannelId) -> usize {
        let me = self.inner.me.to_string();
        self.voice_channel_views(community)
            .iter()
            .filter(|v| v.id == channel.to_string())
            .flat_map(|v| v.participants.iter())
            .filter(|p| p.root_id != me)
            .count()
    }

    /// Emits that a DM or group conversation changed, so the conversation list reads it again.
    fn emit_conversation(&self, scope: Scope, peer: Option<RootId>) {
        let id = match (scope, peer) {
            (Scope::Group { conv }, _) => conv.to_string(),
            (Scope::Dm, Some(peer)) => dm_conv_id(&self.inner.me, &peer).to_string(),
            _ => return,
        };
        self.emit(EngineEvent::ConversationChanged { id });
    }

    /// A direct call was answered by `winner`. The other devices of the contact stop ringing.
    fn cancel_other_targets(&self, room: &Arc<Room>, winner: DeviceId) {
        let others: Vec<DeviceId> = lock(&room.targets)
            .iter()
            .copied()
            .filter(|d| *d != winner)
            .collect();
        for device in others {
            let engine = self.clone();
            let id = room.id;
            self.spawn_task(async move {
                let msg = CallMsg::AnsweredElsewhere { room: id };
                if let Err(e) = engine.call_rpc(device, msg).await {
                    debug!(device = %device, error = %e, "answered-elsewhere not delivered");
                }
            });
        }
    }

    /// The room a message names: the staged or current room of the given id, and of the given
    /// generation if one is named.
    fn room_for(&self, id: &RoomId, gen: Option<&RoomId>) -> Option<Arc<Room>> {
        let matches = |r: &Arc<Room>| r.id == *id && gen.is_none_or(|g| r.gen == *g);
        let staged = lock(&self.inner.calls.staged).clone().filter(matches);
        staged.or_else(|| lock(&self.inner.calls.current).clone().filter(matches))
    }

    /// Waits up to five seconds for the room an offer names. A migration on the other side can
    /// reach this device a moment before this device has built its own room.
    async fn await_room(&self, id: &RoomId, gen: &RoomId) -> Option<Arc<Room>> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(room) = self.room_for(id, Some(gen)).or_else(|| self.relay_room_gen(id, gen)) {
                return Some(room);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(crate) fn current_room(&self, id: &RoomId) -> Option<Arc<Room>> {
        lock(&self.inner.calls.current)
            .clone()
            .filter(|room| room.id == *id)
    }

    /// Answers a join. A device already in the call answers `Here`. A direct call rings and
    /// waits for the user. A channel call is joined only by devices already in it.
    #[allow(clippy::too_many_arguments)]
    fn on_join(
        &self,
        from: DeviceId,
        id: RoomId,
        scope: Scope,
        cert: DeviceCert,
        key: Option<[u8; 32]>,
        _muted: bool,
        addr: PeerAddr,
        ring: bool,
    ) -> CallReply {
        if cert.device != from || cert.verify().is_err() {
            return CallReply::Refused("certificate does not match the sender".into());
        }
        // Someone who may not join hears what anyone hears for a call this device is not in, and
        // this device keeps nothing of theirs, not even the address.
        if let Err(why) = self.may_join(&scope, cert.root) {
            // A DM caller the DM policy keeps out is answered as a call nobody picks up, as the call setting
            // answers, so the caller cannot tell which turned it away.
            if matches!(scope, Scope::Dm) && self.room_for(&id, None).is_none() {
                return self.refuse_quietly(id, from, scope, cert.root, None);
            }
            debug!(device = %from, why, "call join refused");
            return CallReply::Refused("not in this call".into());
        }
        let existing = self.room_for(&id, None);
        if existing.is_none() {
            // A new incoming call. Who can call this device (`call_privacy`) is decided before anything is kept
            // of the caller, the address included.
            match self.call_verdict(&scope, cert.root, ring) {
                crate::call_privacy::Verdict::Ring => {}
                crate::call_privacy::Verdict::Quiet(reason) => {
                    return self.refuse_quietly(id, from, scope, cert.root, Some(reason))
                }
                crate::call_privacy::Verdict::Silent => return self.refuse_quietly(id, from, scope, cert.root, None),
            }
        }
        self.learn_peer(from, addr);
        match existing {
            Some(room) if room.id == id => {
                // A direct call takes one device. A second one that accepts late is refused.
                let taken = matches!(room.scope, Scope::Dm)
                    && lock(&room.peers).values().any(|p| p.device != from);
                if taken {
                    return CallReply::Refused("answered on another device".into());
                }
                // A direct call whose other side has stepped away stays open for a rejoin. A
                // rejoin brings a fresh media key, so the room is rebuilt around that key.
                if let (Scope::Dm, Some(_)) = (room.scope, key) {
                    if lock(&room.peers).is_empty() {
                        return match self.open_dm_room(id, &cert, key) {
                            Ok(fresh) => self.answer_join(&fresh, from, cert.root),
                            Err(e) => CallReply::Refused(e.to_string()),
                        };
                    }
                }
                return self.answer_join(&room, from, cert.root);
            }
            Some(_) => return CallReply::Refused("busy in another call".into()),
            None => {}
        }
        match scope {
            Scope::Channel { .. } => CallReply::Refused("not in this call".into()),
            Scope::Dm => self.ring(id, from, cert, key),
            Scope::Group { conv } if ring => self.ring_group(id, from, cert, conv),
            Scope::Group { .. } => CallReply::Refused("not in this call".into()),
        }
    }

    /// A refused call, answered as an unanswered one. See `call_privacy`.
    fn refuse_quietly(
        &self,
        id: RoomId,
        from: DeviceId,
        scope: Scope,
        caller: RootId,
        note: Option<crate::call_privacy::NoteReason>,
    ) -> CallReply {
        self.ring_quietly(id, from, scope, caller, note)
    }

    /// The device is in this call: say so, and connect if this side has the lower id.
    fn answer_join(&self, room: &Arc<Room>, from: DeviceId, root: RootId) -> CallReply {
        if let Some(sid) = room.sid_of(&from) {
            lock(&room.live).insert(sid, from);
        }
        self.share_call(room);
        if room.forwarding.is_some() {
            if let Some(sid) = room.sid_of(&from) {
                room.add_peer(sid, from, root);
            }
        }
        if room.offers_to(&from) {
            let engine = self.clone();
            let room = room.clone();
            self.spawn_task(async move {
                if let Err(e) = engine.connect(room, from).await {
                    warn!(device = %from, error = %e, "call link not made");
                }
            });
        }
        CallReply::Here {
            cert: self.my_cert(),
            muted: room.is_muted(),
        }
    }

    /// Starts ringing for an incoming direct call. Nothing is connected until the user accepts.
    fn ring(&self, id: RoomId, from: DeviceId, cert: DeviceCert, key: Option<[u8; 32]>) -> CallReply {
        let Some(key) = key else {
            return CallReply::Refused("direct call has no key".into());
        };
        {
            let mut ringing = lock(&self.inner.calls.ringing);
            if let Some(r) = ringing.as_ref() {
                return if r.id == id {
                    CallReply::Ringing
                } else {
                    CallReply::Refused("busy with another incoming call".into())
                };
            }
            *ringing = Some(Ringing {
                id,
                from,
                peer: cert.root,
                cert: cert.clone(),
                scope: Scope::Dm,
                key: Some(key),
            });
        }
        self.emit(EngineEvent::IncomingCall {
            call_id: hex(&id),
            from: cert.root.to_string(),
            scope: CallScope::Dm,
            conv: None,
        });
        let timeout = *lock(&self.inner.calls.ring_timeout);
        let engine = self.clone();
        self.spawn_task(async move {
            tokio::time::sleep(timeout).await;
            if let Some(r) = engine.take_ringing(&id) {
                engine.emit(EngineEvent::CallMissed {
                    call_id: hex(&id),
                    from: r.peer.to_string(),
                });
                if let Err(e) = engine.call_rpc(from, CallMsg::Decline { room: id }).await {
                    debug!(device = %from, error = %e, "missed call not reported to caller");
                }
            }
        });
        CallReply::Ringing
    }

    /// Removes the ringing call if it is `id`.
    fn take_ringing(&self, id: &RoomId) -> Option<Ringing> {
        let mut ringing = lock(&self.inner.calls.ringing);
        if ringing.as_ref().is_some_and(|r| r.id == *id) {
            ringing.take()
        } else {
            None
        }
    }

    /// The user accepts a ringing call. Media starts only now.
    pub async fn accept_call(&self, call_id: String) -> Result<()> {
        let id = parse_call_id(&call_id)?;
        if lock(&self.inner.calls.current).is_some() {
            return Err(CoreError::Invalid("already in a call".into()));
        }
        let ringing = self
            .take_ringing(&id)
            .ok_or_else(|| CoreError::NotFound("no such incoming call".into()))?;
        if let Scope::Group { conv } = ringing.scope {
            // A group call: join it through its group, so every member in the call is linked.
            let room = self.open_group_room(Scope::Group { conv }, group_room_id(&conv)).await?;
            let devices = self.group_devices(conv)?;
            self.join_devices_ring(&room, &devices, None, false).await;
            self.share_call(&room);
            self.emit(EngineEvent::CallUpdated);
            return Ok(());
        }
        let key = ringing
            .key
            .ok_or_else(|| CoreError::Invalid("direct call has no key".into()))?;
        let room = self.open_dm_room(id, &ringing.cert, Some(key))?;
        if self.join_devices(&room, &[ringing.from], None).await == 0 {
            // Another device answered first, and the caller turned this one away.
            *lock(&self.inner.calls.current) = None;
            self.emit(EngineEvent::CallAnsweredElsewhere {
                call_id: hex(&id),
            });
            return Err(CoreError::Rejected("answered on another device".into()));
        }
        self.share_call(&room);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// The user declines a ringing call. The caller is told, and no media starts.
    pub async fn decline_call(&self, call_id: String) -> Result<()> {
        let id = parse_call_id(&call_id)?;
        let ringing = self
            .take_ringing(&id)
            .ok_or_else(|| CoreError::NotFound("no such incoming call".into()))?;
        if let Err(e) = self
            .call_rpc(ringing.from, CallMsg::Decline { room: id })
            .await
        {
            debug!(error = %e, "decline not delivered to caller");
        }
        Ok(())
    }

    /// How long a direct call rings before it counts as missed. Default: 45 seconds.
    pub fn set_ring_timeout(&self, timeout: Duration) {
        *lock(&self.inner.calls.ring_timeout) = timeout;
    }

    /// A direct call the caller ends when nobody answers in time.
    fn end_if_unanswered(&self, id: RoomId) {
        let engine = self.clone();
        let timeout = *lock(&self.inner.calls.ring_timeout);
        self.spawn_task(async move {
            tokio::time::sleep(timeout).await;
            let unanswered = engine
                .current_room(&id)
                .is_some_and(|room| lock(&room.peers).is_empty());
            if unanswered {
                if let Err(e) = engine.leave_call().await {
                    debug!(error = %e, "unanswered call not ended");
                }
            }
        });
    }

    /// The checks a joiner must pass: not banned, and a member of the channel or a contact.
    fn may_join(&self, scope: &Scope, root: RootId) -> std::result::Result<(), String> {
        match scope {
            Scope::Group { conv } => {
                let w = self.read();
                match w.conv_book.convs.get(conv) {
                    Some(meta) if !meta.left && meta.members.contains(&root) => Ok(()),
                    _ => Err("not a member of this group".into()),
                }
            }
            Scope::Dm => {
                if crate::contacts::dm_allowed(&self.read(), &root) {
                    Ok(())
                } else {
                    Err("not a contact".into())
                }
            }
            Scope::Channel { community, channel } => {
                let w = self.read();
                let c = w
                    .communities
                    .get(community)
                    .ok_or_else(|| "unknown community".to_string())?;
                if !c.state().voice_channels.contains(channel) {
                    return Err("not a voice channel".into());
                }
                if c.state().banned.contains(&root) {
                    return Err("banned".into());
                }
                if !c.is_member(&root) {
                    return Err("not a member".into());
                }
                if c.state().members.get(&root).is_some_and(|m| m.storage) {
                    return Err("a seeder does not join calls".into());
                }
                Ok(())
            }
        }
    }

    /// The devices of `scope`'s server's seeders, when it is a voice channel.
    fn storage_devices(&self, scope: Scope) -> Vec<DeviceId> {
        let Scope::Channel { community, .. } = scope else {
            return Vec::new();
        };
        let w = self.read();
        w.communities
            .get(&community)
            .map(|c| {
                c.state()
                    .members
                    .iter()
                    .filter(|(_, m)| m.storage)
                    .flat_map(|(root, _)| w.directory.devices_of(root))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Creates the room for an incoming DM call, keyed by the caller's key.
    fn open_dm_room(&self, id: RoomId, cert: &DeviceCert, key: Option<[u8; 32]>) -> Result<Arc<Room>> {
        let key = key.ok_or_else(|| CoreError::Invalid("DM call has no key".into()))?;
        let my_sid = device_sid(&self.inner.identity.device_id());
        let peer_sid = device_sid(&cert.device);
        let certs = vec![(my_sid, self.my_cert()), (peer_sid, cert.clone())];
        let mesh = Mesh::new_authenticated(MediaKey::new(key), my_sid, self.auth_for(&certs))
            .map_err(call_err)?;
        let room = Room::new(id, Scope::Dm, mesh, my_sid, 0, Some(cert.root), &self.inner.calls.rooms_alive);
        lock(&room.leaves).insert(cert.device, peer_sid);
        let room = Arc::new(room);
        self.install(room.clone());
        self.emit(EngineEvent::CallUpdated);
        Ok(room)
    }

    /// Accepts an offer. The offerer dialled the link, so it is already waiting for us.
    async fn on_offer(&self, from: DeviceId, id: RoomId, gen: RoomId, signal: Vec<u8>) -> CallReply {
        let Some(room) = self.await_room(&id, &gen).await else {
            return CallReply::Refused("not in this call".into());
        };
        let outcome: Result<Vec<u8>> = async {
            let link = self.wait_incoming(from).await?;
            room.note_link(from, &link);
            let offer = Signal::from_bytes(&signal).map_err(call_err)?;
            let sid = room
                .sid_of(&from)
                .ok_or_else(|| call_err("not in this call"))?;
            // A volunteer relays only for current, unbanned members of the server.
            if room.forwarding.as_ref().is_some_and(Forwarding::is_volunteer_relay) {
                self.may_relay_for(&room, &from).map_err(call_err)?;
                lock(&room.live).insert(sid, from);
            }
            let transport = LinkTransport::new(link, Side::Callee);
            let answer = match &room.forwarding {
                // A forwarded call: the forwarder takes the leaf into its relay.
                Some(forwarding) => {
                    let mut sfu = forwarding.sfu.lock().await;
                    let sfu = sfu
                        .as_mut()
                        .ok_or_else(|| call_err("this device is not the forwarder"))?;
                    sfu.accept(sid, CallLink::Net(transport), offer).await.map_err(call_err)?
                }
                None => {
                    let mut mesh = room.mesh.lock().await;
                    room.mesh_by.store(mesh_site::ACCEPT, Ordering::Relaxed);
                    // A peer offers again after it lost the link or came back from another room. A call to it that no
                    // longer works is replaced; a working one, or one still in its handshake, stays, and the offer is
                    // refused.
                    mesh.replace_if_stale(sid, STALE_LINK, self.inner.timings.connect_grace).map_err(call_err)?;
                    mesh.accept(sid, transport, offer).await.map_err(call_err)?
                }
            };
            room.add_peer(sid, from, self.root_of_device(&from));
            room.links.fetch_add(1, Ordering::SeqCst);
            if matches!(room.scope, Scope::Dm) {
                self.cancel_other_targets(&room, from);
            }
            self.emit(EngineEvent::CallUpdated);
            answer.to_bytes().map_err(call_err)
        }
        .await;
        match outcome {
            Ok(bytes) => CallReply::Answer(bytes),
            Err(e) => {
                let sid = room.sid_of(&from).unwrap_or(0);
                let had = room.mesh.lock().await.describe_call(sid);
                self.note_tier(format!(
                    "refused offer from {} gen {}: {e}; call there {had:?}",
                    &from.to_string()[..8],
                    hex(&room.gen[..4])
                ));
                CallReply::Refused(e.to_string())
            }
        }
    }

    /// Sets this device's mute state, tells every participant, and emits a change.
    pub async fn set_mute(&self, muted: bool) -> Result<()> {
        let room = lock(&self.inner.calls.current)
            .clone()
            .ok_or_else(|| CoreError::Invalid("not in a call".into()))?;
        room.muted.store(muted, Ordering::Relaxed);
        let devices: Vec<DeviceId> = lock(&room.peers).values().map(|p| p.device).collect();
        for device in devices {
            let msg = CallMsg::Mute {
                room: room.id,
                muted,
            };
            if let Err(e) = self.call_rpc(device, msg).await {
                debug!(device = %device, error = %e, "mute not sent");
            }
        }
        self.announce_call(&room, self.roster_of(&room));
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    fn current_or_err(&self) -> Result<Arc<Room>> {
        lock(&self.inner.calls.current)
            .clone()
            .ok_or_else(|| CoreError::Invalid("not in a call".into()))
    }

    /// Sets whether this device is deafened. Reported in the call view. Playout is not wired yet.
    pub fn set_deafened(&self, deafened: bool) -> Result<()> {
        let room = self.current_or_err()?;
        room.deafened.store(deafened, Ordering::Relaxed);
        self.announce_call(&room, self.roster_of(&room));
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// The boost ceilings of a call's server: a voice channel's server, level 0 for a DM or group.
    fn room_ceilings(&self, room: &Room) -> crate::boosts::Ceilings {
        match room.scope {
            Scope::Channel { community, .. } => self.call_ceilings(Some(community)),
            _ => self.call_ceilings(None),
        }
    }

    /// Turns this device's camera on or off in the current call. Turning it on starts the capture
    /// thread, and its frames reach the peers from the next tick of the call loop.
    pub fn set_camera(&self, on: bool) -> Result<()> {
        let room = self.current_or_err()?;
        // The call takes the camera over from the lobby preview.
        *lock(&self.inner.calls.lobby_camera) = None;
        {
            let mut camera = lock(&room.camera);
            if !on {
                *camera = None;
            } else if camera.is_none() {
                let mut sending = Sending::new(crate::call_video::start_camera(self.test_capture())?);
                sending.budget.cap(self.room_ceilings(&room).video);
                *camera = Some(sending);
            }
        }
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Previews this device's camera in the lobby, before a call. The window receives the preview
    /// as its own tile. A call's camera takes over from it.
    /// Tells the engine how many pixels wide the tile for a peer's camera or screen is drawn, so
    /// the preview of that tile is made at that width. Takes effect on the next frame.
    /// A width of 0 removes the tile, once it is no longer drawn.
    pub fn set_video_tile(&self, root_id: &str, screen: bool, width: u32) {
        let mut tiles = lock(&self.inner.calls.tiles);
        if width == 0 {
            tiles.remove(&(root_id.to_string(), screen));
        } else {
            tiles.insert((root_id.to_string(), screen), width.min(4096));
        }
    }

    pub fn set_camera_preview(&self, on: bool) -> Result<()> {
        {
            let mut preview = lock(&self.inner.calls.lobby_camera);
            if !on {
                *preview = None;
                return Ok(());
            }
            if preview.is_some() {
                return Ok(());
            }
            *preview = Some(Sending::new(crate::call_video::start_camera(self.test_capture())?));
        }
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(60)).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let engine = Engine { inner };
                let previews: Vec<Preview> = {
                    let slot = lock(&engine.inner.calls.lobby_camera);
                    let Some(sending) = slot.as_ref() else {
                        break;
                    };
                    std::iter::from_fn(|| sending.capture.try_preview()).collect()
                };
                let root_id = engine.inner.identity.root_id().to_string();
                for preview in previews {
                    emit_preview(&engine, 0, &root_id, false, &preview);
                }
            }
        });
        Ok(())
    }

    /// Sends received camera and screen frames to `sink` still encoded, instead of decoding them here. `None` goes back to
    /// the JPEG previews. A window that decodes frames itself uses this, and skips the decode and the JPEG for each frame
    /// the sink takes.
    pub fn set_encoded_video_sink(&self, sink: Option<EncodedSink>) {
        *lock(&self.inner.calls.encoded_sink) = sink;
    }

    /// Asks the peers of the current call for a keyframe, so a viewer that lost its decoder can start again.
    pub fn request_video_keyframes(&self) -> Result<()> {
        let room = self.current_or_err()?;
        room.keyframe_wanted.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Test hook: where this engine's camera and screen take their pictures from. Tests set a pattern or no capture, so
    /// none of them opens a real device. Each engine has its own setting.
    pub fn set_test_capture(&self, source: crate::call_video::TestCapture) {
        *lock(&self.inner.calls.test_capture) = source;
    }

    fn test_capture(&self) -> crate::call_video::TestCapture {
        *lock(&self.inner.calls.test_capture)
    }

    /// Starts or stops sharing a display or a window in the current call. `None` shares the
    /// primary display. Windows and macOS only. Starting a share while one runs replaces it.
    pub fn set_screen_share(
        &self,
        on: bool,
        target: Option<crate::call_video::ShareTarget>,
    ) -> Result<()> {
        let room = self.current_or_err()?;
        {
            let mut screen = lock(&room.screen);
            if !on {
                *screen = None;
            } else {
                let mut sending = Sending::new(crate::call_video::Capture::screen(target, self.test_capture())?);
                sending.set_screen_target(self.inner.calls.screen_kbps.load(Ordering::Relaxed));
                let (height, fps) = *lock(&self.inner.calls.screen_quality);
                sending.set_screen_quality(height, fps);
                if self.inner.calls.screen_sound_on.load(Ordering::Relaxed) {
                    // A share without sound is better than none. `set_screen_audio` already told the sharer if this cannot work.
                    match crate::call_screen_audio::ScreenAudio::start(self.test_capture()) {
                        Ok(sound) => sending.set_sound(Some(sound)),
                        Err(e) => tracing::warn!(error = %e, "the share goes without the computer's sound"),
                    }
                }
                *screen = Some(sending);
            }
        }
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Sets the bitrate of a screen share, in kbps, held to 1500 to 6000. It applies at once to a share
    /// that is running, and to the next one. Returns the bitrate that was set. The link can still hold
    /// the stream below it: see "Screen share" in `docs/design/04-calls.md`.
    pub fn set_screen_bitrate(&self, kbps: u32) -> u32 {
        let kbps = crate::call_video::clamp_screen_kbps(kbps);
        self.inner.calls.screen_kbps.store(kbps, Ordering::Relaxed);
        if let Some(room) = lock(&self.inner.calls.current).clone() {
            if let Some(sending) = lock(&room.screen).as_mut() {
                sending.set_screen_target(kbps);
            }
        }
        kbps
    }

    /// Sets the tallest picture and the highest frame rate of a screen share, for a running share and the
    /// next one. `None` is the source's own size, or no limit on the rate.
    pub fn set_screen_quality(&self, max_height: Option<u32>, max_fps: Option<u32>) {
        *lock(&self.inner.calls.screen_quality) = (max_height, max_fps);
        if let Some(room) = lock(&self.inner.calls.current).clone() {
            if let Some(sending) = lock(&room.screen).as_mut() {
                sending.set_screen_quality(max_height, max_fps);
            }
        }
    }

    /// Turns the computer's sound on or off for a running share and the next one. An error means it cannot
    /// run, and leaves it off.
    pub fn set_screen_audio(&self, on: bool) -> Result<()> {
        let room = lock(&self.inner.calls.current).clone();
        let screen = room.as_ref().map(|room| lock(&room.screen));
        let result = match (on, screen) {
            (false, Some(mut screen)) => {
                if let Some(sending) = screen.as_mut() {
                    sending.set_sound(None);
                }
                Ok(())
            }
            (true, Some(mut screen)) if screen.is_some() => {
                let sound = crate::call_screen_audio::ScreenAudio::start(self.test_capture())?;
                if let Some(sending) = screen.as_mut() {
                    sending.set_sound(Some(sound));
                }
                Ok(())
            }
            // Nothing runs yet, so this checks now rather than failing silently at Go Live.
            (true, _) if self.test_capture() == crate::call_video::TestCapture::Device && !crate::call_screen_audio::screen_audio_available() => {
                Err(CoreError::Invalid("sharing the computer's sound is not available on this system".into()))
            }
            _ => Ok(()),
        };
        self.inner.calls.screen_sound_on.store(on && result.is_ok(), Ordering::Relaxed);
        result
    }

    /// The volume of one sharer's sound on this device, in percent, up to 200.
    pub fn set_screen_volume(&self, root_id: &str, percent: u32) {
        lock(&self.inner.calls.screen_volumes).insert(root_id.to_string(), percent.min(200));
    }

    /// A small JPEG of a display or window, for the share picker.
    pub fn share_thumbnail(&self, target: crate::call_video::ShareTarget) -> Result<Vec<u8>> {
        crate::call_video::share_thumbnail(target, self.test_capture())
    }

    /// For tests: the call loop's timing since the last call, as (ticks, late ticks, worst lateness in ms, audio
    /// frames received), and starts counting again.
    pub(crate) fn take_loop_stats_now(&self) -> Option<(u64, u64, f64, u64)> {
        let room = lock(&self.inner.calls.current).clone()?;
        let [ticks, late, worst, audio] = &room.loop_stats;
        Some((ticks.swap(0, Ordering::Relaxed), late.swap(0, Ordering::Relaxed), worst.swap(0, Ordering::Relaxed) as f64 / 1000.0, audio.swap(0, Ordering::Relaxed)))
    }

    /// For tests: the link's last bandwidth estimate in this call, in kbps.
    pub(crate) fn link_estimate_now(&self) -> Option<u32> {
        let room = lock(&self.inner.calls.current).clone()?;
        Some(room.link_estimate.load(Ordering::Relaxed)).filter(|kbps| *kbps > 0)
    }

    /// For tests: while `on`, this device's video is not stepped down for a busy CPU.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn hold_cpu_governor_now(&self, on: bool) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        room.cpu_held.store(on, Ordering::Relaxed);
        true
    }

    /// For tests: holds this device's estimate of its link at `kbps` when sizing its video, or lets it go with `None`.
    pub(crate) fn force_link_estimate_now(&self, kbps: Option<u32>) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        room.forced_estimate.store(kbps.unwrap_or(0), Ordering::Relaxed);
        true
    }

    /// For tests: starts the estimate of each of this device's mesh links again from `kbps`, as a delay spike leaves it.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn dip_link_estimate_now(&self, kbps: u32) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        room.mesh.lock().await.reset_estimates(kbps);
        true
    }

    /// For tests: the screen encoder's last share of its frame interval.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn screen_cpu_share_now(&self) -> Option<f64> {
        lock(&self.inner.calls.current).clone()?.screen_cpu_share()
    }

    /// For tests: the bitrate this device's screen may reach now.
    pub(crate) fn screen_target_now(&self) -> Option<u32> {
        lock(&self.inner.calls.current).clone()?.screen_target()
    }

    /// For tests: what this device's screen sends now, as (width, height, fps, kbps).
    pub(crate) fn screen_plan_now(&self) -> Option<(u32, u32, u32, u32)> {
        lock(&self.inner.calls.current).clone()?.screen_plan()
    }

    /// Sends text to the call's conversation: the voice channel's messages, or a direct message
    /// to the other person of a DM call.
    pub async fn send_call_message(&self, text: String) -> Result<()> {
        let room = self.current_or_err()?;
        match room.scope {
            Scope::Channel { community, channel } => {
                self.send_message(community, channel, text).await?;
            }
            Scope::Group { conv } => {
                self.send_to_conversation(conv, text).await?;
            }
            Scope::Dm => {
                let peer = room
                    .peer
                    .ok_or_else(|| CoreError::Invalid("this call has no other person".into()))?;
                self.send_dm(peer, text).await?;
            }
        }
        Ok(())
    }

    /// Leaves the call. Participants drop this device's link.
    pub async fn leave_call(&self) -> Result<()> {
        // A migration in progress is abandoned too.
        lock(&self.inner.calls.staged).take();
        let Some(room) = lock(&self.inner.calls.current).take() else {
            return Ok(());
        };
        // Peers drop their link. Devices still ringing stop ringing. A volunteer forwarder drops
        // this device's downlink.
        let mut devices: Vec<DeviceId> = lock(&room.peers).values().map(|p| p.device).collect();
        devices.extend(lock(&room.targets).iter().copied());
        devices.extend(room.forwarding.as_ref().and_then(|f| f.volunteer.map(|(device, _)| device)));
        devices.sort();
        devices.dedup();
        for device in devices {
            if let Err(e) = self.call_rpc(device, CallMsg::Leave { room: room.id }).await {
                debug!(device = %device, error = %e, "leave not sent");
            }
        }
        let sids: Vec<u64> = lock(&room.peers).keys().copied().collect();
        let mut mesh = room.mesh.lock().await;
        room.mesh_by.store(mesh_site::LEAVE, Ordering::Relaxed);
        for sid in sids {
            mesh.remove_peer(sid);
        }
        drop(mesh);
        // An empty roster tells the others this device has left the call.
        self.announce_call(&room, Vec::new());
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// The call on this device, or `None` when it is not in one.
    /// Whether this device is in a call now. Relay probes wait while it is, so they cannot cause an
    /// audio blip (see `boosts`). Tests can pretend with `test_support::pretend_in_call`.
    pub(crate) fn in_call(&self) -> bool {
        lock(&self.inner.calls.current).is_some() || self.inner.pretend_in_call.load(Ordering::Relaxed)
    }

    pub fn call_state(&self) -> Option<CallView> {
        let room = lock(&self.inner.calls.current).clone()?;
        // Read before the peer lock, so `live` is never taken under it.
        let forwarder = room.forwarder_device();
        let paused = room.with_layers(|l| l.paused_here.clone());
        let screens: Vec<u64> = room.with_layers(|l| l.screen_seen.keys().copied().collect());
        let sounding = lock(&room.screen_sound).sounding();
        let reported = self.self_reported_deafen();
        let quality = lock(&room.quality);
        let now = Instant::now();
        let w = self.read();
        let peers = lock(&room.peers);
        let participants: Vec<ParticipantView> = peers
            .iter()
            .map(|(sid, p)| ParticipantView {
                root_id: p.root.to_string(),
                display_name: w.display_name(&p.root),
                muted: p.muted,
                deafened: reported.get(&p.root).copied().unwrap_or(false),
                video: false,
                speaking: !p.muted
                    && p.last_frame
                        .is_some_and(|t| now.duration_since(t) < speaking::SPEAKING_HOLD),
                forwarder: room.forwarding.as_ref().is_some_and(|f| f.forwarder == *sid || f.children.contains(sid)),
                path: room.path_of(&p.device, forwarder.as_ref()),
                video_paused: paused.contains(&(*sid, false)),
                screen_paused: paused.contains(&(*sid, true)),
                screen: screens.contains(sid) || paused.contains(&(*sid, true)),
                screen_audio: sounding.contains(&p.root.to_string()),
            })
            .collect();
        // Two devices in all make a direct link. Three or more make a mesh.
        let topology = if room.tier == Tier::Cascade {
            Topology::Cascade
        } else if room.forwarding.is_some() {
            Topology::Forwarded
        } else if participants.len() <= 1 {
            Topology::P2p
        } else {
            Topology::Mesh
        };
        let relayed = participants.iter().any(|p| p.path == PathView::Relayed);
        let jitter_ms = if peers.is_empty() {
            0.0
        } else {
            peers.values().map(|p| p.jitter_ms).sum::<f64>() / peers.len() as f64
        };
        drop(peers);
        let scope = match room.scope {
            Scope::Dm => CallScope::Dm,
            Scope::Channel { .. } => CallScope::Channel,
            Scope::Group { .. } => CallScope::Group,
        };
        let camera_on = lock(&room.camera).is_some();
        let (channel_name, community_name) = match room.scope {
            Scope::Channel { community, channel } => w
                .communities
                .get(&community)
                .map(|c| (c.state().channels.get(&channel).cloned(), Some(c.name().to_string())))
                .unwrap_or((None, None)),
            _ => (None, None),
        };
        let forwarded_by = room
            .forwarding
            .as_ref()
            .and_then(|f| f.volunteer)
            .map(|(_, root)| w.display_name(&root));
        Some(CallView {
            id: hex(&room.id),
            scope,
            peer: room.peer.map(|p| p.to_string()),
            muted: room.is_muted(),
            deafened: room.deafened.load(Ordering::Relaxed),
            camera: camera_on,
            started_ms: room.started_ms,
            route: route_for(scope, topology, &participants, relayed, forwarded_by.as_deref()),
            participants,
            topology,
            quality: CallQuality {
                rtt_ms: quality.rtt_ms,
                loss_pct: quality.loss_pct,
                jitter_ms,
                flagged: quality.flagged,
            },
            key_epoch: room.epoch.load(Ordering::Relaxed),
            frames_received: quality.frames_received,
            cannot_decrypt: quality.deaf.is_deaf(now),
            forwarded_by,
            self_speaking: room.self_speaking(now),
            channel_name,
            community_name,
        })
    }

    /// Rekeys a channel call when the MLS epoch moved, and drops participants who were banned.
    async fn check_membership(&self, room: &Arc<Room>, mesh: &mut Mesh<LinkTransport>) {
        let Some(group) = room.scope.group_id() else {
            return;
        };
        self.tell_features(room, false);
        if room.forwarding.is_some() {
            self.spawn_signal_layers(room);
        }
        // Moves start only from the current room. A staged room is checked too, but a move started from it would
        // replace it in the staged slot while its own move still waits on it, and leave that move swapping in a room
        // nothing drives.
        let current = lock(&self.inner.calls.current).as_ref().is_some_and(|c| Arc::ptr_eq(c, room));
        if current {
            self.tier_check(room);
            self.forwarder_check(room);
            let mesh_up = mesh.live_count(STALE_LINK);
            self.lonely_check(room, mesh_up).await;
        }
        let tick = room.capacity_clock.fetch_add(1, Ordering::SeqCst);
        if tick.is_multiple_of(CAPACITY_EVERY) {
            // Sent again with each capacity, so a device that missed it, or rejoined, hears it.
            self.tell_features(room, true);
            let mesh_links: Vec<Option<u32>> = if room.forwarding.is_none() {
                mesh.stats().values().map(|s| s.sent_kbps).collect()
            } else {
                Vec::new()
            };
            let measured = self.measure_capacity(room, mesh_links).await;
            let settling = self.settling(room);
            if let Some(kbps) = capacity_to_announce(measured, room.own_kbps.load(Ordering::SeqCst), settling) {
                room.own_kbps.store(kbps, Ordering::SeqCst);
                self.announce_capacity(room, kbps).await;
            }
        }
        if let Ok(indexed) = self.indexed_members_of(group) {
            let me = self.inner.identity.device_id();
            let mut current: BTreeMap<DeviceId, u64> =
                indexed.iter().map(|(sid, d, _)| (*d, *sid)).collect();
            // A seeder that offers to forward is outside the group, so its sender id is kept here.
            current.extend(self.volunteer_leaves(room));
            let previous = std::mem::replace(&mut *lock(&room.leaves), current.clone());
            // A member who joined the group since the last check: verify its digests from now on.
            for (sid, device, cert) in &indexed {
                if *device != me && !previous.contains_key(device) {
                    self.learn_member(room, mesh, *sid, Some(cert.clone())).await;
                }
            }
            // A member who left the group, or was banned: stop verifying it.
            for (device, sid) in previous {
                if !current.contains_key(&device) {
                    self.learn_member(room, mesh, sid, None).await;
                }
            }
        }
        let current = self.with_mls_read(|mls| mls.epoch(&group)).ok();
        if let Some(epoch) = current {
            // A banned device still in the group holds this epoch's key: stay on the last one until it is out.
            if epoch != room.epoch.load(Ordering::Relaxed) && !self.banned_device_in_group(CommunityId(group)) {
                match self.group_key_of(group) {
                    Ok((key, epoch)) => {
                        let mut result = mesh.set_key(key.clone(), epoch).map_err(call_err);
                        if let Some(forwarding) = room.forwarding.as_ref() {
                            if let Some(participant) = forwarding.leaf.lock().await.as_mut() {
                                result = result.and(participant.set_key(key, epoch).map_err(call_err));
                            }
                        }
                        match result {
                            Ok(()) => room.epoch.store(epoch, Ordering::Relaxed),
                            Err(e) => warn!(error = %e, "call rekey failed"),
                        }
                    }
                    Err(e) => warn!(error = %e, "call rekey failed"),
                }
            }
        }
        let banned: Vec<u64> = {
            let w = self.read();
            let state = match room.scope {
                Scope::Channel { community, .. } => w.communities.get(&community).map(|c| c.state()),
                _ => None,
            };
            lock(&room.peers)
                .iter()
                .filter(|(_, p)| state.is_some_and(|s| s.banned.contains(&p.root)))
                .map(|(sid, _)| *sid)
                .collect()
        };
        for sid in banned {
            mesh.remove_peer(sid);
            lock(&room.peers).remove(&sid);
            self.emit(EngineEvent::CallUpdated);
        }
    }

    /// A voice-channel joiner names its own MLS leaf index. This side admits it at once, so the
    /// two can link before this side's group has caught up. The group confirms the same index
    /// later, in `check_membership`.
    async fn admit_leaf(&self, id: RoomId, from: DeviceId, sid: u64, cert: &DeviceCert) {
        let Some(room) = self.current_room(&id) else {
            return;
        };
        if cert.device != from
            || cert.verify().is_err()
            || room.sid_of(&from).is_some()
            || lock(&room.leaves).values().any(|known| *known == sid)
        {
            return;
        }
        lock(&room.leaves).insert(from, sid);
        self.learn_member_locked(&room, sid, Some(cert.clone())).await;
    }

    /// Takes the media lock and starts (`Some`) or stops (`None`) verifying a member's digests.
    async fn learn_member_locked(&self, room: &Arc<Room>, sid: u64, cert: Option<DeviceCert>) {
        let mut mesh = room.mesh.lock().await;
        room.mesh_by.store(mesh_site::LEARN_MEMBER, Ordering::Relaxed);
        self.learn_member(room, &mut mesh, sid, cert).await;
    }

    /// Starts verifying a member's digests (`cert` is `Some`), or stops (`None`), on whichever
    /// link carries this call's media.
    async fn learn_member(
        &self,
        room: &Arc<Room>,
        mesh: &mut Mesh<LinkTransport>,
        sid: u64,
        cert: Option<DeviceCert>,
    ) {
        match room.forwarding.as_ref() {
            None => match cert {
                Some(cert) => {
                    if let Err(e) = mesh.learn_cert(sid, cert) {
                        debug!(error = %e, "member certificate not learned");
                    }
                }
                None => mesh.forget_cert(sid),
            },
            Some(forwarding) => {
                if let Some(participant) = forwarding.leaf.lock().await.as_mut() {
                    match cert {
                        Some(cert) => {
                            if let Err(e) = participant.learn_cert(sid, cert) {
                                debug!(error = %e, "member certificate not learned");
                            }
                        }
                        None => participant.forget_cert(sid),
                    }
                }
            }
        }
    }

    /// Copies the mesh's link measurements into the view's cache.
    fn refresh_quality(&self, room: &Arc<Room>, mesh: &Mesh<LinkTransport>) {
        let stats = mesh.stats();
        let mut rtts = Vec::new();
        let (mut lost, mut sent, mut flagged) = (0u64, 0u64, 0u64);
        for s in stats.values() {
            if let Some(rtt) = s.rtt_ms {
                rtts.push(rtt);
            }
            lost += s.packets_lost;
            flagged += s.impersonation_detected;
            sent += s.packets_sent;
        }
        let mut quality = lock(&room.quality);
        if room.forwarding.is_none() {
            let (undecryptable, received) = stats.values().fold((0, 0), |(u, r), s| (u + s.frames_undecryptable, r + s.frames_received));
            note_deaf(&mut quality, room, undecryptable, received);
        }
        quality.rtt_ms = if rtts.is_empty() {
            None
        } else {
            Some(rtts.iter().sum::<f64>() / rtts.len() as f64)
        };
        quality.flagged = flagged;
        quality.loss_pct = if sent == 0 {
            0.0
        } else {
            lost as f64 * 100.0 / sent as f64
        };
    }
}

/// The devices that hear of a voice channel call in `c`: every current member's but this root's, and a seeder's only
/// with `seeders`. A seeder is never in a call, so it hears of one only to offer to forward it (see
/// `volunteers::seeders_hear_call`).
fn channel_call_devices(w: &crate::world::World, c: &crate::community::Community, seeders: bool) -> Vec<DeviceId> {
    c.state()
        .members
        .iter()
        .filter(|(root, m)| !m.left && **root != w.me && (seeders || !m.storage))
        .flat_map(|(root, _)| w.directory.devices_of(root))
        .filter(|device| *device != w.device)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds the same voiced frames to a voice receiver in `order`, then plays out in time.
    fn play(frames: &[ReceivedFrame], order: &[usize]) -> Vec<Vec<f32>> {
        let mut audio = VoiceAudio::new(SenderConfig::default()).expect("voice audio");
        // Every frame arrives within the same tick, so only the delivery order differs.
        for &i in order {
            audio.incoming(&frames[i], 0);
        }
        (0..frames.len())
            .map(|k| {
                let mut out = vec![0.0f32; FRAME_SAMPLES as usize];
                audio.playout(200 + 20 * k as u64, &mut out);
                out
            })
            .collect()
    }

    #[test]
    fn out_of_order_frames_play_in_sequence_order() {
        let config = SenderConfig {
            noise_suppression: false,
            agc: false,
            ..SenderConfig::default()
        };
        let mut sender = VoiceSender::opus(config).expect("sender");
        let mut voiced = Vec::new();
        for i in 0..30usize {
            let samples: Vec<f32> = (0..FRAME_SAMPLES as usize)
                .map(|n| {
                    let x = ((i * 960 + n) as u32).wrapping_mul(2_654_435_761);
                    0.3 * ((x >> 8) as f32 / (1u32 << 23) as f32 - 1.0)
                })
                .collect();
            voiced.extend(sender.push_capture(&samples).expect("captured").into_iter().filter(|p| p.voiced));
        }
        let frames: Vec<ReceivedFrame> = voiced
            .iter()
            .map(|p| ReceivedFrame {
                sender: 1,
                seq: p.seq,
                timestamp: p.timestamp,
                audio_level: p.audio_level,
                payload: p.to_media_frame(),
            })
            .collect();
        assert!(frames.len() > 10, "enough voiced frames to test");

        let in_order: Vec<usize> = (0..frames.len()).collect();
        // Deterministic shuffle: reverse the halves and swap neighbours.
        let mut shuffled: Vec<usize> = in_order.iter().rev().copied().collect();
        shuffled.chunks_mut(3).for_each(|c| c.reverse());

        let expected = play(&frames, &in_order);
        let got = play(&frames, &shuffled);
        assert!(expected.iter().any(|f| f.iter().any(|x| x.abs() > 0.01)), "audio plays");
        assert_eq!(got, expected, "out-of-order delivery plays in sequence order");
    }
}

/// The tiers a call moves between as its live participant count changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[derive(Serialize, Deserialize)]
pub(crate) enum Tier {
    P2p,
    Mesh,
    Forwarded,
    Cascade,
}

/// Participant counts that move a call between tiers. Each boundary has a gap, so a count that
/// hovers at one edge does not flap the room between two tiers.
pub(crate) struct TierPolicy {
    /// A forwarded call goes up to a cascade at this count, and drops back below `cascade_at - 5`.
    pub cascade_at: usize,
    /// A mesh is forwarded above this count, and a forwarded call goes back to a mesh below it.
    /// Level 0's is 6: forwarded at 7, back at 5. See `effective_mesh_limit`.
    pub mesh_limit: usize,
}

impl TierPolicy {
    /// The tier a call of `n` live participants should run at, given the tier it runs at now.
    pub(crate) fn tier_for(&self, n: usize, current: Tier) -> Tier {
        match current {
            Tier::P2p if n >= 3 => Tier::Mesh,
            Tier::P2p => Tier::P2p,
            Tier::Mesh if n <= 2 => Tier::P2p,
            Tier::Mesh if n > self.mesh_limit => Tier::Forwarded,
            Tier::Mesh => Tier::Mesh,
            Tier::Forwarded if n >= self.cascade_at => Tier::Cascade,
            Tier::Forwarded if n < self.mesh_limit => Tier::Mesh,
            Tier::Forwarded => Tier::Forwarded,
            Tier::Cascade if n < self.cascade_at.saturating_sub(5) => Tier::Forwarded,
            Tier::Cascade => Tier::Cascade,
        }
    }
}

#[cfg(test)]
mod tier_tests {
    use super::{Tier, TierPolicy};

    #[test]
    fn calls_move_up_at_the_thresholds() {
        let p = TierPolicy { cascade_at: 50, mesh_limit: 6 };
        assert_eq!(p.tier_for(2, Tier::P2p), Tier::P2p);
        assert_eq!(p.tier_for(3, Tier::P2p), Tier::Mesh);
        assert_eq!(p.tier_for(6, Tier::Mesh), Tier::Mesh);
        assert_eq!(p.tier_for(7, Tier::Mesh), Tier::Forwarded);
        assert_eq!(p.tier_for(49, Tier::Forwarded), Tier::Forwarded);
        assert_eq!(p.tier_for(50, Tier::Forwarded), Tier::Cascade);
    }

    #[test]
    fn a_call_hovering_at_an_edge_does_not_flap() {
        let p = TierPolicy { cascade_at: 50, mesh_limit: 6 };
        // Forwarded at 6 stays forwarded: going back to mesh needs 5 or fewer.
        assert_eq!(p.tier_for(6, Tier::Forwarded), Tier::Forwarded);
        assert_eq!(p.tier_for(5, Tier::Forwarded), Tier::Mesh);
        // A cascade at 46 stays a cascade: it drops only below 45.
        assert_eq!(p.tier_for(46, Tier::Cascade), Tier::Cascade);
        assert_eq!(p.tier_for(44, Tier::Cascade), Tier::Forwarded);
    }

    #[test]
    fn a_boosted_limit_moves_both_edges() {
        let p = TierPolicy { cascade_at: 50, mesh_limit: 8 };
        assert_eq!(p.tier_for(8, Tier::Mesh), Tier::Mesh);
        assert_eq!(p.tier_for(9, Tier::Mesh), Tier::Forwarded);
        assert_eq!(p.tier_for(8, Tier::Forwarded), Tier::Forwarded);
        assert_eq!(p.tier_for(7, Tier::Forwarded), Tier::Mesh);
    }
}

/// Domain tag for signed tier announcements.
const TIER_DOMAIN: &[u8] = b"dissent/call-tier/v1";

/// A hash of a live roster. Two devices with the same participants get the same hash, whatever
/// order they learned the participants in.
pub(crate) fn roster_hash(sids: &[u64]) -> [u8; 32] {
    let mut sorted = sids.to_vec();
    sorted.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"dissent/call-roster/v1");
    for sid in sorted {
        hasher.update(&sid.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// Announces a tier change for a call. The sender signs it with its device key, so receivers can
/// check who announced it. Receivers apply it only when their own roster hashes to `roster`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TierMsg {
    pub room: RoomId,
    pub tier: Tier,
    /// Sender id of the elected forwarder, for the forwarded tier.
    pub forwarder: Option<u64>,
    pub roster: [u8; 32],
    pub sig: dissent_identity::Signature,
}

fn tier_bytes(tier: Tier, forwarder: Option<u64>, roster: &[u8; 32], room: &RoomId) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(TIER_DOMAIN.len() + 32 + 1 + 9 + 32);
    bytes.extend_from_slice(TIER_DOMAIN);
    bytes.extend_from_slice(room);
    bytes.push(match tier {
        Tier::P2p => 0,
        Tier::Mesh => 1,
        Tier::Forwarded => 2,
        Tier::Cascade => 3,
    });
    match forwarder {
        Some(sid) => {
            bytes.push(1);
            bytes.extend_from_slice(&sid.to_le_bytes());
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(roster);
    bytes
}

impl TierMsg {
    /// Signs a tier announcement with the sender's device key.
    pub(crate) fn sign(
        key: &DeviceKey,
        room: RoomId,
        tier: Tier,
        forwarder: Option<u64>,
        roster: [u8; 32],
    ) -> Self {
        let sig = key.sign(&tier_bytes(tier, forwarder, &roster, &room));
        Self {
            room,
            tier,
            forwarder,
            roster,
            sig,
        }
    }

    /// Checks the signature against the sender's device id.
    pub(crate) fn verify(&self, sender: &DeviceId) -> Result<()> {
        let bytes = tier_bytes(self.tier, self.forwarder, &self.roster, &self.room);
        sender.verify(&bytes, &self.sig).map_err(|_| CoreError::Rejected("bad tier signature".into()))
    }
}

#[cfg(test)]
mod roster_tests {
    use super::*;

    #[test]
    fn roster_hash_ignores_order_and_changes_with_membership() {
        assert_eq!(roster_hash(&[3, 1, 2]), roster_hash(&[1, 2, 3]));
        assert_ne!(roster_hash(&[1, 2, 3]), roster_hash(&[1, 2]));
    }

    #[test]
    fn tier_messages_verify_only_for_their_sender() {
        let key = DeviceKey::generate();
        let room = [7u8; 32];
        let roster = roster_hash(&[1, 2, 3]);
        let msg = TierMsg::sign(&key, room, Tier::Forwarded, Some(2), roster);
        assert!(msg.verify(&key.id()).is_ok());
        let other = DeviceKey::generate();
        assert!(msg.verify(&other.id()).is_err(), "another device cannot sign for it");
        let mut forged = msg.clone();
        forged.tier = Tier::Mesh;
        assert!(forged.verify(&key.id()).is_err(), "a changed tier fails the signature");
    }
}

/// Live participant count at which a forwarded call grows into a cascade.
const CASCADE_AT: usize = 50;
const GEN_DOMAIN: &[u8] = b"dissent/call-gen/v1";

fn tier_tag(tier: Tier) -> u8 {
    match tier {
        Tier::P2p => 0,
        Tier::Mesh => 1,
        Tier::Forwarded => 2,
        Tier::Cascade => 3,
    }
}

/// The media tier a call runs at. A direct pair and a mesh share one mesh. A cascade is not
/// built yet, so it runs as a forwarded call.
fn media_tier(tier: Tier) -> Tier {
    match tier {
        Tier::P2p | Tier::Mesh => Tier::Mesh,
        Tier::Forwarded => Tier::Forwarded,
        Tier::Cascade => Tier::Cascade,
    }
}

/// The generation id of a room migrated to `tier` with this roster. Two devices with the same
/// roster name the same generation, so their offers match.
fn gen_id(base: &RoomId, tier: Tier, roster: &[u8; 32], forwarder: u64) -> RoomId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(GEN_DOMAIN);
    hasher.update(base);
    hasher.update(&[tier_tag(tier)]);
    hasher.update(roster);
    hasher.update(&forwarder.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// One peer's answer to `WhereIs`, as the asker checks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WhereReply {
    pub from: u64,
    pub tier: Tier,
    pub gen: RoomId,
    pub forwarder: Option<u64>,
    pub settled: bool,
}

/// The room a lonely device rejoins, from its peers' answers: the (tier, generation, forwarder) the most settled
/// peers are in, the lowest generation on a tie, so every lonely device picks the same one. One settled answer is
/// enough, as a small call needs. Unsettled answers count for nothing: two lonely devices never adopt each other's
/// room. An answer from a device outside `live`, or naming a forwarder outside it, or a tier other than a mesh or a
/// forwarded call, is ignored. `None` when nothing settled answers, or when the room they are in is `current`: this
/// device is in the right room already, and its links need time, not another move. Leaving `current` out of the count
/// would instead pick the minority, and a device would flap between the two rooms.
pub(crate) fn rejoin_choice(replies: &[WhereReply], live: &[u64], current: &RoomId) -> Option<(Tier, RoomId, Option<u64>)> {
    let mut counts: BTreeMap<(RoomId, u8, Option<u64>), (usize, Tier)> = BTreeMap::new();
    for r in replies {
        let tier_ok = matches!(r.tier, Tier::Mesh) || (r.tier == Tier::Forwarded && r.forwarder.is_some_and(|f| live.contains(&f)));
        if !r.settled || !live.contains(&r.from) || !tier_ok {
            continue;
        }
        counts.entry((r.gen, tier_tag(r.tier), r.forwarder)).or_insert((0, r.tier)).0 += 1;
    }
    // The most answers win; on a tie the first key, which is the lowest generation.
    let best = counts.values().map(|(n, _)| *n).max()?;
    counts
        .into_iter()
        .find(|(_, (n, _))| *n == best)
        .map(|((gen, _, forwarder), (_, tier))| (tier, gen, forwarder))
        .filter(|(_, gen, _)| gen != current)
}

/// How long a dialled media link may wait for its offer before it counts as left over from an earlier offer. An
/// offer follows its link within one request, so this is well above that.
const STALE_INCOMING: Duration = Duration::from_secs(5);

/// How long a mesh call may go without a datagram before a new link from that peer replaces it. Every call carries
/// RTCP and keepalives well within this, so a working one is never taken for stale.
const STALE_LINK: Duration = Duration::from_secs(3);

/// The links a device needs before it swaps in a new tier, of the `expected` ones: a leaf its one link to the
/// forwarder, anyone else a majority.
pub(crate) fn links_needed(expected: usize, role: Option<Role>) -> usize {
    match role {
        Some(Role::Leaf) => expected.min(1),
        _ => (expected / 2 + 1).min(expected),
    }
}

/// What a call's moves into and out of forwarded leave behind, on each device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TierMemory {
    /// The mesh limit in force when this device moved the call to forwarded, or followed that move.
    pub entered_under: Option<usize>,
    /// When this device last moved the call back to a mesh at its limit.
    pub returned_at: Option<Instant>,
    /// How long the way back must hold this time. `None` is the tier sustain.
    pub wait: Option<Duration>,
}

impl TierMemory {
    /// A forwarded call of `live` at limit `mesh_limit` may go back to a mesh at the limit only when it moved to
    /// forwarded under a lower limit: the limit fell, the call did not grow. A call that grew past the limit keeps
    /// the hysteresis and returns only below it.
    pub(crate) fn may_return(&self, live: usize, mesh_limit: usize) -> bool {
        live == mesh_limit && self.entered_under.is_some_and(|under| under < mesh_limit)
    }

    /// Records a move to forwarded under `mesh_limit`. One soon after a return backs the next return off: twice
    /// the last wait, from twice `base`, at most `cap`.
    pub(crate) fn entered(&mut self, mesh_limit: usize, now: Instant, base: Duration, cap: Duration) {
        self.entered_under = Some(mesh_limit);
        if self.returned_at.is_some_and(|at| now.duration_since(at) < base) {
            self.wait = Some(self.wait.map_or(base * 2, |w| w * 2).min(cap));
        }
    }
}

impl Engine {
    /// The tier memory of call `id`, new when the device moved to another call.
    fn tier_memory(&self, id: &RoomId) -> TierMemory {
        match *lock(&self.inner.calls.tier_memory) {
            Some((call, memory)) if call == *id => memory,
            _ => TierMemory::default(),
        }
    }

    fn set_tier_memory(&self, id: RoomId, memory: TierMemory) {
        *lock(&self.inner.calls.tier_memory) = Some((id, memory));
    }

    /// Runs on each driver tick. A channel call migrates once its live roster has wanted another
    /// media tier for `TIER_SUSTAIN`. A forwarded call whose limit has risen back to its size returns to a mesh,
    /// once it has held for its wait (`TierMemory`).
    fn tier_check(&self, room: &Arc<Room>) {
        let live = lock(&room.live).len();
        // A device not heard from yet may be about to say it takes a boosted mesh, so the call holds its tier.
        let Some(mesh_limit) = self.mesh_limit(room) else {
            *lock(&room.tier_since) = None;
            return;
        };
        let policy = TierPolicy {
            cascade_at: self.inner.calls.cascade_at.load(Ordering::SeqCst),
            mesh_limit,
        };
        let mut want = media_tier(policy.tier_for(live, room.tier));
        if want != room.tier && self.settling(room) {
            let uncapped = self.mesh_limit_with(room, false).unwrap_or(mesh_limit);
            if settling_holds(mesh_limit, uncapped, live) {
                *lock(&room.tier_since) = None;
                return;
            }
        }
        let mut memory = self.tier_memory(&room.id);
        let mut sustain = self.inner.timings.tier_sustain;
        let returning = room.tier == Tier::Forwarded && want == Tier::Forwarded && memory.may_return(live, mesh_limit);
        if returning {
            want = Tier::Mesh;
            sustain = memory.wait.unwrap_or(sustain);
        }
        if want == room.tier {
            *lock(&room.tier_since) = None;
            return;
        }
        let since = *lock(&room.tier_since).get_or_insert_with(Instant::now);
        if since.elapsed() >= sustain && !room.migrating.swap(true, Ordering::SeqCst) {
            let now = Instant::now();
            if returning {
                memory.returned_at = Some(now);
                memory.entered_under = None;
            } else if want == Tier::Forwarded {
                let t = &self.inner.timings;
                memory.entered(mesh_limit, now, t.tier_return_backoff, t.tier_return_backoff_cap);
            }
            self.set_tier_memory(room.id, memory);
            let heard: Vec<String> = {
                let now = Instant::now();
                let features = lock(&room.features);
                let seen = lock(&room.seen_live);
                lock(&room.live)
                    .keys()
                    .map(|sid| {
                        let f = features
                            .get(sid)
                            .map(|(bits, at)| format!("bits={bits:#x}/{}ms", now.duration_since(*at).as_millis()))
                            .unwrap_or_else(|| "none".into());
                        let first = seen.get(sid).map(|t| now.duration_since(*t).as_millis());
                        format!("{sid}:{f},seen={first:?}ms")
                    })
                    .collect()
            };
            let caps = self.fresh_capacities(room);
            self.note_tier(format!(
                "moves {:?}->{want:?}: live={live} mesh_limit={mesh_limit} heard=[{}] caps={caps:?}",
                room.tier,
                heard.join(" ")
            ));
            self.spawn_migration(room.clone(), want, None);
        }
    }

    /// Keeps a line on a tier move for `call_debug`, with when it happened, the newest sixteen.
    fn note_tier(&self, line: String) {
        let mut log = lock(&self.inner.calls.tier_log);
        if log.len() == 16 {
            log.pop_front();
        }
        log.push_back((Instant::now(), line));
    }

    fn spawn_migration(&self, old: Arc<Room>, to: Tier, forced: Option<u64>) {
        self.spawn_migration_to(old, to, forced, None);
    }

    /// As `spawn_migration`. With `adopt`, the new room joins that generation, with that forwarder, without telling
    /// anyone: it rejoins a room the others are in already. See `rejoin`.
    fn spawn_migration_to(&self, old: Arc<Room>, to: Tier, forced: Option<u64>, adopt: Option<(RoomId, Option<u64>)>) {
        let engine = self.clone();
        self.spawn_task(async move {
            if let Err(e) = engine.migrate(&old, to, forced, adopt).await {
                warn!(error = %e, "call migration failed; staying on the current tier");
            }
            old.migrating.store(false, Ordering::SeqCst);
        });
    }

    /// Builds the room for `to` beside the current one, links it to the live roster, and swaps it
    /// in once it is linked enough. Until then the old room carries the audio.
    async fn migrate(&self, old: &Arc<Room>, to: Tier, forced: Option<u64>, adopt: Option<(RoomId, Option<u64>)>) -> Result<()> {
        let Some(group) = old.scope.group_id() else {
            return Err(call_err("only channel calls migrate"));
        };
        if !is_current(&self.inner, old) {
            return Ok(());
        }
        let (key, epoch) = self.group_key_of(group)?;
        let indexed = self.indexed_members_of(group)?;
        let live: BTreeMap<u64, DeviceId> = lock(&old.live).clone();
        let sids: Vec<u64> = live.keys().copied().collect();
        let roster = roster_hash(&sids);
        let my_sid = old.my_sid;
        let certs: Vec<(u64, DeviceCert)> = indexed.iter().map(|(s, _, c)| (*s, c.clone())).collect();
        let auth = self.auth_for(&certs);
        let mesh = Mesh::new_authenticated(key.clone(), my_sid, auth.clone()).map_err(call_err)?;
        let mut room = Room::new(old.id, old.scope, mesh, my_sid, epoch, None, &self.inner.calls.rooms_alive);
        room.features = Mutex::new(lock(&old.features).clone());
        room.offers = Mutex::new(lock(&old.offers).clone());
        room.own_kbps = AtomicU32::new(old.own_kbps.load(Ordering::SeqCst));
        // Stall reports carry over, so a device that follows a failover before it judged the
        // volunteer itself still excludes it once the reports add up.
        room.stall = Mutex::new(lock(&old.stall).carried());
        // A tier change elects on the roster and the volunteers' offers alone. Each device sees
        // capacities at its own times, so electing on them here could split the call across two
        // forwarders. An offer reaches every device in the call at once.
        let (forwarder, volunteer) = match (to, adopt) {
            (Tier::Forwarded, Some((_, Some(forwarder)))) => (Some(forwarder), None),
            (Tier::Forwarded, _) => {
                let (forwarder, volunteer) = self.migration_forwarder(old, &sids, forced)?;
                (Some(forwarder), volunteer)
            }
            _ => (None, None),
        };
        room.gen = match adopt {
            Some((gen, _)) => gen,
            None => gen_id(&old.id, to, &roster, forwarder.unwrap_or(0)),
        };
        room.tier = to;
        room.sending = AtomicBool::new(false);
        *lock(&room.live) = live.clone();
        // Everyone live is listed as a participant now, linked or not.
        for (sid, device) in live.iter().filter(|(sid, _)| **sid != my_sid) {
            room.add_peer(*sid, *device, self.root_of_device(device));
        }
        lock(&room.leaves).extend(indexed.iter().map(|(s, d, _)| (*d, *s)));
        lock(&room.leaves).extend(self.volunteer_leaves(old));
        match to {
            Tier::Forwarded => {
                let forwarder = forwarder.unwrap_or(my_sid);
                room.forwarding = Some(self.forwarding_for(&sids, my_sid, key, auth, forwarder, volunteer)?);
            }
            Tier::Cascade => room.forwarding = Some(self.cascade_forwarding(&sids, my_sid, key, auth)?),
            _ => {}
        }
        let room = Arc::new(room);
        *lock(&self.inner.calls.staged) = Some(room.clone());
        spawn_drive(self, &room);

        // Tell the others which tier and roster this comes from, so they can follow.
        let me = self.inner.identity.device_id();
        let forwarder = room.forwarding.as_ref().map(|f| f.forwarder);
        let announce = TierMsg::sign(&self.inner.identity.device, old.id, to, forwarder, roster);
        let others: Vec<DeviceId> = live.values().copied().filter(|d| *d != me).collect();
        // A volunteer forwarder is not live. It is told too, since it builds its relay from this.
        let volunteer = volunteer.map(|(device, _)| device);
        #[cfg(feature = "test-hooks")]
        let reach = lock(&self.inner.calls.tier_announce_reach).take().unwrap_or(usize::MAX);
        #[cfg(not(feature = "test-hooks"))]
        let reach = usize::MAX;
        // A rejoin joins a room the others are in already, so there is nothing to announce.
        let reach = if adopt.is_some() { 0 } else { reach };
        for device in others.iter().chain(volunteer.iter()).take(reach) {
            let engine = self.clone();
            let device = *device;
            let msg = CallMsg::Tier(Box::new(announce.clone()));
            self.spawn_task(async move {
                if let Err(e) = engine.call_rpc(device, msg).await {
                    debug!(device = %device, error = %e, "tier change not delivered");
                }
            });
        }

        if let Err(e) = self.connect_self(&room).await {
            warn!(error = %e, "forwarder's own link not made for the new tier");
        }
        for device in others.iter().chain(volunteer.iter()) {
            // A rejoin linked through `join_devices` above.
            if room.offers_to(device) && adopt.is_none() {
                if let Err(e) = self.connect(room.clone(), *device).await {
                    debug!(device = %device, error = %e, "new-tier link not made yet");
                    self.note_tier(format!("link to {} not made: {e}", &device.to_string()[..8]));
                }
            }
        }

        // A rejoin tells every live peer it is back with a `Join`, so each drops its old call to this device and links
        // again: it offers to the ones this device does not offer to, and accepts this device's offer, which `join_one`
        // makes right after the peer answers. They still count it in the call, and would not otherwise notice it left
        // their room.
        if adopt.is_some() {
            self.join_devices(&room, &others, None).await;
        }

        // A leaf needs one link, to the forwarder. Anyone else needs one to each live peer.
        let expected = match room.forwarding.as_ref() {
            None => others.len(),
            Some(forwarding) => match forwarding.role {
                Role::Leaf => 1,
                Role::Child => 1 + usize::from(forwarding.standby.is_some()) + forwarding.served,
                Role::Root if to == Tier::Cascade => forwarding.children.len(),
                Role::Standby => forwarding.children.len(),
                Role::Root => others.len(),
            },
        };
        // The new tier is swapped in only with enough of those links: a leaf its one, anyone else a majority. With
        // fewer, the peers mostly did not follow, as when they see another roster, and moving anyway would leave this
        // device apart from them. It stays on the old tier instead.
        let needed = links_needed(expected, room.forwarding.as_ref().map(|f| f.role));
        #[cfg(feature = "test-hooks")]
        let needed = if self.inner.calls.swap_without_links.swap(false, Ordering::Relaxed) { 0 } else { needed };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            // Another move took the staged slot. This room's driver has stopped, so it can never carry the call: it
            // is dropped here, which hangs up its links, and the other move goes on.
            if !lock(&self.inner.calls.staged).as_ref().is_some_and(|s| Arc::ptr_eq(s, &room)) {
                self.note_tier(format!("drops {to:?} gen {}: another move took its place", hex(&room.gen[..4])));
                return Err(call_err("another tier move replaced this one"));
            }
            // Links that work, not offers answered: an answered offer whose other end never connects carries nothing.
            let mesh_up = room.mesh.lock().await.live_count(STALE_LINK);
            let linked = match self.links_up(&room, mesh_up).await {
                Some((up, _)) => up,
                None => room.links.load(Ordering::SeqCst),
            };
            if linked >= expected || std::time::Instant::now() >= deadline {
                if linked < needed.max(1) && needed > 0 {
                    clear_staged(&self.inner, &room);
                    let calls: Vec<String> = {
                        let mesh = room.mesh.lock().await;
                        mesh.peers().into_iter().map(|sid| format!("{sid}: {}", mesh.describe_call(sid).unwrap_or_default())).collect()
                    };
                    self.note_tier(format!(
                        "stays {:?} gen {}: {to:?} gen {} linked {linked} of {expected}, needs {needed}; calls [{}]",
                        old.tier,
                        hex(&old.gen[..4]),
                        hex(&room.gen[..4]),
                        calls.join(", ")
                    ));
                    return Err(call_err("too few peers followed the new tier in time"));
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if self.swap_in(old, room) {
            Ok(())
        } else {
            Err(call_err("the call moved on while this tier move waited"))
        }
    }

    /// Makes `new` the current room. The audio moves to it, so the stream keeps its place. Only the staged room is
    /// swapped in, and only over the room it was built from: a room another move replaced in the staged slot has no
    /// loop left, and would hold the call without carrying it. False when nothing was swapped.
    fn swap_in(&self, old: &Arc<Room>, new: Arc<Room>) -> bool {
        {
            let mut current = lock(&self.inner.calls.current);
            let mut staged = lock(&self.inner.calls.staged);
            let ours = staged.as_ref().is_some_and(|s| Arc::ptr_eq(s, &new));
            if !ours || !current.as_ref().is_some_and(|c| Arc::ptr_eq(c, old)) {
                if ours {
                    *staged = None;
                }
                return false;
            }
            // A staged room keeps its loop until it leaves the staged slot, and it is swapped in only from there.
            debug_assert!(new.driven.load(Ordering::SeqCst), "swapped in a room with no call loop");
            let audio = std::mem::replace(&mut *lock(&old.audio), Box::new(SyntheticAudio::default()));
            *lock(&new.audio) = audio;
            new.sending.store(true, Ordering::SeqCst);
            *lock(&new.swapped_at) = Some(Instant::now());
            *current = Some(new.clone());
            *staged = None;
        }
        self.note_tier(format!("swapped in {:?} gen {} for gen {}", new.tier, hex(&new.gen[..4]), hex(&old.gen[..4])));
        // The old room's presence clock stops with it, so the new room keeps the roster re-sent.
        // Without it the call's roster lapses for the server's other members after a migration, and a
        // volunteer stops hearing of the call it forwards.
        self.start_presence_clock(&new);
        self.watch_driver(&new);
        self.emit(EngineEvent::CallUpdated);
        true
    }

    /// Warns once if `room`'s call loop stops ticking for `DRIVER_STALL` while it is the current room: a current room
    /// nothing drives holds the call in silence, with no check left to bring it back. Ends when the room is no
    /// longer current.
    fn watch_driver(&self, room: &Arc<Room>) {
        let (weak, room) = (Arc::downgrade(&self.inner), Arc::downgrade(room));
        self.spawn_task(async move {
            let mut last = (u64::MAX, Instant::now());
            let mut warned = false;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let (Some(inner), Some(room)) = (weak.upgrade(), room.upgrade()) else {
                    return;
                };
                if !lock(&inner.calls.current).as_ref().is_some_and(|c| Arc::ptr_eq(c, &room)) {
                    return;
                }
                let ticks = room.loop_stats[0].load(Ordering::Relaxed);
                if ticks != last.0 {
                    last = (ticks, Instant::now());
                    warned = false;
                } else if !warned && last.1.elapsed() >= DRIVER_STALL {
                    warned = true;
                    let gen = hex(&room.gen[..4]);
                    warn!(gen = %gen, driven = room.driven.load(Ordering::SeqCst), "the current call room stopped running; the call is silent");
                    Engine { inner }.note_tier(format!("call loop of gen {gen} stopped"));
                }
            }
        });
    }

    /// Applies a tier change another device announced, if it describes the roster we see.
    async fn on_tier(&self, from: DeviceId, msg: TierMsg) -> CallReply {
        #[cfg(feature = "test-hooks")]
        if self.inner.calls.refuse_tier_moves.load(Ordering::Relaxed) {
            return CallReply::Refused("the tier change names another roster".into());
        }
        let msg_forwarder = msg.forwarder;
        let Some(room) = self.current_room(&msg.room) else {
            // Not in the call: it may name this device as the call's volunteer forwarder.
            return self.on_volunteer_tier(from, msg).await;
        };
        let live = lock(&room.live).clone();
        let current_forwarder = room.forwarding.as_ref().map(|f| f.forwarder);
        match tier_change_applies(&msg, &from, &live, room.tier, current_forwarder) {
            Ok(to) => {
                // A forwarder outside the roster must be a volunteer this device may link to.
                if let (Tier::Forwarded, Some(f)) = (to, msg_forwarder) {
                    if !live.contains_key(&f) && self.volunteer_offer(&room, f).is_none() {
                        return CallReply::Refused("the named forwarder may not forward this call".into());
                    }
                }
                if !room.migrating.swap(true, Ordering::SeqCst) {
                    self.note_tier(format!("follows {from}'s move {:?}->{to:?}", room.tier));
                    if to == Tier::Forwarded && room.tier != Tier::Forwarded {
                        // The limit this device sees now. A device not heard from yet leaves it at level 0's.
                        let limit = self.mesh_limit(&room).unwrap_or(LEVEL0_MESH_LIMIT);
                        let mut memory = self.tier_memory(&room.id);
                        let t = &self.inner.timings;
                        memory.entered(limit, Instant::now(), t.tier_return_backoff, t.tier_return_backoff_cap);
                        self.set_tier_memory(room.id, memory);
                    }
                    self.spawn_migration(room, to, msg_forwarder);
                }
                CallReply::Ack
            }
            Err(why) => CallReply::Refused(why.into()),
        }
    }
}

/// Decides whether a tier announcement applies to this device. It must be signed by a device in
/// the live roster, and name the roster this device sees. Returns the media tier to move to.
pub(crate) fn tier_change_applies(
    msg: &TierMsg,
    from: &DeviceId,
    live: &BTreeMap<u64, DeviceId>,
    current: Tier,
    current_forwarder: Option<u64>,
) -> std::result::Result<Tier, &'static str> {
    // A device outside the call hears the same as for a call this device is not in.
    if !live.values().any(|device| device == from) {
        return Err("not in this call");
    }
    if msg.verify(from).is_err() {
        return Err("bad tier signature");
    }
    let sids: Vec<u64> = live.keys().copied().collect();
    if roster_hash(&sids) != msg.roster {
        return Err("the tier change names another roster");
    }
    let to = media_tier(msg.tier);
    if to == current && (to != Tier::Forwarded || msg.forwarder == current_forwarder) {
        return Err("already on that tier");
    }
    Ok(to)
}

#[cfg(test)]
mod tier_change_tests {
    use super::*;

    fn roster(devices: &[DeviceId]) -> BTreeMap<u64, DeviceId> {
        devices.iter().map(|d| (device_sid(d), *d)).collect()
    }

    #[test]
    fn a_matching_signed_change_from_a_member_applies() {
        let a = DeviceKey::generate();
        let b = DeviceKey::generate();
        let live = roster(&[a.id(), b.id()]);
        let sids: Vec<u64> = live.keys().copied().collect();
        let msg = TierMsg::sign(&a, [1u8; 32], Tier::Forwarded, None, roster_hash(&sids));
        assert_eq!(tier_change_applies(&msg, &a.id(), &live, Tier::Mesh, None), Ok(Tier::Forwarded));
    }

    #[test]
    fn a_change_naming_another_roster_is_ignored() {
        let a = DeviceKey::generate();
        let b = DeviceKey::generate();
        let live = roster(&[a.id(), b.id()]);
        // The sender saw a different roster, so its hash differs.
        let other = roster_hash(&[1, 2, 3]);
        let msg = TierMsg::sign(&a, [1u8; 32], Tier::Forwarded, None, other);
        assert_eq!(
            tier_change_applies(&msg, &a.id(), &live, Tier::Mesh, None),
            Err("the tier change names another roster")
        );
    }

    #[test]
    fn a_forged_or_unsigned_change_is_ignored() {
        let a = DeviceKey::generate();
        let b = DeviceKey::generate();
        let live = roster(&[a.id(), b.id()]);
        let sids: Vec<u64> = live.keys().copied().collect();
        let msg = TierMsg::sign(&a, [1u8; 32], Tier::Forwarded, None, roster_hash(&sids));
        // Presented as coming from b, who did not sign it.
        assert_eq!(
            tier_change_applies(&msg, &b.id(), &live, Tier::Mesh, None),
            Err("bad tier signature")
        );
        // Signed by a device outside the call, which is not in the roster.
        let outsider = DeviceKey::generate();
        let forged = TierMsg::sign(&outsider, [1u8; 32], Tier::Forwarded, None, roster_hash(&sids));
        assert_eq!(
            tier_change_applies(&forged, &outsider.id(), &live, Tier::Mesh, None),
            Err("not in this call")
        );
    }
}

impl Engine {
    /// The live participant count at which a forwarded call grows into a cascade. Defaults to 50.
    /// Calls take it as they migrate, so set it before the calls you want to affect.
    pub fn set_cascade_at(&self, participants: usize) {
        self.inner
            .calls
            .cascade_at
            .store(participants.max(3), Ordering::SeqCst);
    }
}

impl Engine {
    /// This device's place in the call's forwarding tree: "root", "standby", "child" or "leaf".
    /// `None` when not in a call, or in a call that is not forwarded. For diagnostics and tests.
    pub fn call_role(&self) -> Option<&'static str> {
        let room = lock(&self.inner.calls.current).clone()?;
        let forwarding = room.forwarding.as_ref()?;
        Some(match forwarding.role {
            Role::Root => "root",
            Role::Standby => "standby",
            Role::Child => "child",
            Role::Leaf => "leaf",
        })
    }
}

/// Domain tag for signed capacity announcements.
const CAPACITY_DOMAIN: &[u8] = b"dissent/call-capacity/v1";
/// How long an announced capacity counts before it is ignored.
const CAPACITY_FRESH: Duration = Duration::from_secs(30);
/// Ticks of the membership check between capacity measurements. The check runs about twice a
/// second, so this announces about every five seconds.
const CAPACITY_EVERY: u64 = 10;

fn capacity_bytes(room: &RoomId, kbps: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CAPACITY_DOMAIN.len() + 32 + 4);
    bytes.extend_from_slice(CAPACITY_DOMAIN);
    bytes.extend_from_slice(room);
    bytes.extend_from_slice(&kbps.to_le_bytes());
    bytes
}

impl Engine {
    /// The capacities this device can use: announced ones that are fresh, plus its own.
    fn fresh_capacities(&self, room: &Room) -> BTreeMap<u64, u32> {
        let now = Instant::now();
        let mut caps: BTreeMap<u64, u32> = lock(&room.capacities)
            .iter()
            .filter(|(_, (_, at))| now.duration_since(*at) < CAPACITY_FRESH)
            .map(|(sid, (kbps, _))| (*sid, *kbps))
            .collect();
        let own = room.own_kbps.load(Ordering::SeqCst);
        if own > 0 {
            caps.insert(room.my_sid, own);
        }
        caps
    }

    /// This device's upload in kbps, as a floor: what it sends now, summed over the links that carry
    /// its media (`CallStats::sent_kbps`). A mesh device sums its links to the peers, a forwarder its
    /// downlinks, and a leaf its link to its forwarder. Links with no rate yet add nothing, and with
    /// none at all there is no measurement. It is what the device carries, not what it could: no
    /// probe measures headroom yet. The audio target, 16 to 64 kbps, is a separate value that sizes
    /// the voice only.
    async fn measure_capacity(&self, room: &Arc<Room>, mesh_links: Vec<Option<u32>>) -> Option<u32> {
        let overridden = self.inner.calls.capacity_override.load(Ordering::SeqCst);
        if overridden > 0 {
            return Some(overridden);
        }
        let mut links: Vec<Option<u32>> = Vec::new();
        match room.forwarding.as_ref() {
            None => links.extend(mesh_links),
            Some(forwarding) if forwarding.role == Role::Leaf => {
                if let Some(participant) = forwarding.leaf.lock().await.as_ref() {
                    links.extend(participant.link_stats().values().map(|s| s.sent_kbps));
                }
            }
            Some(forwarding) => {
                if let Some(relay) = forwarding.sfu.lock().await.as_ref() {
                    links.extend(relay.downlink_stats().values().map(|s| s.sent_kbps));
                }
            }
        }
        upload_kbps(&links)
    }

    /// Signs and sends this device's capacity to the other devices in the call.
    async fn announce_capacity(&self, room: &Arc<Room>, kbps: u32) {
        let sig = self.inner.identity.device.sign(&capacity_bytes(&room.id, kbps));
        let me = self.inner.identity.device_id();
        let devices: Vec<DeviceId> = lock(&room.live)
            .values()
            .copied()
            .filter(|d| *d != me)
            .collect();
        for device in devices {
            let engine = self.clone();
            let msg = CallMsg::Capacity {
                room: room.id,
                kbps,
                sig,
            };
            self.spawn_task(async move {
                if let Err(e) = engine.call_rpc(device, msg).await {
                    debug!(device = %device, error = %e, "capacity not delivered");
                }
            });
        }
    }

    /// Records a capacity another device announced. It counts only if the sender is in the call
    /// and signed it.
    fn on_capacity(&self, from: DeviceId, id: RoomId, kbps: u32, sig: dissent_identity::Signature) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        if from.verify(&capacity_bytes(&id, kbps), &sig).is_err() {
            return CallReply::Refused("bad capacity signature".into());
        }
        if !lock(&room.live).values().any(|d| *d == from) {
            return CallReply::Refused("not in this call".into());
        }
        lock(&room.capacities).insert(sender_id_of(&room, &from), (kbps, Instant::now()));
        CallReply::Ack
    }

    /// Re-elects the forwarder when another device has at least 30% more capacity, and has held
    /// that for `REELECT_SUSTAIN`. A forwarded call only, and the new forwarder is chosen by the
    /// same migration path as a tier change.
    fn forwarder_check(&self, room: &Arc<Room>) {
        if room.tier != Tier::Forwarded {
            return;
        }
        let Some(forwarding) = room.forwarding.as_ref() else {
            return;
        };
        if self.volunteer_check(room, forwarding) {
            return;
        }
        let caps = self.fresh_capacities(room);
        let live: Vec<u64> = lock(&room.live).keys().copied().collect();
        let Some(best) = better_forwarder(forwarding.forwarder, &caps, &live) else {
            *lock(&room.forwarder_since) = None;
            return;
        };
        let since = *lock(&room.forwarder_since).get_or_insert_with(Instant::now);
        if since.elapsed() >= self.inner.timings.reelect_sustain && !room.migrating.swap(true, Ordering::SeqCst) {
            *lock(&room.forwarder_since) = None;
            self.spawn_migration(room.clone(), Tier::Forwarded, Some(best));
        }
    }

    /// Test hook: moves this device's call to forwarded (or back to a mesh), announcing it to only `reach` peers, as
    /// when most of them see another roster and refuse it. Returns false when there is no call to move.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn force_tier_move_now(&self, forwarded: bool, reach: usize) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        if room.migrating.swap(true, Ordering::SeqCst) {
            return false;
        }
        *lock(&self.inner.calls.tier_announce_reach) = Some(reach);
        let to = if forwarded { Tier::Forwarded } else { Tier::Mesh };
        self.note_tier(format!("forced {:?}->{to:?}, announced to {reach}", room.tier));
        self.spawn_migration(room, to, None);
        true
    }

    /// Test hook: refuse every tier announcement. See `test_support::refuse_tier_moves`.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn refuse_tier_moves_now(&self, on: bool) {
        self.inner.calls.refuse_tier_moves.store(on, Ordering::Relaxed);
    }

    /// Test hook: starts a second tier move from the current room while another is staged, past the guard that keeps
    /// moves one at a time, as a move started from a staged room's own checks once did. False without a call.
    /// See `test_support::start_second_move`.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn start_second_move_now(&self, forwarded: bool) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        let to = if forwarded { Tier::Forwarded } else { Tier::Mesh };
        let engine = self.clone();
        self.spawn_task(async move {
            if let Err(e) = engine.migrate(&room, to, None, None).await {
                engine.note_tier(format!("second move ended: {e}"));
            }
        });
        true
    }

    /// Test hook: how many times the current room's call loop has ticked. Zero without a call.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn call_loop_ticks_now(&self) -> u64 {
        lock(&self.inner.calls.current).as_ref().map_or(0, |room| room.loop_stats[0].load(Ordering::Relaxed))
    }

    /// Test hook: drops the current mesh's call to `device`, as a link that died. See `test_support::drop_mesh_call`.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn drop_mesh_call_now(&self, device: DeviceId) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        let Some(sid) = room.sid_of(&device) else {
            return false;
        };
        let dropped = room.mesh.lock().await.remove_peer(sid);
        dropped
    }

    /// Test hook: two connects to `device` that cross. The first holds its answer for a second, and the second starts
    /// while the first is still in its handshake and holds its own answer for two. `None` when this device is not the
    /// one that offers to `device`. See `test_support::race_connects`.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn race_connects_now(&self, device: DeviceId) -> Option<(Result<()>, Result<()>)> {
        let room = lock(&self.inner.calls.current).clone()?;
        if room.sid_of(&device)? < room.my_sid() {
            return None;
        }
        lock(&self.inner.calls.answer_holds).extend([Duration::from_secs(1), Duration::from_secs(2)]);
        let first = self.connect(room.clone(), device);
        let second = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            self.connect(room.clone(), device).await
        };
        Some(tokio::join!(first, second))
    }

    /// Test hook: whether the current mesh's call to `device` works: connected, and heard within `STALE_LINK`.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn mesh_call_live_now(&self, device: DeviceId) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        let Some(sid) = room.sid_of(&device) else {
            return false;
        };
        let health = room.mesh.lock().await.call_health(sid);
        health.is_some_and(|(connected, heard)| connected && heard.is_some_and(|h| h < STALE_LINK))
    }

    /// Test hook: tier moves swap in whatever links they made. See `test_support::swap_without_links`.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn swap_without_links_now(&self, on: bool) {
        self.inner.calls.swap_without_links.store(on, Ordering::Relaxed);
    }

    /// The working links this device has in `room` now, and how many it would need for its role: a mesh device's to
    /// each live peer, a leaf's to its forwarder, a forwarder's to each leaf. `None` for a cascade, which is left to its
    /// own failover.
    /// Links count only while they work: connected, with a datagram within `STALE_LINK`. A link that died stays
    /// in its map until a receive fails, which may never happen, so its presence says nothing. `mesh_up` is the
    /// mesh's live calls, counted by the caller under the mesh lock.
    async fn links_up(&self, room: &Room, mesh_up: usize) -> Option<(usize, usize)> {
        let peers = lock(&room.live).len().saturating_sub(1);
        match room.forwarding.as_ref() {
            None => Some((mesh_up, peers)),
            Some(f) if f.role == Role::Leaf => {
                let up = f.leaf.lock().await.as_ref().map_or(0, |p| p.live_links(STALE_LINK).min(1));
                Some((up, 1))
            }
            // The forwarder's own leaf is one of its relay's downlinks.
            Some(f) if f.role == Role::Root && room.tier == Tier::Forwarded => {
                let up = f.sfu.lock().await.as_ref().map_or(0, |s| s.live_downlinks(STALE_LINK).saturating_sub(1));
                Some((up, peers))
            }
            Some(_) => None,
        }
    }

    /// Whether this device is settled in `room`: enough working links for its role, and no move under way.
    async fn settled_in(&self, room: &Room, mesh_up: usize) -> bool {
        if room.migrating.load(Ordering::SeqCst) {
            return false;
        }
        match self.links_up(room, mesh_up).await {
            Some((up, expected)) => up >= links_needed(expected, room.forwarding.as_ref().map(|f| f.role)),
            None => true,
        }
    }

    /// On the membership tick: a device whose room has had too few working links for `Timings::rejoin_after`, with
    /// peers still live, asks them which room they are in and rejoins the one the settled ones are in. That brings back a
    /// mover whose peers refused its tier move, a follower left on a move its mover gave up, and a device split off
    /// from the others for any reason.
    async fn lonely_check(&self, room: &Arc<Room>, mesh_up: usize) {
        let peers = lock(&room.live).len().saturating_sub(1);
        let lonely = peers > 0 && !room.migrating.load(Ordering::SeqCst) && !self.settled_in(room, mesh_up).await;
        let now = Instant::now();
        let after = self.inner.timings.rejoin_after;
        let ask = {
            let mut state = lock(&room.lonely);
            if !lonely {
                state.0 = None;
                return;
            }
            let since = *state.0.get_or_insert(now);
            let due = now.duration_since(since) >= after && state.1.is_none_or(|at| now.duration_since(at) >= after);
            if due {
                state.1 = Some(now);
            }
            due
        };
        if ask {
            let engine = self.clone();
            let room = room.clone();
            self.spawn_task(async move { engine.rejoin(room).await });
        }
    }

    /// Asks every live peer of `room` where it is, and rejoins the room the settled ones are in. A reply is a peer's
    /// word and may be a lie: it is taken only from a live roster member and only when the forwarder it names is one,
    /// and the adopted room is still swapped in only with the links it needs (`links_needed`). So a lying peer costs
    /// one staged room that never swaps in, never a call taken over.
    async fn rejoin(&self, room: Arc<Room>) {
        let live: BTreeMap<u64, DeviceId> = lock(&room.live).clone();
        let me = self.inner.identity.device_id();
        let mut asks = tokio::task::JoinSet::new();
        for (sid, device) in live.iter().filter(|(_, d)| **d != me) {
            let (engine, sid, device, id) = (self.clone(), *sid, *device, room.id);
            asks.spawn(async move {
                match engine.call_rpc(device, CallMsg::WhereIs { room: id }).await {
                    Ok(CallReply::There { tier, gen, forwarder, settled }) => Some(WhereReply { from: sid, tier, gen, forwarder, settled }),
                    Ok(_) => None,
                    // A released device answers it as a bad request.
                    Err(e) => {
                        debug!(device = %device, error = %e, "where-is not answered");
                        None
                    }
                }
            });
        }
        let replies: Vec<WhereReply> = asks.join_all().await.into_iter().flatten().collect();
        let sids: Vec<u64> = live.keys().copied().collect();
        let Some((tier, gen, forwarder)) = rejoin_choice(&replies, &sids, &room.gen).or_else(|| {
            // The settled peers are in this very room, yet this device has had too few working links here for three
            // asks. Waiting longer will not bring them: no link is ever made again on its own. It rebuilds the room on
            // the same generation, which links to each peer afresh.
            let since = lock(&room.lonely).0?;
            let stuck = since.elapsed() >= self.inner.timings.rejoin_after * 3;
            let here = replies.iter().any(|r| r.settled && r.gen == room.gen);
            (stuck && here).then(|| (room.tier, room.gen, room.forwarding.as_ref().map(|f| f.forwarder)))
        }) else {
            return;
        };
        if !is_current(&self.inner, &room) || room.migrating.swap(true, Ordering::SeqCst) {
            return;
        }
        self.note_tier(format!(
            "rejoins {:?} gen {} of {} settled answers of {}",
            tier,
            hex(&gen[..4]),
            replies.iter().filter(|r| r.settled).count(),
            replies.len()
        ));
        let mut memory = self.tier_memory(&room.id);
        if tier == Tier::Forwarded {
            let limit = self.mesh_limit(&room).unwrap_or(LEVEL0_MESH_LIMIT);
            let t = &self.inner.timings;
            memory.entered(limit, Instant::now(), t.tier_return_backoff, t.tier_return_backoff_cap);
        } else {
            memory.entered_under = None;
            memory.wait = None;
        }
        self.set_tier_memory(room.id, memory);
        self.spawn_migration_to(room, tier, None, Some((gen, forwarder)));
    }

    /// Answers a peer's `WhereIs` with the room this device is in for that call, at most once per half
    /// `Timings::rejoin_after` for each device.
    async fn on_where_is(&self, from: DeviceId, id: RoomId) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        if !lock(&room.live).values().any(|d| *d == from) {
            return CallReply::Refused("not in this call".into());
        }
        let now = Instant::now();
        {
            let mut answered = lock(&self.inner.calls.where_answered);
            if answered.get(&from).is_some_and(|at| now.duration_since(*at) < self.inner.timings.rejoin_after / 2) {
                return CallReply::Refused("asked too often".into());
            }
            answered.retain(|_, at| now.duration_since(*at) < self.inner.timings.rejoin_after);
            answered.insert(from, now);
        }
        let mesh_up = room.mesh.lock().await.live_count(STALE_LINK);
        let settled = self.settled_in(&room, mesh_up).await;
        CallReply::There {
            tier: room.tier,
            gen: room.gen,
            forwarder: room.forwarding.as_ref().map(|f| f.forwarder),
            settled,
        }
    }

    /// Test hook: holds back this device's `Features` for `hold`, as a joiner whose first one is slow to arrive.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn hold_call_features_now(&self, hold: Duration) {
        *lock(&self.inner.calls.features_held_until) = Some(Instant::now() + hold);
    }

    /// Test hook: overrides this device's measured capacity, in kbps. `None` measures again.
    pub fn set_capacity_override(&self, kbps: Option<u32>) {
        self.inner
            .calls
            .capacity_override
            .store(kbps.unwrap_or(0), Ordering::SeqCst);
    }
}

/// The sender id of a device in a room, or zero if it is not a participant.
fn sender_id_of(room: &Room, device: &DeviceId) -> u64 {
    room.sid_of(device).unwrap_or(0)
}

/// The device a forwarded call should move to, if any. A candidate must have at least 30% more
/// capacity than the current forwarder. A tie goes to the lower sender id.
pub(crate) fn better_forwarder(current: u64, caps: &BTreeMap<u64, u32>, live: &[u64]) -> Option<u64> {
    let &current_kbps = caps.get(&current)?;
    let (best_kbps, std::cmp::Reverse(best)) = live
        .iter()
        .filter_map(|sid| caps.get(sid).map(|kbps| (*kbps, std::cmp::Reverse(*sid))))
        .max()?;
    (best != current && u64::from(best_kbps) * 10 >= u64::from(current_kbps) * 13).then_some(best)
}

#[cfg(test)]
mod reelect_tests {
    use super::better_forwarder;
    use std::collections::BTreeMap;

    #[test]
    fn a_call_is_deaf_only_while_frames_arrive_and_none_opens() {
        use super::DeafWatch;
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let later = |secs| start + Duration::from_secs(secs);
        let mut watch = DeafWatch::default();
        assert!(!watch.note(0, 0, start), "nothing yet");
        assert!(!watch.note(10, 0, start));
        assert!(!watch.is_deaf(later(4)));
        assert!(watch.note(20, 0, later(6)), "the first time it warns");
        assert!(watch.is_deaf(later(6)));
        assert!(!watch.note(30, 0, later(7)), "and only once");
        assert!(!watch.note(30, 1, later(8)), "one frame that opens ends it");
        assert!(!watch.is_deaf(later(8)));
        // A quiet call is not deaf: nothing failing means nothing to say.
        let mut quiet = DeafWatch::default();
        assert!(!quiet.note(0, 50, start));
        assert!(!quiet.note(0, 50, later(60)));
        assert!(!quiet.is_deaf(later(60)));
    }

    #[test]
    fn a_candidate_needs_thirty_percent_more_capacity() {
        let caps: BTreeMap<u64, u32> = [(1, 1000), (2, 1250), (3, 1300)].into();
        // 1250 is only 25% above 1000, so it does not take over. 1300 is 30% above, and does.
        assert_eq!(better_forwarder(1, &caps, &[1, 2]), None);
        assert_eq!(better_forwarder(1, &caps, &[1, 2, 3]), Some(3));
    }

    #[test]
    fn ties_go_to_the_lower_id_and_the_current_forwarder_stays() {
        let caps: BTreeMap<u64, u32> = [(1, 1000), (2, 5000), (3, 5000)].into();
        assert_eq!(better_forwarder(1, &caps, &[1, 2, 3]), Some(2));
        // Already the best: nothing to do.
        assert_eq!(better_forwarder(2, &caps, &[1, 2, 3]), None);
    }
}

/// The room id of a group's call.
fn group_room_id(conv: &ConvId) -> RoomId {
    room_id(&[&b"group"[..], &conv.0[..]])
}

impl Engine {
    /// The MLS key and epoch of a group, for media. A group call's key comes from its MLS group,
    /// as a channel's does.
    /// What `test_support::call_key_state` reads.
    /// The call loop's step and how long it has been there, and whether the mesh lock is free, from atomics and
    /// try-locks only, so a watchdog can read it while a thread holding a lock is stuck.
    pub(crate) fn call_breadcrumbs(&self) -> String {
        let room = match self.inner.calls.current.try_lock() {
            Ok(current) => current.clone(),
            Err(_) => return "calls.current is locked".into(),
        };
        let Some(room) = room else {
            return "no call".into();
        };
        let step = room.step.load(Ordering::Relaxed);
        let since = now_ms().saturating_sub(room.step_at.load(Ordering::Relaxed));
        let mesh = match room.mesh.try_lock() {
            Ok(_) => "free".to_string(),
            Err(_) => format!(
                "held, last taken by {}",
                mesh_site::NAMES.get(room.mesh_by.load(Ordering::Relaxed) as usize).copied().unwrap_or("?")
            ),
        };
        format!(
            "step={} for {since} ms, ticks={}, mesh lock {mesh}",
            step::NAMES.get(step as usize).copied().unwrap_or("?"),
            room.loop_stats[0].load(Ordering::Relaxed),
        )
    }

    /// Where this device's call loop stands, for a test that timed out: ticks run, speaking
    /// checks and announcements, what the window last heard of, and each link's frame counters.
    pub(crate) fn call_debug(&self) -> String {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return "no call".into();
        };
        let now = Instant::now();
        let (checks, emits, shown, sent_ago) = {
            let s = lock(&room.speaking);
            (s.checks, s.emits, s.shown_len(), s.last_sent.map(|t| now.duration_since(t).as_millis()))
        };
        let peers: Vec<String> = lock(&room.peers)
            .iter()
            .map(|(sid, p)| format!("{sid}:muted={} last_frame_ago_ms={:?}", p.muted, p.last_frame.map(|t| now.duration_since(t).as_millis())))
            .collect();
        // The pump holds the mesh lock for a moment at a time; a few tries get between its turns.
        let mut tries = 0;
        let guard = loop {
            match room.mesh.try_lock() {
                Ok(mesh) => break Ok(mesh),
                Err(e) if tries == 50 => break Err(e),
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        let links = match guard {
            Ok(mesh) => format!(
                "connected={:?} drops={:?} calls=[{}] ",
                mesh.connected(),
                mesh.drops(),
                mesh.peers().into_iter().map(|sid| format!("{sid}: {}", mesh.describe_call(sid).unwrap_or_default())).collect::<Vec<_>>().join(", ")
            )
                + &mesh
                .stats()
                .iter()
                .map(|(sid, s)| {
                    format!(
                        "{sid}:sent={} recv={} undecryptable={} replayed={} unauth={} impersonation={} retired={} sender_capped={} id_capped={} pkts_sent={} pkts_recv={}",
                        s.frames_sent,
                        s.frames_received,
                        s.frames_undecryptable,
                        s.frames_replayed,
                        s.frames_unauthenticated,
                        s.impersonation_detected,
                        s.frames_retired_session,
                        s.frames_sender_capped,
                        s.frames_id_capped,
                        s.packets_sent,
                        s.packets_received,
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
            Err(_) => "mesh locked".into(),
        };
        // Copied out under one guard. A `lock` in each format argument would hold the first guard while taking the
        // next, and deadlock.
        let sends = lock(&room.sends).clone();
        let tiers: Vec<String> = lock(&self.inner.calls.tier_log)
            .iter()
            .map(|(at, line)| format!("-{:.1}s {line}", now.duration_since(*at).as_secs_f32()))
            .collect();
        format!(
            "rooms_alive={} staged={:?} gen={} send_ok={} send_err={:?} empty_ticks={} not_sending_ticks={} ticks={} forwarded={} sending={} muted={} speaking_checks={checks} speaking_emits={emits} shown={shown} last_sent_ago_ms={sent_ago:?} peers=[{}] links=[{links}] receivers={} tiers=[{}]",
            self.inner.calls.rooms_alive.load(Ordering::Relaxed),
            lock(&self.inner.calls.staged).as_ref().map(|r| hex(&r.gen[..4])),
            hex(&room.gen[..4]),
            sends.sent,
            sends.failed,
            sends.empty,
            sends.not_sending,
            room.loop_stats[0].load(Ordering::Relaxed),
            room.forwarding.is_some(),
            room.sending.load(Ordering::SeqCst),
            room.is_muted(),
            peers.join(", "),
            self.inner.events.receiver_count(),
            tiers.join(" | "),
        )
    }

    pub(crate) fn call_key_state(&self) -> Option<([u8; 4], u64, u64)> {
        let room = lock(&self.inner.calls.current).clone()?;
        let (key, epoch) = self.group_key_of(room.scope.group_id()?).ok()?;
        Some((key.fingerprint(), epoch, room.my_sid))
    }

    fn group_key_of(&self, group: [u8; 32]) -> Result<(MediaKey, u64)> {
        let (epoch, secret) = self
            .with_mls_read(|mls| -> dissent_mls::Result<(u64, Vec<u8>)> {
                Ok((
                    mls.epoch(&group)?,
                    mls.export_secret(&group, KEY_LABEL, 32)?,
                ))
            })
            .map_err(call_err)?;
        let bytes: [u8; 32] = secret
            .try_into()
            .map_err(|_| call_err("exported key is not 32 bytes"))?;
        Ok((MediaKey::new(bytes), epoch))
    }

    /// The members of a group's MLS group, with their leaf indices as sender ids.
    fn indexed_members_of(&self, group: [u8; 32]) -> Result<Vec<(u64, DeviceId, DeviceCert)>> {
        let members = self
            .with_mls_read(|mls| mls.indexed_members(&group))
            .map_err(call_err)?;
        Ok(members
            .into_iter()
            .map(|(index, cert)| (u64::from(index), cert.device, cert))
            .collect())
    }

    /// Builds this device's room for a group call, installs it, and links its own speech. The room
    /// starts as a mesh and moves by its live roster, as a channel call does.
    async fn open_group_room(&self, scope: Scope, id: RoomId) -> Result<Arc<Room>> {
        let group = scope
            .group_id()
            .ok_or_else(|| call_err("this scope has no group"))?;
        let (key, epoch) = self.group_key_of(group)?;
        let indexed = self.indexed_members_of(group)?;
        let my_device = self.inner.identity.device_id();
        let my_sid = indexed
            .iter()
            .find(|(_, d, _)| *d == my_device)
            .map(|(sid, _, _)| *sid)
            .ok_or_else(|| CoreError::Invalid("this device is not in the group".into()))?;
        let certs: Vec<(u64, DeviceCert)> = indexed.iter().map(|(s, _, c)| (*s, c.clone())).collect();
        let mesh = Mesh::new_authenticated(key, my_sid, self.auth_for(&certs)).map_err(call_err)?;
        let room = Room::new(id, scope, mesh, my_sid, epoch, None, &self.inner.calls.rooms_alive);
        lock(&room.live).insert(my_sid, my_device);
        lock(&room.leaves).extend(indexed.iter().map(|(s, d, _)| (*d, *s)));
        let room = Arc::new(room);
        self.install(room.clone());
        if let Err(e) = self.connect_self(&room).await {
            warn!(error = %e, "group call: own link not made");
        }
        Ok(room)
    }

    /// The devices of a group's other members. Only a member of the group has them.
    fn group_devices(&self, conv: ConvId) -> Result<Vec<DeviceId>> {
        let w = self.read();
        let meta = w
            .conv_book
            .convs
            .get(&conv)
            .ok_or_else(|| CoreError::NotFound("no such group".into()))?;
        if meta.left || !meta.members.contains(&self.inner.me) {
            return Err(CoreError::Invalid("you are not a member of this group".into()));
        }
        let me = self.inner.identity.device_id();
        Ok(meta
            .members
            .iter()
            .flat_map(|root| w.directory.devices_of(root))
            .filter(|d| *d != me)
            .collect())
    }

    /// Joins a group call, starting it if nobody is in it. Members who are not in the call ring
    /// only when nobody is in it yet, so a late joiner does not ring the group.
    pub async fn join_group_call(&self, conv: ConvId) -> Result<()> {
        let id = group_room_id(&conv);
        if self.check_free(id)?.is_some() {
            return Ok(());
        }
        let devices = self.group_devices(conv)?;
        let room = self.open_group_room(Scope::Group { conv }, id).await?;
        // First pass: join whoever is already in the call, without ringing anyone.
        if self.join_devices_ring(&room, &devices, None, false).await == 0 {
            // Nobody is in it, so this call starts now: ring the other members.
            self.join_devices_ring(&room, &devices, None, true).await;
        }
        self.share_call(&room);
        self.end_if_unanswered(room.id);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Rings for a group call someone started. It is answered with `accept_call`.
    fn ring_group(&self, id: RoomId, from: DeviceId, cert: DeviceCert, conv: ConvId) -> CallReply {
        {
            let mut ringing = lock(&self.inner.calls.ringing);
            if let Some(r) = ringing.as_ref() {
                return if r.id == id {
                    CallReply::Ringing
                } else {
                    CallReply::Refused("busy with another incoming call".into())
                };
            }
            *ringing = Some(Ringing {
                id,
                from,
                peer: cert.root,
                cert: cert.clone(),
                scope: Scope::Group { conv },
                key: None,
            });
        }
        self.emit(EngineEvent::IncomingCall {
            call_id: hex(&id),
            from: cert.root.to_string(),
            scope: CallScope::Group,
            conv: Some(conv.to_string()),
        });
        let timeout = *lock(&self.inner.calls.ring_timeout);
        let engine = self.clone();
        self.spawn_task(async move {
            tokio::time::sleep(timeout).await;
            if let Some(r) = engine.take_ringing(&id) {
                engine.emit(EngineEvent::CallMissed {
                    call_id: hex(&id),
                    from: r.peer.to_string(),
                });
                if let Err(e) = engine.call_rpc(from, CallMsg::Decline { room: id }).await {
                    debug!(device = %from, error = %e, "missed group call not reported");
                }
            }
        });
        CallReply::Ringing
    }
}

/// A DM or group call another device said is live, shaped as the conversation list reads it.
/// `participants` and `not_in` leave out this device's own person.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LiveCallView {
    pub started_ms: u64,
    /// The display name of the person whose device started the call.
    pub started_by: String,
    /// Everyone in the conversation, this device's person included.
    pub total: usize,
    pub route: LiveRoute,
    pub participants: Vec<LiveParticipant>,
    /// Members of the group who are not in the call. Empty for a direct call.
    pub not_in: Vec<LiveMember>,
    /// `dm` for a direct call. Absent for a group call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<&'static str>,
}

/// How the call's media is routed. A mesh call is `direct`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LiveRoute {
    pub kind: &'static str,
}

/// A person in a live call. Speaking is only known on the device in the call, so it is false here.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LiveParticipant {
    pub root_id: String,
    pub display_name: String,
    pub muted: bool,
    pub speaking: bool,
}

/// A group member who is not in the live call.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LiveMember {
    pub root_id: String,
    pub display_name: String,
}

impl Engine {
    /// The live call of a group, if another member's device said one is running.
    pub fn live_call_of_group(&self, conv: ConvId) -> Option<LiveCallView> {
        let (members, left) = {
            let w = self.read();
            let meta = w.conv_book.convs.get(&conv)?;
            (meta.members.clone(), meta.left)
        };
        if left {
            return None;
        }
        self.live_view(|h| h.scope == (Scope::Group { conv }), Some(&members), false)
    }

    /// Rosters heard recently enough to trust. A device that stops re-sending drops out.
    fn fresh_heard(&self) -> Vec<Heard> {
        let now = now_ms();
        lock(&self.inner.calls.heard)
            .values()
            .filter(|h| now.saturating_sub(h.seen_ms) < millis(self.inner.timings.call_presence_ttl))
            .cloned()
            .collect()
    }

    /// The peers whose devices said a direct call with us is live.
    pub(crate) fn live_dm_peers(&self) -> Vec<RootId> {
        let mut peers: Vec<RootId> = self
            .fresh_heard()
            .into_iter()
            .filter(|h| h.scope == Scope::Dm)
            .map(|h| h.from)
            .collect();
        peers.sort();
        peers.dedup();
        peers
    }

    /// The live direct call with `peer`, if the peer's device said one is running.
    pub fn live_call_of_dm(&self, peer: RootId) -> Option<LiveCallView> {
        self.live_view(|h| h.scope == Scope::Dm && h.from == peer, None, true)
    }

    /// Builds the view of the earliest call that `want` matches. Several devices may report the
    /// same room, so their rosters are merged. `group` is the group's members, or `None` for a DM.
    fn live_view(
        &self,
        want: impl Fn(&Heard) -> bool,
        group: Option<&[RootId]>,
        dm: bool,
    ) -> Option<LiveCallView> {
        let mut calls: Vec<Heard> = self.fresh_heard().into_iter().filter(|h| want(h)).collect();
        calls.sort_by_key(|h| h.started_ms);
        let first = calls.first()?.clone();
        // Each person counts only from their own device's report, as in a voice channel. In a group,
        // only the group's members count.
        let mut roster: Vec<PresenceMember> = Vec::new();
        for member in calls.iter().filter(|h| h.room == first.room).filter_map(Heard::own_entry) {
            let in_group = group.is_none_or(|g| g.contains(&member.root));
            if in_group && !roster.iter().any(|m| m.root == member.root) {
                roster.push(*member);
            }
        }
        let me = self.inner.me;
        let w = self.read();
        let participants = roster
            .iter()
            .filter(|m| m.root != me)
            .map(|m| LiveParticipant {
                root_id: m.root.to_string(),
                display_name: w.display_name(&m.root),
                muted: m.muted,
                speaking: false,
            })
            .collect();
        let not_in = group
            .unwrap_or(&[])
            .iter()
            .filter(|r| **r != me && !roster.iter().any(|m| m.root == **r))
            .map(|r| LiveMember {
                root_id: r.to_string(),
                display_name: w.display_name(r),
            })
            .collect();
        Some(LiveCallView {
            started_ms: first.started_ms,
            started_by: w.display_name(&first.from),
            total: group.map_or(2, <[RootId]>::len),
            route: LiveRoute { kind: "direct" },
            participants,
            not_in,
            scope: dm.then_some("dm"),
        })
    }
}

impl Engine {
    /// For tests: this device's measured upload in its current call, in kbps, before any override.
    /// `None` before the first measurement.
    pub(crate) fn own_call_capacity(&self) -> Option<u32> {
        let room = lock(&self.inner.calls.current).clone()?;
        Some(room.own_kbps.load(Ordering::SeqCst)).filter(|k| *k > 0)
    }

    /// For tests: the layers this device's camera or screen was asked for in its current call.
    pub(crate) fn wanted_layers_now(&self, screen: bool) -> Option<u8> {
        lock(&self.inner.calls.current).clone()?.wanted_layers(screen)
    }

    /// For tests: the election over measured capacities alone, with no defaults. Returns how many
    /// live devices have a fresh measurement, and the forwarder it picks among them for a call of
    /// the live roster's size.
    pub(crate) fn measured_election(&self) -> (usize, Option<u64>) {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return (0, None);
        };
        let caps = self.fresh_capacities(&room);
        let sids: Vec<u64> = caps.keys().copied().collect();
        let participants = lock(&room.live).len();
        (sids.len(), elect_forwarder(&sids, participants, &caps))
    }
}

/// A device's upload from its links' send rates: their sum, ignoring links with no rate yet. `None`
/// when no link has one.
fn upload_kbps(links: &[Option<u32>]) -> Option<u32> {
    let measured: Vec<u64> = links.iter().flatten().map(|k| u64::from(*k)).collect();
    if measured.is_empty() {
        return None;
    }
    Some(u32::try_from(measured.iter().sum::<u64>()).unwrap_or(u32::MAX))
}

/// The mesh limit of boost level 0. A released device uses it too: it forwards at 7 and goes back
/// to a mesh at 5.
const LEVEL0_MESH_LIMIT: usize = 6;

/// What a live device has said about boosted tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoostHeard {
    /// A fresh `Features` with `FEATURE_BOOST_TIERS`.
    Supports,
    /// `Features` without it, gone stale, or none within `Timings::unheard_grace`: a released client.
    Released,
    /// Nothing yet, and within the grace since it was first seen live.
    NotYet,
}

/// The mesh limit a call uses. A boosted limit above level 0's holds only while every live device
/// has said it supports one (`FEATURE_BOOST_TIERS`), and every live device's announced upload covers
/// a copy of its stream for each other device, `copy_kbps` each. Otherwise it is level 0's, which
/// released devices use, so they never see a mesh larger than they expect. A device with no fresh
/// capacity counts as the election's default. `None` while a device has not been heard from yet: a
/// joiner's first `Features` can take seconds on a loaded machine, and counting it released meanwhile
/// would move a boosted mesh to forwarded, where it then stays at the limit.
/// Whether a room still settling after its swap keeps its tier: when only capacity lowers the limit (`limit` below
/// `uncapped`, the limit without capacities) and the call has not outgrown the uncapped one. A room that just swapped
/// in measures this device's upload on links still ramping, so a move then would follow a dip, not the call.
fn settling_holds(limit: usize, uncapped: usize, live: usize) -> bool {
    limit < uncapped && live <= uncapped
}

/// The capacity to announce for this device: what was measured, or while the room is settling after its swap, the
/// value it carried over from the room before, so peers do not lower their limit on a dip. `None` announces nothing.
fn capacity_to_announce(measured: Option<u32>, carried: u32, settling: bool) -> Option<u32> {
    if settling {
        (carried > 0).then_some(carried)
    } else {
        measured
    }
}

fn effective_mesh_limit(
    boosted: usize,
    live: &[u64],
    heard: impl Fn(u64) -> BoostHeard,
    capacities: &BTreeMap<u64, u32>,
    copy_kbps: u32,
) -> Option<usize> {
    if boosted <= LEVEL0_MESH_LIMIT {
        return Some(boosted);
    }
    let heard: Vec<BoostHeard> = live.iter().map(|sid| heard(*sid)).collect();
    if heard.contains(&BoostHeard::NotYet) {
        return None;
    }
    if heard.contains(&BoostHeard::Released) {
        return Some(LEVEL0_MESH_LIMIT);
    }
    let copies = u32::try_from(live.len().saturating_sub(1)).unwrap_or(u32::MAX);
    let need = copies.saturating_mul(copy_kbps);
    if live
        .iter()
        .any(|sid| capacities.get(sid).copied().unwrap_or(DEFAULT_KBPS) < need)
    {
        return Some(LEVEL0_MESH_LIMIT);
    }
    Some(boosted)
}

impl Engine {
    /// The mesh limit for `room` now: its server's boosted one when every device can take it. `None` while
    /// a live device has not been heard from yet. See `effective_mesh_limit`.
    fn mesh_limit(&self, room: &Room) -> Option<usize> {
        self.mesh_limit_with(room, true)
    }

    /// Whether `room` was swapped in less than `Timings::settle_after_swap` ago.
    fn settling(&self, room: &Room) -> bool {
        lock(&room.swapped_at).is_some_and(|at| at.elapsed() < self.inner.timings.settle_after_swap)
    }

    /// As `mesh_limit`. Without `capacities`, every device counts as able to carry the mesh, which gives the limit
    /// the server's boost and the devices' clients allow.
    fn mesh_limit_with(&self, room: &Room, capacities: bool) -> Option<usize> {
        if self.inner.legacy_calls.load(Ordering::Relaxed) {
            return Some(LEVEL0_MESH_LIMIT);
        }
        let boosted = self.room_ceilings(room).mesh_limit;
        // Each copy carries audio, and a camera or screen when anyone in the call shows one.
        let video = lock(&room.camera).is_some()
            || lock(&room.screen).is_some()
            || lock(&room.remote_video).receiving();
        let copy_kbps = AUDIO_STREAM_KBPS + if video { VIDEO_720P_KBPS } else { 0 };
        let live: Vec<u64> = lock(&room.live).keys().copied().collect();
        let capacities = if capacities { self.fresh_capacities(room) } else { BTreeMap::new() };
        let now = Instant::now();
        let features = lock(&room.features).clone();
        let seen = {
            let mut seen = lock(&room.seen_live);
            seen.retain(|sid, _| live.contains(sid));
            for sid in &live {
                seen.entry(*sid).or_insert(now);
            }
            seen.clone()
        };
        let grace = self.inner.timings.unheard_grace;
        let heard = |sid: u64| match features.get(&sid) {
            _ if sid == room.my_sid => BoostHeard::Supports,
            Some((bits, at)) if bits & FEATURE_BOOST_TIERS != 0 && now.duration_since(*at) < CAPACITY_FRESH => {
                BoostHeard::Supports
            }
            Some(_) => BoostHeard::Released,
            None if seen.get(&sid).is_some_and(|first| now.duration_since(*first) < grace) => BoostHeard::NotYet,
            None => BoostHeard::Released,
        };
        effective_mesh_limit(boosted, &live, heard, &capacities, copy_kbps)
    }

    /// Sends this device's `Features` to the live devices it has not told yet, or to all of them
    /// with `again`. A device that refuses it, because it has not seen us join yet, is told again on
    /// the next tick. A released device answers it as a bad request, and is asked again only with
    /// the next `again`.
    fn tell_features(&self, room: &Arc<Room>, again: bool) {
        if self.inner.legacy_calls.load(Ordering::Relaxed) {
            return;
        }
        #[cfg(feature = "test-hooks")]
        if lock(&self.inner.calls.features_held_until).is_some_and(|until| Instant::now() < until) {
            return;
        }
        let me = self.inner.identity.device_id();
        // A volunteer forwarder is not live, but it reads the call's features for the layers.
        let volunteer = room
            .forwarding
            .as_ref()
            .and_then(|f| f.volunteer.map(|(device, _)| (f.forwarder, device)));
        let targets: Vec<(u64, DeviceId)> = {
            let live = lock(&room.live);
            let mut told = lock(&room.told);
            live.iter()
                .map(|(sid, device)| (*sid, *device))
                .chain(volunteer)
                .filter(|(_, device)| *device != me)
                .filter(|(sid, _)| told.insert(*sid) || again)
                .collect()
        };
        for (sid, device) in targets {
            let engine = self.clone();
            let weak = Arc::downgrade(room);
            let msg = CallMsg::Features {
                room: room.id,
                bits: match self.inner.call_features_override.load(Ordering::Relaxed) {
                    0 => FEATURES,
                    bits => bits,
                },
            };
            self.spawn_task(async move {
                match engine.call_rpc(device, msg).await {
                    Ok(CallReply::Refused(_)) => {
                        if let Some(room) = weak.upgrade() {
                            lock(&room.told).remove(&sid);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => debug!(device = %device, error = %e, "call features not delivered"),
                }
            });
        }
    }

    /// Records the `Features` a device in the call sent.
    fn on_features(&self, from: DeviceId, id: RoomId, bits: u64) -> CallReply {
        let Some(room) = self.current_room(&id).or_else(|| self.relay_room(&id)) else {
            return CallReply::Refused("not in this call".into());
        };
        if !lock(&room.live).values().any(|d| *d == from) {
            return CallReply::Refused("not in this call".into());
        }
        lock(&room.features).insert(sender_id_of(&room, &from), (bits, Instant::now()));
        CallReply::Ack
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    #[test]
    fn a_settling_room_holds_against_a_limit_only_capacity_lowered() {
        assert!(settling_holds(6, 8, 7), "seven fit the boosted limit; only a capacity dip says six");
        assert!(!settling_holds(8, 8, 9), "nine outgrew the limit: real growth moves");
        assert!(!settling_holds(6, 8, 9), "past the uncapped limit it moves whatever the capacities say");
        assert!(!settling_holds(6, 6, 7), "a limit the clients or the boost set is not capacity's doing");
    }

    #[test]
    fn a_settling_room_announces_the_capacity_it_carried_over() {
        assert_eq!(capacity_to_announce(Some(90), 400, true), Some(400), "the dip is not announced");
        assert_eq!(capacity_to_announce(Some(90), 0, true), None, "nothing carried over, nothing announced");
        assert_eq!(capacity_to_announce(Some(90), 400, false), Some(90), "once settled, the measure counts");
        assert_eq!(capacity_to_announce(None, 400, false), None);
    }
}

#[cfg(test)]
mod rejoin_tests {
    use super::*;

    fn reply(from: u64, tier: Tier, gen: u8, forwarder: Option<u64>, settled: bool) -> WhereReply {
        WhereReply { from, tier, gen: [gen; 32], forwarder, settled }
    }

    #[test]
    fn one_settled_answer_is_enough_and_unsettled_ones_count_for_nothing() {
        let live = [0, 1, 2, 3];
        let replies = [reply(1, Tier::Mesh, 7, None, true), reply(2, Tier::Forwarded, 3, Some(2), false)];
        assert_eq!(rejoin_choice(&replies, &live, &[9; 32]), Some((Tier::Mesh, [7; 32], None)));
        // Two lonely devices: neither is settled, so nobody moves.
        let lonely = [reply(1, Tier::Forwarded, 3, Some(1), false), reply(2, Tier::Forwarded, 4, Some(2), false)];
        assert_eq!(rejoin_choice(&lonely, &live, &[9; 32]), None);
    }

    #[test]
    fn the_most_settled_answers_win_and_a_tie_goes_to_the_lowest_generation() {
        let live = [0, 1, 2, 3, 4];
        let split = [reply(1, Tier::Mesh, 8, None, true), reply(2, Tier::Mesh, 8, None, true), reply(3, Tier::Mesh, 5, None, true)];
        assert_eq!(rejoin_choice(&split, &live, &[9; 32]).map(|c| c.1), Some([8; 32]), "two against one");
        let tie = [reply(1, Tier::Mesh, 8, None, true), reply(2, Tier::Mesh, 5, None, true)];
        assert_eq!(rejoin_choice(&tie, &live, &[9; 32]).map(|c| c.1), Some([5; 32]), "the lowest generation");
    }

    #[test]
    fn answers_outside_the_roster_and_the_current_room_are_ignored() {
        let live = [0, 1, 2];
        let replies = [
            reply(7, Tier::Mesh, 1, None, true),              // not live here
            reply(1, Tier::Forwarded, 2, Some(7), true),      // a forwarder outside the roster
            reply(2, Tier::Cascade, 3, Some(2), true),        // not a tier a device rejoins
        ];
        assert_eq!(rejoin_choice(&replies, &live, &[9; 32]), None);
    }

    /// The room this device is in already counts like any other: when most settled peers are in it, the device stays,
    /// even though a minority is in another room. Leaving it out would pick the minority and flap between the two.
    #[test]
    fn a_device_already_in_the_majoritys_room_stays() {
        let live = [0, 1, 2, 3, 4];
        let replies = [
            reply(1, Tier::Mesh, 9, None, true),
            reply(2, Tier::Mesh, 9, None, true),
            reply(3, Tier::Mesh, 9, None, true),
            reply(4, Tier::Forwarded, 3, Some(4), true),
        ];
        assert_eq!(rejoin_choice(&replies, &live, &[9; 32]), None, "this device is where the majority is");
        assert_eq!(rejoin_choice(&replies, &live, &[3; 32]).map(|c| c.1), Some([9; 32]), "from the minority's room it goes");
    }
}

#[cfg(test)]
mod links_needed_tests {
    use super::*;

    #[test]
    fn a_leaf_needs_its_link_and_anyone_else_a_majority() {
        assert_eq!(links_needed(1, Some(Role::Leaf)), 1);
        for (expected, majority) in [(0, 0), (1, 1), (2, 2), (3, 2), (5, 3), (6, 4), (7, 4)] {
            assert_eq!(links_needed(expected, None), majority, "a mesh device with {expected} peers");
            assert_eq!(links_needed(expected, Some(Role::Root)), majority, "a forwarder with {expected} leaves");
        }
    }
}

#[cfg(test)]
mod tier_return_tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(30);
    const CAP: Duration = Duration::from_secs(300);

    #[test]
    fn a_call_moved_under_a_lower_limit_may_return_at_the_limit() {
        let mut m = TierMemory::default();
        m.entered(6, Instant::now(), BASE, CAP);
        assert!(m.may_return(8, 8), "the limit rose back to 8 with 8 in the call");
        assert!(!m.may_return(8, 6), "not while the limit is still 6");
        assert!(!m.may_return(9, 8), "not above the limit");
        assert!(!m.may_return(7, 8), "below the limit the usual way back applies");
    }

    #[test]
    fn a_call_that_grew_past_the_limit_keeps_the_hysteresis() {
        let mut m = TierMemory::default();
        m.entered(8, Instant::now(), BASE, CAP);
        assert!(!m.may_return(8, 8), "it moved because it grew, so 8 of 8 stays forwarded");
        assert!(!TierMemory::default().may_return(8, 8), "nor without a recorded move");
    }

    #[test]
    fn falling_back_soon_after_a_return_doubles_the_wait_up_to_the_cap() {
        let now = Instant::now();
        let mut m = TierMemory::default();
        m.entered(6, now, BASE, CAP);
        assert_eq!(m.wait, None, "the first return waits the tier sustain");
        let mut waits = Vec::new();
        for _ in 0..5 {
            m.returned_at = Some(now);
            m.entered(6, now + Duration::from_secs(1), BASE, CAP);
            waits.push(m.wait.unwrap().as_secs());
        }
        assert_eq!(waits, vec![60, 120, 240, 300, 300]);
        // A fall long after the last return does not back off further.
        m.returned_at = Some(now);
        m.entered(6, now + BASE * 2, BASE, CAP);
        assert_eq!(m.wait, Some(CAP));
    }
}

#[cfg(test)]
mod mesh_limit_tests {
    use super::*;

    fn caps(entries: &[(u64, u32)]) -> BTreeMap<u64, u32> {
        entries.iter().copied().collect()
    }

    fn all(_: u64) -> BoostHeard {
        BoostHeard::Supports
    }

    #[test]
    fn level_zero_limits_need_nothing() {
        let live = [1, 2, 3, 4, 5, 6, 7];
        assert_eq!(effective_mesh_limit(6, &live, |_| BoostHeard::Released, &caps(&[(1, 1)]), 32), Some(6));
        assert_eq!(effective_mesh_limit(6, &live, |_| BoostHeard::NotYet, &caps(&[(1, 1)]), 32), Some(6));
    }

    #[test]
    fn a_boosted_limit_holds_when_every_device_supports_it() {
        let live = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(effective_mesh_limit(8, &live, all, &BTreeMap::new(), 32), Some(8));
    }

    #[test]
    fn one_released_device_brings_back_level_zero() {
        let live = [1, 2, 3, 4, 5, 6, 7];
        let heard = |sid| if sid == 4 { BoostHeard::Released } else { BoostHeard::Supports };
        assert_eq!(effective_mesh_limit(10, &live, heard, &BTreeMap::new(), 32), Some(6));
    }

    #[test]
    fn an_upload_short_of_the_copies_keeps_the_mesh_small() {
        let live = [1, 2, 3, 4, 5, 6, 7];
        // Six copies of a 720p camera need over 9 Mbps. Device 3 has 5.
        let low = caps(&[(3, 5_000)]);
        let copy = AUDIO_STREAM_KBPS + VIDEO_720P_KBPS;
        assert_eq!(effective_mesh_limit(8, &live, all, &low, copy), Some(6));
        // The same upload covers six copies of audio.
        assert_eq!(effective_mesh_limit(8, &live, all, &low, AUDIO_STREAM_KBPS), Some(8));
    }

    /// A device not heard from yet is neither for nor against the boosted limit: there is no limit to decide by,
    /// and the call keeps its tier. A released device among them still decides nothing until the rest are heard.
    #[test]
    fn a_device_not_heard_from_yet_leaves_the_limit_undecided() {
        let live = [1, 2, 3, 4, 5, 6, 7, 8];
        let heard = |sid| if sid == 8 { BoostHeard::NotYet } else { BoostHeard::Supports };
        assert_eq!(effective_mesh_limit(8, &live, heard, &BTreeMap::new(), 32), None);
        let mixed = |sid| match sid {
            8 => BoostHeard::NotYet,
            4 => BoostHeard::Released,
            _ => BoostHeard::Supports,
        };
        assert_eq!(effective_mesh_limit(8, &live, mixed, &BTreeMap::new(), 32), None);
    }

    /// The tier a boosted call of seven and of eight wants: a mesh while the eighth device is not heard from yet,
    /// since the call holds its tier; forwarded once the grace is over and it counts as released.
    #[test]
    fn a_boosted_mesh_holds_while_a_joiner_is_unheard_and_forwards_once_it_counts_released() {
        for n in [7usize, 8] {
            let live: Vec<u64> = (1..=n as u64).collect();
            let unheard = |sid| if sid == n as u64 { BoostHeard::NotYet } else { BoostHeard::Supports };
            let released = |sid| if sid == n as u64 { BoostHeard::Released } else { BoostHeard::Supports };
            let tier = |limit: Option<usize>| match limit {
                None => Tier::Mesh,
                Some(mesh_limit) => TierPolicy { cascade_at: 50, mesh_limit }.tier_for(n, Tier::Mesh),
            };
            assert_eq!(tier(effective_mesh_limit(8, &live, unheard, &BTreeMap::new(), 32)), Tier::Mesh, "{n} unheard");
            assert_eq!(
                tier(effective_mesh_limit(8, &live, released, &BTreeMap::new(), 32)),
                Tier::Forwarded,
                "{n} with one released"
            );
        }
    }
}

#[cfg(test)]
mod features_compat_tests {
    use super::*;

    /// `CallMsg` as released devices have it: the same variants, in order, without `Features`.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    enum ReleasedCallMsg {
        Join(Box<JoinMsg>),
        Offer { room: RoomId, gen: RoomId, signal: Vec<u8> },
        Mute { room: RoomId, muted: bool },
        Decline { room: RoomId },
        Tier(Box<TierMsg>),
        Capacity { room: RoomId, kbps: u32, sig: dissent_identity::Signature },
        AnsweredElsewhere { room: RoomId },
        Leave { room: RoomId },
        Presence(Box<PresenceMsg>),
    }

    #[test]
    fn a_released_device_cannot_decode_features_but_still_decodes_the_rest() {
        let features = postcard::to_stdvec(&CallMsg::Features { room: [1; 32], bits: FEATURES }).unwrap();
        assert!(postcard::from_bytes::<ReleasedCallMsg>(&features).is_err());
        let mute = postcard::to_stdvec(&CallMsg::Mute { room: [1; 32], muted: true }).unwrap();
        assert!(matches!(
            postcard::from_bytes::<ReleasedCallMsg>(&mute),
            Ok(ReleasedCallMsg::Mute { muted: true, .. })
        ));
    }
}

#[cfg(test)]
mod roster_trust_tests {
    use super::*;
    use crate::membership::tests::Node;

    fn member(root: RootId) -> PresenceMember {
        PresenceMember { root, muted: false, deafened: false }
    }

    #[test]
    fn only_the_senders_own_entry_is_first_hand() {
        let (bob, mallory) = (RootId([2; 32]), RootId([3; 32]));
        let heard = Heard {
            room: [0; 32],
            scope: Scope::Dm,
            started_ms: 0,
            from: bob,
            members: vec![member(mallory), member(bob)],
            seen_ms: 0,
        };
        assert_eq!(heard.own_entry().map(|m| m.root), Some(bob));
        let empty = Heard { members: vec![member(mallory)], ..heard };
        assert!(empty.own_entry().is_none(), "a roster that leaves out its sender vouches for nobody");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_relayed_entry_does_not_put_anyone_in_a_voice_channel() {
        crate::testutil::bounded(async {
            let alice = Node::named("alice").await;
            let community: CommunityId = alice.engine.create_community("Hall".into()).await.expect("create").id.parse().unwrap();
            let channel: ChannelId = alice
                .engine
                .create_voice_channel(community, "Lounge".into())
                .await
                .expect("voice channel")
                .id
                .parse()
                .unwrap();
            // A roster that lists alice's own root beside its non-member sender: neither shows. The sender is not a
            // member, and alice is never listed from someone else's report.
            let stranger = RootId([7; 32]);
            lock(&alice.engine.inner.calls.heard).insert(
                DeviceKey::generate().id(),
                Heard {
                    room: [1; 32],
                    scope: Scope::Channel { community, channel },
                    started_ms: now_ms(),
                    from: stranger,
                    members: vec![member(stranger), member(RootId([8; 32]))],
                    seen_ms: now_ms(),
                },
            );
            let views = alice.engine.voice_channel_views(community);
            let lounge = views.iter().find(|v| v.id == channel.to_string()).expect("the lounge");
            assert!(lounge.participants.is_empty(), "{:?}", lounge.participants);
            assert_eq!(alice.engine.voice_headcount(community, channel), 0, "a fake roster cannot fill a channel");
        })
        .await;
    }
}
