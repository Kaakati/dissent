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

use std::collections::BTreeMap;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::auth::{digest_sender, is_digest};
use crate::call::Call;
use crate::e2ee::{audio_level, peek_sender};
use crate::election::{Mode, PeerId, AUDIO_TOP_SPEAKERS};
use crate::error::{MediaError, Result};
use crate::signal::Signal;
use crate::transport::PacketTransport;

/// How recently a participant must have sent a frame to count as an active speaker.
const ACTIVE_WINDOW: Duration = Duration::from_secs(1);

/// The loudest a frame may be quiet and still count as speech. Frames at or above this level
/// are silence or background, and are never forwarded as speakers. Without it, a forwarder that
/// has just started could forward silence, because it has not yet heard any speakers.
pub const SPEECH_LEVEL_MAX: u8 = 50;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    /// A participant or child forwarder.
    Down(PeerId),
    /// A parent forwarder.
    Up(PeerId),
}

type Boxed<'a, O> = Pin<Box<dyn Future<Output = O> + Send + 'a>>;
type RawOut = Result<Option<Vec<u8>>>;

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
    /// Each downlink's bitrate budget in kbps, as the estimator reports it.
    budgets: BTreeMap<PeerId, u32>,
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

    /// The layer to send one listener, out of `layers_kbps` (ascending). A listener with no
    /// budget gets the top layer. Simulcast frames are not carried yet, so this only chooses.
    pub fn listener_layer(&self, downlink: PeerId, layers_kbps: &[u32]) -> Option<usize> {
        let budget = self.budgets.get(&downlink).copied().unwrap_or(u32::MAX);
        crate::bwe::pick_layer(budget, layers_kbps)
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

    /// Waits for one packet from any link and forwards it. Returns `false` once no link is left.
    /// A link that fails is removed, and the others keep going.
    ///
    /// This method is cancel-safe.
    pub async fn step(&mut self) -> Result<bool> {
        loop {
            if self.links.is_empty() && self.upstream.is_empty() {
                return Ok(false);
            }
            let (from, outcome) = self.poll_links().await;
            match outcome {
                Ok(Some(wire)) => {
                    self.forward(from, wire).await;
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
                Box::pin(call.next_raw()) as Boxed<'_, RawOut>,
            ));
        }
        for (peer, call) in self.links.iter_mut() {
            futs.push((
                Link::Down(*peer),
                Box::pin(call.next_raw()) as Boxed<'_, RawOut>,
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

    async fn forward(&mut self, from: Link, wire: Vec<u8>) {
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
