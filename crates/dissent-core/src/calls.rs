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

use crate::engine::{now_ms, Engine, Inner};
use crate::error::{CoreError, Result};
use crate::rpc::{decode, Request, Response};
use crate::views::EngineEvent;
use dissent_events::{ChannelId, Payload};
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId};
use dissent_audio::{OpusEncoder, SenderConfig, SenderPacket, VoiceReceiver, VoiceSender};
use dissent_media::{
    elect, Auth, Candidate, LinkTransport, MediaKey, Mesh, Mode, Participant, ReceivedFrame, Sfu,
    Side, Signal, FRAME_SAMPLES,
};
use std::collections::VecDeque;
use dissent_net::{MediaHandler, MediaLink, PathKind, PeerAddr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Mutex as AsyncMutex};
use tracing::{debug, warn};

/// Identifies a call. Both sides compute it the same way.
pub(crate) type RoomId = [u8; 32];

/// The RFC 9605 export label for the SFrame base key.
const KEY_LABEL: &str = "SFrame 1.0 Base Key";
/// One synthetic audio frame every 20 ms, 160 bytes.
const FRAME_MS: u64 = 20;
const FRAME_BYTES: usize = 160;
/// How long a participant counts as speaking after its last frame.
const SPEAKING_WINDOW: Duration = Duration::from_millis(300);
/// How long the accepting side waits for the media link that the offering side dials.
const LINK_WAIT: Duration = Duration::from_secs(10);
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
    pub from: PeerAddr,
}

/// Signalling messages, sent over the device RPC.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum CallMsg {
    /// The sender joins the room. Answered with `CallReply::Here` by a device in the room.
    Join(Box<JoinMsg>),
    /// The lower-id side's offer for one pair of devices. Answered with `CallReply::Answer`.
    Offer { room: RoomId, signal: Vec<u8> },
    /// The sender's mute state.
    Mute { room: RoomId, muted: bool },
    /// The caller gave up before the call was answered, or the callee declined it.
    Decline { room: RoomId },
    /// The sender has left the room and drops its link to us.
    Leave { room: RoomId },
}

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

    fn capture(&mut self, samples: &[f32]) -> Result<()> {
        let packets = self.sender.push_capture(samples).map_err(call_err)?;
        self.pending
            .extend(packets.into_iter().filter(|p| p.voiced));
        Ok(())
    }
}

/// Calls with more members than this route media through a forwarder. Up to this many, each pair
/// links directly, which is a mesh.
pub(crate) const MESH_MAX: usize = 6;

/// The forwarded tier. One elected device relays audio between the others. Every other device
/// links to it, and it links to none of them directly.
pub(crate) struct Forwarding {
    /// Sender id of the elected forwarder.
    forwarder: u64,
    /// On the forwarder: relays between the leaves.
    sfu: AsyncMutex<Option<Sfu<LinkTransport>>>,
    /// On a leaf: its link to the forwarder.
    leaf: AsyncMutex<Option<Participant<LinkTransport>>>,
}

/// A candidate forwarder. Capacity is assumed, because devices do not report their upload yet.
fn candidate(peer: u64) -> Candidate {
    Candidate {
        peer,
        upload_kbps: 10_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 50,
    }
}

/// The forwarder for a call of `participants` devices, chosen by dissent-media's election. Every
/// member computes the same answer from the same sender ids.
fn elect_forwarder(sids: &[u64], participants: usize) -> Option<u64> {
    let candidates: Vec<Candidate> = sids.iter().map(|s| candidate(*s)).collect();
    elect(&candidates, participants, Mode::AudioTop3).map(|e| e.primary)
}

/// One remote participant of a call.
struct Peer {
    device: DeviceId,
    root: RootId,
    direct: bool,
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
}

/// How long an incoming or outgoing direct call rings before it counts as missed.
pub const RING_TIMEOUT: Duration = Duration::from_secs(45);

/// An incoming direct call the user has not answered yet. No media exists until they accept.
pub(crate) struct Ringing {
    id: RoomId,
    from: DeviceId,
    peer: RootId,
    cert: DeviceCert,
    key: [u8; 32],
}

/// Calls on this device, the media links the transport has delivered, and any call ringing.
pub(crate) struct CallHub {
    current: Mutex<Option<Arc<Room>>>,
    incoming: Mutex<BTreeMap<DeviceId, MediaLink>>,
    arrived: Notify,
    ringing: Mutex<Option<Ringing>>,
    ring_timeout: Mutex<Duration>,
    /// Whether new calls use the voice pipeline instead of synthetic frames.
    voice: AtomicBool,
    /// Capture settings for the voice pipeline. Defaults include noise suppression and AGC.
    sender_config: Mutex<SenderConfig>,
}

impl Default for CallHub {
    fn default() -> Self {
        Self {
            current: Mutex::new(None),
            incoming: Mutex::new(BTreeMap::new()),
            arrived: Notify::new(),
            ringing: Mutex::new(None),
            ring_timeout: Mutex::new(RING_TIMEOUT),
            voice: AtomicBool::new(false),
            sender_config: Mutex::new(SenderConfig::default()),
        }
    }
}

/// Receives the media links that other devices dial. Each one waits in the hub until the call
/// that expects it takes it.
pub(crate) struct MediaSink(Weak<Inner>);

impl MediaHandler for MediaSink {
    fn incoming(&self, from: DeviceId, link: MediaLink) {
        if let Some(inner) = self.0.upgrade() {
            lock(&inner.calls.incoming).insert(from, link);
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
    /// Video is not built yet, so this is always false.
    pub video: bool,
    pub speaking: bool,
    /// True when this device relays for others. Not used yet, so always false.
    pub forwarder: bool,
    pub path: PathView,
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
}

fn room_id(parts: &[&[u8]]) -> RoomId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ROOM_DOMAIN);
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn call_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("call failed: {e}"))
}

/// The route sentence for the call panel. The engine writes it, the UI shows it verbatim.
fn route_for(topology: Topology, participants: &[ParticipantView], relayed: bool) -> CallRoute {
    let via = if relayed {
        " Some links go through a relay."
    } else {
        ""
    };
    match (topology, participants.first()) {
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
        (Topology::Forwarded, _) => CallRoute {
            kind: "forwarded".into(),
            title: "Forwarded call".into(),
            detail: format!(
                "A forwarder relays audio between the {} people in this call.{via}",
                participants.len() + 1
            ),
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

impl Room {
    fn new(
        id: RoomId,
        scope: Scope,
        mesh: Mesh<LinkTransport>,
        my_sid: u64,
        epoch: u64,
        peer: Option<RootId>,
    ) -> Self {
        Self {
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
        }
    }

    fn add_peer(&self, sid: u64, device: DeviceId, root: RootId, direct: bool) {
        lock(&self.peers).insert(
            sid,
            Peer {
                device,
                root,
                direct,
                muted: false,
                last_frame: None,
                last_arrival: None,
                jitter_ms: 0.0,
            },
        );
    }

    /// Records one decrypted frame from `sid` and updates that peer's jitter.
    fn note_frame(&self, sid: u64, now: Instant) {
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

    /// The sender id of `device` in this call, or `None` if it is not a participant. A direct call
    /// has two: the caller is 0 and the callee is 1. A channel call uses the MLS leaf index.
    fn sid_of(&self, device: &DeviceId) -> Option<u64> {
        match self.scope {
            Scope::Dm => Some(1 - self.my_sid),
            Scope::Channel { .. } => lock(&self.leaves).get(device).copied(),
        }
    }

    /// Whether this device makes the offer for its link to `device`. In a mesh the lower id
    /// offers. In a forwarded call each leaf offers to the forwarder, and no two leaves link.
    fn offers_to(&self, device: &DeviceId) -> bool {
        match &self.forwarding {
            Some(f) => f.forwarder != self.my_sid && self.sid_of(device) == Some(f.forwarder),
            None => self.sid_of(device).is_some_and(|sid| self.my_sid < sid),
        }
    }

    fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }
}

/// True if `room` is still the call this device is in.
fn is_current(inner: &Inner, room: &Arc<Room>) -> bool {
    lock(&inner.calls.current)
        .as_ref()
        .is_some_and(|c| Arc::ptr_eq(c, room))
}

/// Forwarded calls. The forwarder relays between the leaves. A leaf receives from its forwarder
/// and sends through it. A forwarder does not send its own audio yet, since it has no leaf link.
async fn pump_forwarded(room: &Room, forwarding: &Forwarding) {
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
    while let Ok(Ok(Some(frame))) =
        tokio::time::timeout(Duration::from_millis(1), participant.next_received_frame()).await
    {
        room.note_frame(frame.sender, Instant::now());
        lock(&room.quality).frames_received += 1;
        lock(&room.audio).incoming(&frame, now_ms());
    }
    let mut outgoing = Vec::new();
    lock(&room.audio).outgoing(&mut outgoing);
    if !room.is_muted() {
        for (timestamp, bytes) in outgoing {
            // The first byte of a frame is its audio level, which the forwarder ranks speakers by.
            let level = bytes.first().copied().unwrap_or(127);
            if let Err(e) = participant.send_audio_frame_at(&bytes, timestamp, level).await {
                debug!(error = %e, "call frame not sent");
            }
        }
    }
}

/// Runs one call: sends a synthetic frame every 20 ms, drains arriving frames, and every half
/// second rekeys a channel call and drops banned participants. Stops when the call ends.
async fn drive(weak: Weak<Inner>, room: Weak<Room>) {
    let mut tick = tokio::time::interval(Duration::from_millis(FRAME_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut since_check: u32 = 0;
    loop {
        tick.tick().await;
        let (Some(inner), Some(room)) = (weak.upgrade(), room.upgrade()) else {
            break;
        };
        if !is_current(&inner, &room) {
            break;
        }
        let engine = Engine { inner };
        let mut mesh = room.mesh.lock().await;
        since_check += 1;
        if since_check >= MEMBERSHIP_EVERY {
            since_check = 0;
            engine.check_membership(&room, &mut mesh).await;
            engine.refresh_quality(&room, &mesh);
        }
        if let Some(forwarding) = room.forwarding.as_ref() {
            drop(mesh);
            pump_forwarded(&room, forwarding).await;
            continue;
        }
        while let Ok(Ok(Some(frame))) =
            tokio::time::timeout(Duration::from_millis(1), mesh.next_received_frame()).await
        {
            room.note_frame(frame.sender, Instant::now());
            lock(&room.quality).frames_received += 1;
            lock(&room.audio).incoming(&frame, now_ms());
        }
        let mut outgoing = Vec::new();
        lock(&room.audio).outgoing(&mut outgoing);
        if !room.is_muted() {
            for (timestamp, bytes) in outgoing {
                if let Err(e) = mesh.send_audio_frame_at(&bytes, timestamp).await {
                    debug!(error = %e, "call frame not sent");
                }
            }
        }
    }
}

impl Engine {
    /// The forwarded-tier state for this device. The elected forwarder gets a relay. Every other
    /// device gets a leaf link to it.
    fn forwarding_for(
        &self,
        sids: &[u64],
        my_sid: u64,
        key: MediaKey,
        auth: Auth,
    ) -> Result<Forwarding> {
        let forwarder = elect_forwarder(sids, sids.len())
            .ok_or_else(|| call_err("no device can forward this call"))?;
        let (sfu, leaf) = if forwarder == my_sid {
            (Some(Sfu::new(Mode::AudioTop3)), None)
        } else {
            let participant = Participant::new_authenticated(
                key,
                my_sid,
                vec![candidate(forwarder)],
                sids.len(),
                Mode::AudioTop3,
                auth,
            )
            .map_err(call_err)?;
            (None, Some(participant))
        };
        Ok(Forwarding {
            forwarder,
            sfu: AsyncMutex::new(sfu),
            leaf: AsyncMutex::new(leaf),
        })
    }

    /// Leaf side of a forwarded call: links to the forwarder, and sends and receives through it.
    async fn connect_leaf(&self, room: Arc<Room>, device: DeviceId) -> Result<()> {
        let Some(forwarding) = room.forwarding.as_ref() else {
            return Err(call_err("this call is not forwarded"));
        };
        let sid = forwarding.forwarder;
        let link = self.node().open_media(device).await?;
        let direct = path_is_direct(&link);
        let transport = LinkTransport::new(link, Side::Caller);
        let offer = {
            let mut leaf = forwarding.leaf.lock().await;
            let participant = leaf
                .as_mut()
                .ok_or_else(|| call_err("this device is the forwarder"))?;
            participant.offer_sfu(sid, transport).await.map_err(call_err)?
        };
        let reply = self
            .call_rpc(
                device,
                CallMsg::Offer {
                    room: room.id,
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
                room.add_peer(sid, device, self.root_of_device(&device), direct);
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
            let config = *lock(&self.inner.calls.sender_config);
            match VoiceAudio::new(config) {
                Ok(voice) => *lock(&room.audio) = Box::new(voice),
                Err(e) => warn!(error = %e, "voice audio unavailable; this call is silent"),
            }
        }
        *lock(&self.inner.calls.current) = Some(room.clone());
        self.spawn_task(drive(Arc::downgrade(&self.inner), Arc::downgrade(&room)));
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
            }
            _ => out.fill(0.0),
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
        Ok(crate::views::ChannelInfo {
            id: ChannelId(se.event.id.0).to_string(),
            name,
            voice: true,
        })
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
            w.member_devices(c)
                .into_iter()
                .filter(|d| *d != self.inner.identity.device_id())
                .collect::<Vec<DeviceId>>()
        };
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
        let mut room = Room::new(room_id, scope, mesh, my_sid, epoch, None);
        let sids: Vec<u64> = indexed.iter().map(|(s, _, _)| *s).collect();
        if sids.len() > MESH_MAX {
            room.forwarding = Some(self.forwarding_for(&sids, my_sid, key, auth)?);
        }
        let room = Arc::new(room);
        lock(&room.leaves).extend(indexed.iter().map(|(s, d, _)| (*d, *s)));
        self.install(room.clone());
        self.join_devices(&room, &devices, None).await;
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Calls a contact directly. The contact's first device rings. Nothing connects until it
    /// accepts. A direct call has no MLS group, so its two participants take sender ids 0 for the
    /// caller and 1 for the callee.
    pub async fn call_dm(&self, peer: RootId) -> Result<()> {
        let me = self.inner.me;
        let (device, cert) = {
            let w = self.read();
            if !w.is_contact(&peer) {
                return Err(CoreError::Invalid("can only call a contact".into()));
            }
            let device = w
                .directory
                .devices_of(&peer)
                .into_iter()
                .next()
                .ok_or_else(|| CoreError::NotFound("no known device for that contact".into()))?;
            let cert = w
                .directory
                .cert_of(&device)
                .ok_or_else(|| CoreError::NotFound("no certificate for that device".into()))?;
            (device, cert)
        };
        let (a, b) = if me <= peer { (me, peer) } else { (peer, me) };
        let room_id = room_id(&[&a.0[..], &b.0[..]]);
        if self.check_free(room_id)?.is_some() {
            return Ok(());
        }
        // chisle: a DM call has no MLS group yet. The caller picks the key, and it reaches the
        // peer only inside the authenticated device RPC. Switch to the DM group when it exists.
        let key = DeviceKey::generate().0.seed();
        let auth = self.auth_for(&[(0, self.my_cert()), (1, cert)]);
        let mesh = Mesh::new_authenticated(MediaKey::new(key), 0, auth).map_err(call_err)?;
        let room = Arc::new(Room::new(room_id, Scope::Dm, mesh, 0, 0, Some(peer)));
        self.install(room.clone());
        if self.join_devices(&room, &[device], Some(key)).await == 0 {
            *lock(&self.inner.calls.current) = None;
            self.emit(EngineEvent::CallUpdated);
            return Err(CoreError::Rejected(
                "that person cannot be called right now".into(),
            ));
        }
        self.end_if_unanswered(room.id);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Returns how many devices answered, either in the call or ringing.
    async fn join_devices(&self, room: &Arc<Room>, devices: &[DeviceId], key: Option<[u8; 32]>) -> usize {
        let mut answered = 0;
        let muted = room.is_muted();
        *lock(&room.targets) = devices.to_vec();
        let from = match self.my_addr() {
            Ok(addr) => addr,
            Err(e) => {
                warn!(error = %e, "call not joined: no address");
                return 0;
            }
        };
        for device in devices {
            let msg = CallMsg::Join(Box::new(JoinMsg {
                room: room.id,
                scope: room.scope,
                cert: self.my_cert(),
                key,
                muted,
                from: from.clone(),
            }));
            match self.call_rpc(*device, msg).await {
                Ok(CallReply::Here { cert, .. }) if cert.device == *device => {
                    answered += 1;
                    // A forwarded call shows everyone who is in it, even without a direct link.
                    if let (Some(_), Some(sid)) = (&room.forwarding, room.sid_of(device)) {
                        room.add_peer(sid, *device, cert.root, true);
                    }
                    if room.offers_to(device) {
                        if let Err(e) = self.connect(room.clone(), *device).await {
                            warn!(device = %device, error = %e, "call link not made");
                        }
                    }
                }
                Ok(CallReply::Ringing) => answered += 1,
                Ok(reply) => debug!(device = %device, ?reply, "device is not in the call"),
                Err(e) => debug!(device = %device, error = %e, "call join not delivered"),
            }
        }
        answered
    }

    /// Offering side of one pair: dials the link, offers, and applies the answer.
    async fn connect(&self, room: Arc<Room>, device: DeviceId) -> Result<()> {
        if room.forwarding.is_some() {
            return self.connect_leaf(room, device).await;
        }
        let sid = room
            .sid_of(&device)
            .ok_or_else(|| call_err("device is not in this call"))?;
        let link = self.node().open_media(device).await?;
        let direct = path_is_direct(&link);
        let transport = LinkTransport::new(link, Side::Caller);
        let offer = {
            let mut mesh = room.mesh.lock().await;
            mesh.offer(sid, transport).await.map_err(call_err)?
        };
        let reply = self
            .call_rpc(
                device,
                CallMsg::Offer {
                    room: room.id,
                    signal: offer.to_bytes().map_err(call_err)?,
                },
            )
            .await?;
        match reply {
            CallReply::Answer(bytes) => {
                let answer = Signal::from_bytes(&bytes).map_err(call_err)?;
                room.mesh.lock().await.apply(sid, answer).map_err(call_err)?;
                room.add_peer(sid, device, self.root_of_device(&device), direct);
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
            if let Some(link) = lock(&self.inner.calls.incoming).remove(&from) {
                return Ok(link);
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                return Err(call_err("no media link from the peer"));
            }
        }
    }

    /// Sends one signalling message and waits for its reply.
    async fn call_rpc(&self, device: DeviceId, msg: CallMsg) -> Result<CallReply> {
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
                    from: addr,
                } = *join;
                self.on_join(from, room, scope, cert, key, muted, addr)
            }
            CallMsg::Offer { room, signal } => self.on_offer(from, room, signal).await,
            CallMsg::Mute { room, muted } => {
                if let Some(current) = self.current_room(&room) {
                    let Some(sid) = current.sid_of(&from) else {
                        return CallReply::Refused("not in this call".into());
                    };
                    if let Some(peer) = lock(&current.peers).get_mut(&sid) {
                        peer.muted = muted;
                    }
                    self.emit(EngineEvent::CallUpdated);
                }
                CallReply::Ack
            }
            CallMsg::Decline { room } => {
                // The other side refused, or gave up before anyone joined. End our unanswered call.
                let unanswered = self
                    .current_room(&room)
                    .is_some_and(|r| lock(&r.peers).is_empty());
                if unanswered {
                    *lock(&self.inner.calls.current) = None;
                    self.emit(EngineEvent::CallUpdated);
                }
                CallReply::Ack
            }
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
                if let Some(current) = self.current_room(&room) {
                    let Some(sid) = current.sid_of(&from) else {
                        return CallReply::Refused("not in this call".into());
                    };
                    current.mesh.lock().await.remove_peer(sid);
                    lock(&current.peers).remove(&sid);
                    self.emit(EngineEvent::CallUpdated);
                }
                CallReply::Ack
            }
        }
    }

    fn current_room(&self, id: &RoomId) -> Option<Arc<Room>> {
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
    ) -> CallReply {
        if cert.device != from || cert.verify().is_err() {
            return CallReply::Refused("certificate does not match the sender".into());
        }
        self.learn_peer(from, addr);
        if let Err(why) = self.may_join(&scope, cert.root) {
            return CallReply::Refused(why);
        }
        let existing = lock(&self.inner.calls.current).clone();
        match existing {
            Some(room) if room.id == id => return self.answer_join(&room, from, cert.root),
            Some(_) => return CallReply::Refused("busy in another call".into()),
            None => {}
        }
        match scope {
            Scope::Channel { .. } => CallReply::Refused("not in this call".into()),
            Scope::Dm => self.ring(id, from, cert, key),
        }
    }

    /// The device is in this call: say so, and connect if this side has the lower id.
    fn answer_join(&self, room: &Arc<Room>, from: DeviceId, root: RootId) -> CallReply {
        if room.forwarding.is_some() {
            if let Some(sid) = room.sid_of(&from) {
                room.add_peer(sid, from, root, true);
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
                key,
            });
        }
        self.emit(EngineEvent::IncomingCall {
            call_id: hex(&id),
            from: cert.root.to_string(),
            scope: CallScope::Dm,
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
        let room = self.open_dm_room(id, &ringing.cert, Some(ringing.key))?;
        self.join_devices(&room, &[ringing.from], None).await;
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
                Ok(())
            }
        }
    }

    /// Creates the room for an incoming DM call, keyed by the caller's key.
    fn open_dm_room(&self, id: RoomId, cert: &DeviceCert, key: Option<[u8; 32]>) -> Result<Arc<Room>> {
        let key = key.ok_or_else(|| CoreError::Invalid("DM call has no key".into()))?;
        let my_sid = 1;
        let certs = vec![(1, self.my_cert()), (0, cert.clone())];
        let mesh = Mesh::new_authenticated(MediaKey::new(key), my_sid, self.auth_for(&certs))
            .map_err(call_err)?;
        let room = Arc::new(Room::new(id, Scope::Dm, mesh, my_sid, 0, Some(cert.root)));
        self.install(room.clone());
        self.emit(EngineEvent::CallUpdated);
        Ok(room)
    }

    /// Accepts an offer. The offerer dialled the link, so it is already waiting for us.
    async fn on_offer(&self, from: DeviceId, id: RoomId, signal: Vec<u8>) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        let outcome: Result<Vec<u8>> = async {
            let link = self.wait_incoming(from).await?;
            let direct = path_is_direct(&link);
            let offer = Signal::from_bytes(&signal).map_err(call_err)?;
            let sid = room
                .sid_of(&from)
                .ok_or_else(|| call_err("not in this call"))?;
            let transport = LinkTransport::new(link, Side::Callee);
            let answer = match &room.forwarding {
                // A forwarded call: the forwarder takes the leaf into its relay.
                Some(forwarding) => {
                    let mut sfu = forwarding.sfu.lock().await;
                    let sfu = sfu
                        .as_mut()
                        .ok_or_else(|| call_err("this device is not the forwarder"))?;
                    sfu.accept(sid, transport, offer).await.map_err(call_err)?
                }
                None => {
                    let mut mesh = room.mesh.lock().await;
                    mesh.accept(sid, transport, offer).await.map_err(call_err)?
                }
            };
            room.add_peer(sid, from, self.root_of_device(&from), direct);
            self.emit(EngineEvent::CallUpdated);
            answer.to_bytes().map_err(call_err)
        }
        .await;
        match outcome {
            Ok(bytes) => CallReply::Answer(bytes),
            Err(e) => CallReply::Refused(e.to_string()),
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
        self.current_or_err()?
            .deafened
            .store(deafened, Ordering::Relaxed);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// Video is not built yet, so turning the camera on fails. Turning it off is accepted.
    pub fn set_camera(&self, on: bool) -> Result<()> {
        self.current_or_err()?;
        if on {
            return Err(CoreError::Invalid("video is not built yet".into()));
        }
        Ok(())
    }

    /// Sends text to the call's conversation: the voice channel's messages, or a direct message
    /// to the other person of a DM call.
    pub async fn send_call_message(&self, text: String) -> Result<()> {
        let room = self.current_or_err()?;
        match room.scope {
            Scope::Channel { community, channel } => {
                self.send_message(community, channel, text).await?;
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
        let Some(room) = lock(&self.inner.calls.current).take() else {
            return Ok(());
        };
        // Peers drop their link. Devices still ringing stop ringing.
        let mut devices: Vec<DeviceId> = lock(&room.peers).values().map(|p| p.device).collect();
        devices.extend(lock(&room.targets).iter().copied());
        devices.sort();
        devices.dedup();
        for device in devices {
            if let Err(e) = self.call_rpc(device, CallMsg::Leave { room: room.id }).await {
                debug!(device = %device, error = %e, "leave not sent");
            }
        }
        let sids: Vec<u64> = lock(&room.peers).keys().copied().collect();
        let mut mesh = room.mesh.lock().await;
        for sid in sids {
            mesh.remove_peer(sid);
        }
        drop(mesh);
        self.emit(EngineEvent::CallUpdated);
        Ok(())
    }

    /// The call on this device, or `None` when it is not in one.
    pub fn call_state(&self) -> Option<CallView> {
        let room = lock(&self.inner.calls.current).clone()?;
        let quality = lock(&room.quality);
        let now = Instant::now();
        let w = self.read();
        let peers = lock(&room.peers);
        let participants: Vec<ParticipantView> = peers
            .values()
            .map(|p| ParticipantView {
                root_id: p.root.to_string(),
                display_name: w.display_name(&p.root),
                muted: p.muted,
                video: false,
                speaking: !p.muted
                    && p.last_frame
                        .is_some_and(|t| now.duration_since(t) < SPEAKING_WINDOW),
                forwarder: false,
                path: if p.direct {
                    PathView::Direct
                } else {
                    PathView::Relayed
                },
            })
            .collect();
        // Two devices in all make a direct link. Three or more make a mesh.
        let topology = if room.forwarding.is_some() {
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
        };
        Some(CallView {
            id: hex(&room.id),
            scope,
            peer: room.peer.map(|p| p.to_string()),
            muted: room.is_muted(),
            deafened: room.deafened.load(Ordering::Relaxed),
            camera: false,
            started_ms: room.started_ms,
            route: route_for(topology, &participants, relayed),
            participants,
            topology,
            quality: CallQuality {
                rtt_ms: quality.rtt_ms,
                loss_pct: quality.loss_pct,
                jitter_ms,
            },
            key_epoch: room.epoch.load(Ordering::Relaxed),
            frames_received: quality.frames_received,
        })
    }

    /// Rekeys a channel call when the MLS epoch moved, and drops participants who were banned.
    async fn check_membership(&self, room: &Arc<Room>, mesh: &mut Mesh<LinkTransport>) {
        if let Scope::Channel { community, .. } = room.scope {
            if let Ok(indexed) = self.indexed_members(community) {
                *lock(&room.leaves) = indexed.into_iter().map(|(sid, d, _)| (d, sid)).collect();
            }
            let current = self.with_mls_read(|mls| mls.epoch(&community.0)).ok();
            if let Some(epoch) = current {
                if epoch != room.epoch.load(Ordering::Relaxed) {
                    match self.channel_key(community) {
                        Ok((key, epoch)) => {
                            if let Err(e) = mesh.set_key(key, epoch) {
                                warn!(error = %e, "call rekey failed");
                            } else {
                                room.epoch.store(epoch, Ordering::Relaxed);
                            }
                        }
                        Err(e) => warn!(error = %e, "call rekey failed"),
                    }
                }
            }
            let banned: Vec<u64> = {
                let w = self.read();
                let state = w.communities.get(&community).map(|c| c.state());
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
    }

    /// Copies the mesh's link measurements into the view's cache.
    fn refresh_quality(&self, room: &Arc<Room>, mesh: &Mesh<LinkTransport>) {
        let stats = mesh.stats();
        let mut rtts = Vec::new();
        let (mut lost, mut sent) = (0u64, 0u64);
        for s in stats.values() {
            if let Some(rtt) = s.rtt_ms {
                rtts.push(rtt);
            }
            lost += s.packets_lost;
            sent += s.packets_sent;
        }
        let mut quality = lock(&room.quality);
        quality.rtt_ms = if rtts.is_empty() {
            None
        } else {
            Some(rtts.iter().sum::<f64>() / rtts.len() as f64)
        };
        quality.loss_pct = if sent == 0 {
            0.0
        } else {
            lost as f64 * 100.0 / sent as f64
        };
    }
}
