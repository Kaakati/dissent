//! The forwarder. It relays encrypted frames and digests between participants, holds no key, and
//! can sit in a cascade.
//!
//! A forwarder has downlinks and, unless it is a root, upstream parents:
//!
//! - A downlink is a participant or a child forwarder. Its frames are routed by the sender named
//!   in their key id, which a forwarder can read without a key.
//! - A frame from a downlink whose sender is one of this forwarder's top speakers goes to the
//!   other downlinks and to the active parent.
//! - A frame from the active parent goes to every downlink. The parent already chose it.
//! - A standby parent is connected but idle, so a failover needs no handshake.
//!
//! Digests follow the same rules as frames, so the receivers still verify every sender.
//!
//! Video can come in simulcast layers, named by the clear header. The forwarder sends each viewer
//! one layer of each stream: the smallest that covers the viewer's tile, lowered to fit the
//! viewer's budget, the active speaker last. It changes a viewer's layer only at a keyframe of the
//! new layer, and keeps sending the old one until then. A viewer that does not know layers (a
//! released client) always gets layer 0. See `docs/design/13-forwarded-calls.md`, "Layers per
//! viewer".

use std::collections::{BTreeMap, HashSet};
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::auth::{digest_sender, is_digest};
use crate::call::{Call, CallStats};
use crate::e2ee::{audio_level, peek_sender};
use crate::election::{Mode, PeerId, AUDIO_STREAM_KBPS, AUDIO_TOP_SPEAKERS};
use crate::error::{MediaError, Result};
use crate::signal::Signal;
use crate::transport::PacketTransport;
use crate::video::{layer_key, stream_key, RawFrame, RawVideo, MAX_LAYERS};

/// How recently a participant must have sent a frame to count as an active speaker.
const ACTIVE_WINDOW: Duration = Duration::from_secs(1);

/// The loudest a frame may be quiet and still count as speech. Frames at or above this level
/// are silence or background, and are never forwarded as speakers. Without it, a forwarder that
/// has just started could forward silence, because it has not yet heard any speakers.
pub const SPEECH_LEVEL_MAX: u8 = 50;

/// One simulcast layer of a stream, as its sender announces it: its width in pixels and its
/// bitrate in kbps. A stream's layers go from the smallest, layer 0, up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerInfo {
    pub width: u32,
    pub kbps: u32,
}

/// Called with each packet the forwarder receives, before it forwards anything. The encrypted
/// bytes are what the forwarder can see.
pub type Observer = Box<dyn FnMut(PeerId, &[u8]) + Send>;

/// Counters of the forwarder's work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SfuStats {
    /// Packets received from downlinks and from the active parent.
    pub frames_in: u64,
    /// Packet copies written to links.
    pub frames_forwarded: u64,
    /// Packets not forwarded, because their sender is not a top speaker here, or the link was
    /// still connecting.
    pub frames_dropped: u64,
    /// Downlinks that closed or failed, and were removed.
    pub links_lost: u64,
    /// Times the active parent was lost and the forwarder moved to a standby.
    pub upstream_failovers: u64,
}

/// Where a packet came from, or where it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Link {
    /// A participant or child forwarder.
    Down(PeerId),
    /// A parent forwarder.
    Up(PeerId),
}

type Boxed<'a, O> = Pin<Box<dyn Future<Output = O> + Send + 'a>>;
type RawOut = Result<Option<RawFrame>>;

/// A forwarder node.
///
/// The constructor takes no media key. The forwarder cannot decrypt, and the type gives it no way
/// to learn a key.
pub struct Sfu<T: PacketTransport> {
    mode: Mode,
    links: BTreeMap<PeerId, Call<T>>,
    upstream: BTreeMap<PeerId, Call<T>>,
    active_up: Option<PeerId>,
    levels: BTreeMap<u64, (u8, Instant)>,
    /// Where the next poll starts, so no link is starved by a busy one.
    rotation: usize,
    /// Each downlink's bitrate budget in kbps, as set by `set_listener_budget`.
    budgets: BTreeMap<PeerId, u32>,
    /// Each downlink's video budget from its own bandwidth estimate, in kbps, while that estimate
    /// is below where it starts. See `refresh_estimates`.
    estimated: BTreeMap<PeerId, u32>,
    /// Whether downlink estimates cap budgets. Tests turn it off so a set budget is the only limit.
    use_estimates: bool,
    /// Each (downlink, sender) pair that has received a keyframe, so its deltas may follow.
    video_started: HashSet<(Link, u64)>,
    /// Each (downlink, sender) pair that has asked for a keyframe and not yet got one.
    video_wanted: HashSet<(Link, u64)>,
    /// The link each sender's video arrives on, for keyframe requests.
    video_source: BTreeMap<u64, Link>,
    /// Each stream's layers, as its sender announced them. A stream with none has one layer.
    layers: BTreeMap<u64, Vec<LayerInfo>>,
    /// The width each viewer draws each stream at, in pixels, by (viewer, stream).
    tiles: BTreeMap<(PeerId, u64), u32>,
    /// Viewers that know layers. Any other viewer gets layer 0 only.
    layer_aware: HashSet<PeerId>,
    /// The layer each (viewer, stream) is being sent now. It changes at a keyframe of the new one.
    sending_layer: BTreeMap<(Link, u64), u8>,
    observer: Option<Observer>,
    stats: SfuStats,
}

impl<T: PacketTransport> Sfu<T> {
    /// A forwarder for `mode`, with no links yet.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            links: BTreeMap::new(),
            upstream: BTreeMap::new(),
            active_up: None,
            levels: BTreeMap::new(),
            rotation: 0,
            budgets: BTreeMap::new(),
            estimated: BTreeMap::new(),
            use_estimates: true,
            video_started: HashSet::new(),
            video_wanted: HashSet::new(),
            video_source: BTreeMap::new(),
            layers: BTreeMap::new(),
            tiles: BTreeMap::new(),
            layer_aware: HashSet::new(),
            sending_layer: BTreeMap::new(),
            observer: None,
            stats: SfuStats::default(),
        }
    }

    /// Calls `observer` with every packet the forwarder receives. Use it to audit what the
    /// forwarder can see.
    pub fn with_observer(mut self, observer: Observer) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Accepts a downlink's offer (a participant or a child forwarder) and returns the answer.
    pub async fn accept(
        &mut self,
        downlink: PeerId,
        transport: T,
        offer: Signal,
    ) -> Result<Signal> {
        let (call, answer) = Call::accept_blind(transport, offer).await?;
        self.links.insert(downlink, call);
        Ok(answer)
    }

    /// Offers this forwarder's upstream link to `parent`, and returns the offer to send to it.
    /// The first parent offered becomes the active one. Later ones stand by.
    pub async fn offer_upstream(&mut self, parent: PeerId, transport: T) -> Result<Signal> {
        let (call, offer) = Call::offer_blind(transport).await?;
        self.upstream.insert(parent, call);
        if self.active_up.is_none() {
            self.active_up = Some(parent);
        }
        Ok(offer)
    }

    /// Applies the parent's answer to the offer for `parent`.
    pub fn apply_upstream(&mut self, parent: PeerId, answer: Signal) -> Result<()> {
        self.upstream
            .get_mut(&parent)
            .ok_or(MediaError::BadState("no upstream link to this parent"))?
            .apply(answer)
    }

    /// The parent this forwarder sends to now, if any.
    pub fn active_upstream(&self) -> Option<PeerId> {
        self.active_up
    }

    /// The downlinks with a live link.
    pub fn downlinks(&self) -> Vec<PeerId> {
        self.links.keys().copied().collect()
    }

    /// Records what a listener's link can carry, in kbps.
    pub fn set_listener_budget(&mut self, downlink: PeerId, kbps: u32) {
        self.budgets.insert(downlink, kbps);
    }

    /// Whether each downlink's own bandwidth estimate also caps its budget (on by default). Off, a
    /// budget set with `set_listener_budget` is the only limit, so a test is not at the mercy of a
    /// loaded machine's estimates.
    pub fn set_use_estimates(&mut self, on: bool) {
        self.use_estimates = on;
        if !on {
            self.estimated.clear();
        }
    }

    /// The layer to send one listener, out of `layers_kbps` (ascending). A listener with no
    /// budget gets the top layer.
    pub fn listener_layer(&self, downlink: PeerId, layers_kbps: &[u32]) -> Option<usize> {
        let budget = self.budgets.get(&downlink).copied().unwrap_or(u32::MAX);
        crate::bwe::pick_layer(budget, layers_kbps)
    }

    /// Records the layers `sender` sends for its camera or screen, smallest first. At most
    /// [`MAX_LAYERS`] are kept.
    pub fn set_sender_layers(&mut self, sender: u64, screen: bool, layers: Vec<LayerInfo>) {
        let mut layers = layers;
        layers.truncate(usize::from(MAX_LAYERS));
        self.layers.insert(stream_key(sender, screen), layers);
    }

    /// The layers `sender` announced for its camera or screen, smallest first. Empty if none.
    pub fn sender_layers(&self, sender: u64, screen: bool) -> &[LayerInfo] {
        self.layers.get(&stream_key(sender, screen)).map_or(&[], Vec::as_slice)
    }

    /// Records how wide `viewer` draws `sender`'s camera or screen, in pixels. Zero means it is not
    /// shown.
    pub fn set_viewer_tile(&mut self, viewer: PeerId, sender: u64, screen: bool, width: u32) {
        self.tiles.insert((viewer, stream_key(sender, screen)), width);
    }

    /// Forgets every tile width `viewer` reported, before it reports its whole set again. A tile
    /// it no longer draws then counts as not shown.
    pub fn clear_viewer_tiles(&mut self, viewer: PeerId) {
        self.tiles.retain(|(v, _), _| *v != viewer);
    }

    /// The layer `viewer` should get of `sender`'s camera or screen now, or `None` if the stream
    /// has no layers announced. See `plan_for`.
    pub fn planned_layer(&self, viewer: PeerId, sender: u64, screen: bool) -> Option<u8> {
        self.plan_for(viewer).get(&stream_key(sender, screen)).copied()
    }

    /// Whether `viewer` knows simulcast layers. A viewer that does not gets layer 0 only, since it
    /// would decode two layers as one stream.
    pub fn set_layer_aware(&mut self, viewer: PeerId, aware: bool) {
        if aware {
            self.layer_aware.insert(viewer);
        } else {
            self.layer_aware.remove(&viewer);
        }
    }

    /// The layers of `sender`'s camera or screen that some viewer is sent or is waiting to be
    /// switched to, as a bit mask (bit 0 for layer 0). The sender encodes only these. A stream
    /// with no viewer yet gives 0.
    pub fn wanted_layers(&self, sender: u64, screen: bool) -> u8 {
        let stream = stream_key(sender, screen);
        let mut mask = 0u8;
        for viewer in self.links.keys().copied().filter(|v| *v != sender) {
            let plan = self.plan_for(viewer);
            if let Some(layer) = plan.get(&stream) {
                mask |= 1 << layer;
            }
            if let Some(layer) = self.sending_layer.get(&(Link::Down(viewer), stream)) {
                mask |= 1 << layer;
            }
        }
        mask
    }

    /// The layer `viewer` should get of each stream it receives: the smallest that covers its tile,
    /// then lowered until the sum fits its budget. Streams other than the active speaker's are
    /// lowered first, the one that saves most first, and the speaker's only after them.
    fn plan_for(&self, viewer: PeerId) -> BTreeMap<u64, u8> {
        let mut plan: BTreeMap<u64, u8> = BTreeMap::new();
        let aware = self.layer_aware.contains(&viewer);
        for (stream, layers) in &self.layers {
            // A viewer is never sent its own stream, nor one that is not forwarded now.
            if *stream >> 1 == viewer || !self.is_active(*stream >> 1) {
                continue;
            }
            if !aware || layers.len() <= 1 {
                plan.insert(*stream, 0);
                continue;
            }
            let tile = self.tiles.get(&(viewer, *stream)).copied().unwrap_or(0);
            let top = layers.len() - 1;
            let fits = layers.iter().position(|l| l.width >= tile).unwrap_or(top);
            plan.insert(*stream, u8::try_from(fits).unwrap_or(0));
        }
        let budget = match (self.budgets.get(&viewer), self.estimated.get(&viewer)) {
            (Some(set), Some(estimated)) => (*set).min(*estimated),
            (Some(budget), None) | (None, Some(budget)) => *budget,
            (None, None) => return plan,
        };
        let speaker = self.top_speakers().first().copied();
        let cost = |plan: &BTreeMap<u64, u8>| -> u64 {
            plan.iter()
                .map(|(stream, layer)| {
                    self.layers
                        .get(stream)
                        .and_then(|l| l.get(usize::from(*layer)))
                        .map_or(0, |l| u64::from(l.kbps))
                })
                .sum()
        };
        while cost(&plan) > u64::from(budget) {
            let saving = |stream: &u64, layer: u8| -> u64 {
                let layers = &self.layers[stream];
                let now = u64::from(layers[usize::from(layer)].kbps);
                let below = u64::from(layers[usize::from(layer) - 1].kbps);
                now.saturating_sub(below)
            };
            // The streams that can still go down a layer, the speaker's last.
            let next = plan
                .iter()
                .filter(|(_, layer)| **layer > 0)
                .map(|(stream, layer)| (*stream, *layer))
                .max_by_key(|(stream, layer)| (Some(*stream >> 1) != speaker, saving(stream, *layer)));
            match next {
                Some((stream, layer)) => {
                    plan.insert(stream, layer - 1);
                }
                None => break,
            }
        }
        plan
    }

    /// The forwarder's counters.
    pub fn stats(&self) -> SfuStats {
        self.stats
    }

    /// The top speakers right now: the loudest recent senders, by audio level.
    pub fn top_speakers(&self) -> Vec<u64> {
        let now = Instant::now();
        let mut recent: Vec<(u8, u64)> = self
            .levels
            .iter()
            .filter(|(_, (level, seen))| {
                *level <= SPEECH_LEVEL_MAX && now.duration_since(*seen) <= ACTIVE_WINDOW
            })
            .map(|(sender, (level, _))| (*level, *sender))
            .collect();
        // A lower level is louder. Ties go to the lower sender id, so the choice is stable.
        recent.sort();
        recent
            .into_iter()
            .take(AUDIO_TOP_SPEAKERS as usize)
            .map(|(_, sender)| sender)
            .collect()
    }

    /// The stats of each downlink, keyed by the participant it serves. Each carries that
    /// downlink's bandwidth target in `target_kbps`.
    pub fn downlink_stats(&self) -> BTreeMap<PeerId, CallStats> {
        self.links
            .iter()
            .map(|(peer, call)| (*peer, call.stats()))
            .collect()
    }

    /// Waits for one packet from any link and forwards it. Returns `false` once no link is left.
    /// A link that fails is removed, and the others keep going.
    ///
    /// This method is cancel-safe.
    pub async fn step(&mut self) -> Result<bool> {
        loop {
            if self.links.is_empty() && self.upstream.is_empty() {
                return Ok(false);
            }
            self.answer_keyframe_requests();
            self.refresh_estimates();
            let (from, outcome) = self.poll_links().await;
            match outcome {
                Ok(Some(frame)) => {
                    self.forward(from, frame).await;
                    return Ok(true);
                }
                Ok(None) | Err(_) => self.lose(from),
            }
        }
    }

    /// Forwards packets until no link is left.
    pub async fn run(&mut self) -> Result<()> {
        while self.step().await? {}
        Ok(())
    }

    /// Polls every link, starting at a rotating position. A busy link must not starve the others:
    /// a link that is never polled never runs its handshake or timers.
    async fn poll_links(&mut self) -> (Link, RawOut) {
        let mut futs: Vec<(Link, Boxed<'_, RawOut>)> = Vec::new();
        for (peer, call) in self.upstream.iter_mut() {
            futs.push((
                Link::Up(*peer),
                Box::pin(call.next_raw_any()) as Boxed<'_, RawOut>,
            ));
        }
        for (peer, call) in self.links.iter_mut() {
            futs.push((
                Link::Down(*peer),
                Box::pin(call.next_raw_any()) as Boxed<'_, RawOut>,
            ));
        }
        let count = futs.len();
        let start = self.rotation % count.max(1);
        self.rotation = self.rotation.wrapping_add(1);
        poll_fn(|cx| {
            for step in 0..count {
                let (link, fut) = &mut futs[(start + step) % count];
                if let Poll::Ready(out) = fut.as_mut().poll(cx) {
                    return Poll::Ready((*link, out));
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Removes a link that closed or failed. Losing the active parent moves to a standby.
    fn lose(&mut self, link: Link) {
        match link {
            Link::Down(peer) => {
                self.links.remove(&peer);
                self.stats.links_lost += 1;
            }
            Link::Up(peer) => {
                self.upstream.remove(&peer);
                if self.active_up == Some(peer) {
                    self.active_up = self.upstream.keys().next().copied();
                    self.stats.upstream_failovers += 1;
                }
            }
        }
    }

    async fn forward(&mut self, from: Link, frame: RawFrame) {
        match frame {
            RawFrame::Audio(wire) => self.forward_audio(from, wire).await,
            RawFrame::Video(raw) => self.forward_video(from, raw).await,
        }
    }

    async fn forward_audio(&mut self, from: Link, wire: Vec<u8>) {
        self.stats.frames_in += 1;
        if let Some(observer) = self.observer.as_mut() {
            let origin = match from {
                Link::Down(peer) | Link::Up(peer) => peer,
            };
            observer(origin, &wire);
        }

        // The sender a packet names in its key id, or in its digest. A forwarder never sends a
        // sender's packets back to that sender's own link.
        let digest = is_digest(&wire);
        let sender = if digest {
            digest_sender(&wire)
        } else {
            peek_sender(&wire)
        };

        let targets = match from {
            Link::Up(parent) => {
                // Only the active parent is listened to. A standby sends nothing until it is
                // promoted, so anything from it is stale and dropped.
                if self.active_up != Some(parent) {
                    self.stats.frames_dropped += 1;
                    return;
                }
                self.downlinks()
                    .into_iter()
                    .filter(|peer| Some(*peer) != sender)
                    .map(Link::Down)
                    .collect::<Vec<_>>()
            }
            Link::Down(child) => {
                if !digest {
                    let level = audio_level(&wire).unwrap_or(127);
                    if let Some(sender) = sender {
                        self.levels.insert(sender, (level, Instant::now()));
                    }
                }
                let Some(sender) = sender else {
                    self.stats.frames_dropped += 1;
                    return;
                };
                if !self.is_active(sender) {
                    self.stats.frames_dropped += 1;
                    return;
                }
                let mut targets: Vec<Link> = self
                    .links
                    .keys()
                    .copied()
                    .filter(|peer| *peer != child && Some(*peer) != Some(sender))
                    .map(Link::Down)
                    .collect();
                if let Some(parent) = self.active_up {
                    targets.push(Link::Up(parent));
                }
                targets
            }
        };
        self.fan_out(targets, &wire).await;
    }

    /// Routes one video frame that a link admitted. A downlink gets a frame only after it has a
    /// keyframe from this sender, so it never starts mid-GOP. Until then the forwarder asks the
    /// frame's source for a keyframe.
    async fn forward_video(&mut self, from: Link, raw: RawVideo) {
        if let Link::Up(parent) = from {
            if self.active_up != Some(parent) {
                self.stats.frames_dropped += 1;
                return;
            }
        }
        if !self.is_active(raw.sender) {
            self.stats.frames_dropped += 1;
            return;
        }
        let stream = stream_key(raw.sender, raw.screen);
        self.video_source.insert(stream, from);
        // A sender that never announced layers has one, and its frames are all layer 0.
        let multi = self.layers.get(&stream).is_some_and(|l| l.len() > 1);

        let mut targets: Vec<Link> = self
            .links
            .keys()
            .copied()
            .filter(|peer| Link::Down(*peer) != from && *peer != raw.sender)
            .map(Link::Down)
            .collect();
        if let (Link::Down(_), Some(parent)) = (from, self.active_up) {
            targets.push(Link::Up(parent));
        }

        for target in targets {
            // Without a layer table, a stream's only layer for a viewer is 0. A stream that arrives
            // with more, such as from a parent forwarder whose sender announced layers to it, would
            // reach the decoder as two streams in one.
            if !multi && raw.layer != 0 && matches!(target, Link::Down(_)) {
                continue;
            }
            // The layer this target should be sent. A parent forwarder takes every layer, so it
            // can choose for its own viewers.
            if multi {
                if let Link::Down(viewer) = target {
                    let want = self.plan_for(viewer).get(&stream).copied().unwrap_or(0);
                    let now = self.sending_layer.get(&(target, stream)).copied();
                    if raw.layer != want && now != Some(raw.layer) {
                        continue;
                    }
                    if raw.layer == want && now != Some(want) {
                        // A switch, or the first frame: it starts at a keyframe of the new layer.
                        if !raw.keyframe {
                            if self.video_wanted.insert((target, layer_key(stream, want))) {
                                self.request_keyframe_from(from);
                            }
                            continue;
                        }
                        self.video_wanted.remove(&(target, layer_key(stream, want)));
                        self.sending_layer.insert((target, stream), want);
                        self.video_started.insert((target, layer_key(stream, want)));
                    }
                    // Otherwise this is the layer the target is on now, kept until the switch.
                }
            }
            let key = (target, if multi { layer_key(stream, raw.layer) } else { stream });
            if !raw.keyframe && !self.video_started.contains(&key) {
                if self.video_wanted.insert(key) {
                    self.request_keyframe_from(from);
                }
                continue;
            }
            if raw.keyframe {
                self.video_wanted.remove(&key);
            }
            self.video_started.insert(key);
            match self.send_video(target, &raw.nal).await {
                Ok(()) => self.stats.frames_forwarded += 1,
                Err(MediaError::NotConnected) => self.stats.frames_dropped += 1,
                // A downlink without video, `m=video 0`, just gets nothing.
                Err(MediaError::NoVideoTrack) => {}
                Err(e) => {
                    tracing::debug!(error = %e, "video copy refused, dropping the link");
                    self.lose(target);
                }
            }
        }
    }

    /// Writes one carrier NAL to a link, or does nothing if the link is gone.
    async fn send_video(&mut self, target: Link, nal: &[u8]) -> Result<()> {
        let call = match target {
            Link::Down(peer) => self.links.get_mut(&peer),
            Link::Up(peer) => self.upstream.get_mut(&peer),
        };
        match call {
            Some(call) => call.send_raw_video(nal).await,
            None => Ok(()),
        }
    }

    /// Asks the source of a stream for a keyframe, on the link the stream arrives on.
    fn request_keyframe_from(&mut self, source: Link) {
        let call = match source {
            Link::Down(peer) => self.links.get_mut(&peer),
            Link::Up(peer) => self.upstream.get_mut(&peer),
        };
        if let Some(call) = call {
            if let Err(e) = call.request_keyframe() {
                tracing::debug!(error = %e, "could not ask a source for a keyframe");
            }
        }
    }

    /// Takes each downlink's bandwidth estimate as its video budget, less the top speakers' audio.
    /// Nothing probes a link for headroom, so an estimate never climbs past where it starts
    /// (`BWE_INITIAL_KBPS`). Only an estimate that fell below it is a measured limit; at or above
    /// it the link counts as unlimited, as before layers. See `docs/design/13-forwarded-calls.md`.
    fn refresh_estimates(&mut self) {
        if !self.use_estimates {
            return;
        }
        let audio = AUDIO_TOP_SPEAKERS * AUDIO_STREAM_KBPS;
        self.estimated = self
            .links
            .iter()
            .filter_map(|(peer, call)| {
                let kbps = call.video_available_kbps()?;
                (u64::from(kbps) < crate::call::BWE_INITIAL_KBPS).then(|| (*peer, kbps.saturating_sub(audio)))
            })
            .collect();
    }

    /// A downlink that asks for a keyframe has lost video it cannot decode. Its stream restarts
    /// at the next keyframe of each sender, and each sender is asked for one now.
    fn answer_keyframe_requests(&mut self) {
        let asked: Vec<PeerId> = self
            .links
            .iter_mut()
            .filter_map(|(peer, call)| call.take_keyframe_request().then_some(*peer))
            .collect();
        for peer in asked {
            let link = Link::Down(peer);
            self.video_started.retain(|(target, _)| *target != link);
            self.video_wanted.retain(|(target, _)| *target != link);
            self.sending_layer.retain(|(target, _), _| *target != link);
            let sources: Vec<(u64, Link)> = self
                .video_source
                .iter()
                .map(|(sender, source)| (*sender, *source))
                .collect();
            for (sender, source) in sources {
                if self.is_active(sender >> 1) {
                    self.request_keyframe_from(source);
                }
            }
        }
    }

    /// Writes one packet to each target link, and drops any link that refuses it.
    async fn fan_out(&mut self, targets: Vec<Link>, wire: &[u8]) {
        for target in targets {
            let result = match target {
                Link::Down(peer) => match self.links.get_mut(&peer) {
                    Some(call) => call.send_encrypted(wire).await,
                    None => continue,
                },
                Link::Up(peer) => match self.upstream.get_mut(&peer) {
                    Some(call) => call.send_encrypted(wire).await,
                    None => continue,
                },
            };
            match result {
                Ok(()) => self.stats.frames_forwarded += 1,
                // The link is still connecting, so this copy cannot go yet.
                Err(MediaError::NotConnected) => self.stats.frames_dropped += 1,
                Err(e) => {
                    tracing::debug!(?target, error = %e, "dropping forwarder link after a send failure");
                    self.lose(target);
                }
            }
        }
    }

    fn is_active(&self, sender: u64) -> bool {
        match self.mode {
            Mode::Video720p => true,
            Mode::AudioTop3 => self.top_speakers().contains(&sender),
        }
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use crate::transport::UdpTransport;

    /// 180p, 360p and 720p, as a camera on the top rung announces them.
    fn three() -> Vec<LayerInfo> {
        vec![
            LayerInfo { width: 320, kbps: 150 },
            LayerInfo { width: 640, kbps: 400 },
            LayerInfo { width: 1280, kbps: 1500 },
        ]
    }

    /// A forwarder with senders 1, 2 and 3 on three layers, and viewer 9 drawing each at `tile`.
    fn forwarder(tile: u32) -> Sfu<UdpTransport> {
        let mut sfu = Sfu::new(Mode::Video720p);
        for sender in [1, 2, 3] {
            sfu.set_sender_layers(sender, false, three());
            sfu.set_viewer_tile(9, sender, false, tile);
        }
        sfu.set_layer_aware(9, true);
        sfu
    }

    fn speaks(sfu: &mut Sfu<UdpTransport>, sender: u64) {
        sfu.levels.insert(sender, (10, Instant::now()));
    }

    #[test]
    fn the_smallest_layer_that_covers_the_tile_is_chosen() {
        for (tile, layer) in [(0, 0), (200, 0), (320, 0), (321, 1), (640, 1), (900, 2), (3840, 2)] {
            assert_eq!(forwarder(tile).planned_layer(9, 1, false), Some(layer), "a {tile} px tile");
        }
    }

    #[test]
    fn a_viewer_that_does_not_know_layers_gets_the_base() {
        let mut sfu = forwarder(1280);
        sfu.set_layer_aware(9, false);
        assert_eq!(sfu.planned_layer(9, 1, false), Some(0));
    }

    #[test]
    fn a_stream_with_one_layer_or_none_is_sent_as_it_is() {
        let mut sfu = forwarder(1280);
        sfu.set_sender_layers(4, false, vec![LayerInfo { width: 1280, kbps: 1500 }]);
        assert_eq!(sfu.planned_layer(9, 4, false), Some(0), "one layer is layer 0");
        assert_eq!(sfu.planned_layer(9, 5, false), None, "a stream with no table is not planned");
        assert_eq!(sfu.planned_layer(1, 1, false), None, "a viewer is never planned its own stream");
    }

    #[test]
    fn a_tight_budget_lowers_the_others_before_the_speaker() {
        let mut sfu = forwarder(1280);
        speaks(&mut sfu, 3);
        // All three at 720p need 4500. 2300 holds the speaker at 720p and the others at 360p.
        sfu.set_listener_budget(9, 2_300);
        let plan = |sfu: &Sfu<UdpTransport>| [1, 2, 3].map(|s| sfu.planned_layer(9, s, false).unwrap());
        assert_eq!(plan(&sfu), [1, 1, 2]);
        // 1800 holds the speaker at 720p and the others at 180p.
        sfu.set_listener_budget(9, 1_800);
        assert_eq!(plan(&sfu), [0, 0, 2]);
        // Below that the speaker goes down too, only once the others are at the base.
        sfu.set_listener_budget(9, 1_000);
        assert_eq!(plan(&sfu), [0, 0, 1]);
        sfu.set_listener_budget(9, 100);
        assert_eq!(plan(&sfu), [0, 0, 0], "the base layer is the floor");
        // With room for everything, each gets what its tile needs, and no more.
        sfu.set_listener_budget(9, 10_000);
        assert_eq!(plan(&sfu), [2, 2, 2]);
    }

    #[test]
    fn without_a_speaker_the_layer_that_saves_most_drops_first() {
        let mut sfu = forwarder(1280);
        sfu.set_viewer_tile(9, 1, false, 640);
        // Streams 2 and 3 want 720p and stream 1 360p: 3400. At 2400 one 720p goes down, and 1100
        // is the most a single step saves.
        sfu.set_listener_budget(9, 2_400);
        let plan = [1, 2, 3].map(|s| sfu.planned_layer(9, s, false).unwrap());
        assert_eq!(plan.iter().filter(|l| **l == 2).count(), 1, "one 720p stream stays: {plan:?}");
        assert_eq!(plan[0], 1, "the 360p tile keeps its layer: {plan:?}");
    }
}
