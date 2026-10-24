//! The peer-SFU forwarder. It relays encrypted frames between participants and holds no key.
//!
//! Each participant has one link to the forwarder. The forwarder reads each frame's audio-level
//! byte, which is unencrypted SFrame metadata, to pick the speakers. It then writes the frame
//! unchanged to the other participants' links. The receivers decrypt it, because they hold the
//! key and the forwarder does not.

use std::collections::BTreeMap;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::auth::is_digest;
use crate::call::Call;
use crate::e2ee::audio_level;
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

/// Called with each frame the forwarder receives, before it forwards anything. The sender id and
/// the encrypted bytes are what the forwarder can see.
pub type Observer = Box<dyn FnMut(PeerId, &[u8]) + Send>;

/// Counters of the forwarder's work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SfuStats {
    /// Frames received from participants.
    pub frames_in: u64,
    /// Frame copies written to participant links.
    pub frames_forwarded: u64,
    /// Frames not forwarded, because their sender is not a top speaker.
    pub frames_dropped: u64,
    /// Links that closed or failed, and were removed.
    pub links_lost: u64,
}

/// A peer-SFU: a forwarder for one call.
///
/// The constructor takes no media key. The forwarder cannot decrypt, and the type gives it no
/// way to learn a key.
pub struct Sfu<T: PacketTransport> {
    mode: Mode,
    links: BTreeMap<PeerId, Call<T>>,
    levels: BTreeMap<PeerId, (u8, Instant)>,
    observer: Option<Observer>,
    stats: SfuStats,
}

type Boxed<'a, O> = Pin<Box<dyn Future<Output = O> + Send + 'a>>;
type RawOut = Result<Option<Vec<u8>>>;

impl<T: PacketTransport> Sfu<T> {
    /// A forwarder for `mode`, with no participants yet.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            links: BTreeMap::new(),
            levels: BTreeMap::new(),
            observer: None,
            stats: SfuStats::default(),
        }
    }

    /// Calls `observer` with every frame the forwarder receives. Use it to audit what the
    /// forwarder can see.
    pub fn with_observer(mut self, observer: Observer) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Accepts a participant's offer over `transport` and returns the answer to send back.
    pub async fn accept(
        &mut self,
        participant: PeerId,
        transport: T,
        offer: Signal,
    ) -> Result<Signal> {
        let (call, answer) = Call::accept_blind(transport, offer).await?;
        self.links.insert(participant, call);
        Ok(answer)
    }

    /// The participants with a live link.
    pub fn participants(&self) -> Vec<PeerId> {
        self.links.keys().copied().collect()
    }

    /// The forwarder's counters.
    pub fn stats(&self) -> SfuStats {
        self.stats
    }

    /// The top speakers right now: the loudest recent senders, by audio level.
    pub fn top_speakers(&self) -> Vec<PeerId> {
        let now = Instant::now();
        let mut recent: Vec<(u8, PeerId)> = self
            .levels
            .iter()
            .filter(|(_, (level, seen))| {
                *level <= SPEECH_LEVEL_MAX && now.duration_since(*seen) <= ACTIVE_WINDOW
            })
            .map(|(peer, (level, _))| (*level, *peer))
            .collect();
        // A lower level is louder. Ties go to the lower peer id, so the choice is stable.
        recent.sort();
        recent
            .into_iter()
            .take(AUDIO_TOP_SPEAKERS as usize)
            .map(|(_, peer)| peer)
            .collect()
    }

    /// Waits for one frame from any participant and forwards it. Returns `false` once no
    /// participant is left. A link that fails is removed, and the others keep going.
    ///
    /// This method is cancel-safe.
    pub async fn step(&mut self) -> Result<bool> {
        loop {
            if self.links.is_empty() {
                return Ok(false);
            }
            let (from, outcome) = self.poll_links().await;
            match outcome {
                Ok(Some(wire)) => {
                    self.forward(from, wire).await;
                    return Ok(true);
                }
                Ok(None) | Err(_) => {
                    self.links.remove(&from);
                    self.stats.links_lost += 1;
                }
            }
        }
    }

    /// Forwards frames until no participant is left.
    pub async fn run(&mut self) -> Result<()> {
        while self.step().await? {}
        Ok(())
    }

    async fn poll_links(&mut self) -> (PeerId, RawOut) {
        let mut futs: Vec<(PeerId, Boxed<'_, RawOut>)> = self
            .links
            .iter_mut()
            .map(|(peer, call)| (*peer, Box::pin(call.next_raw()) as Boxed<'_, _>))
            .collect();
        poll_fn(|cx| {
            for (peer, fut) in futs.iter_mut() {
                if let Poll::Ready(out) = fut.as_mut().poll(cx) {
                    return Poll::Ready((*peer, out));
                }
            }
            Poll::Pending
        })
        .await
    }

    async fn forward(&mut self, from: PeerId, wire: Vec<u8>) {
        if is_digest(&wire) {
            // A digest is signed by its sender and passes through untouched. It is not a
            // speech frame, so it neither counts as a speaker nor moves the speaker levels.
            if self.should_forward(from) {
                self.fan_out(from, &wire).await;
            }
            return;
        }
        self.stats.frames_in += 1;
        if let Some(observer) = self.observer.as_mut() {
            observer(from, &wire);
        }
        let level = audio_level(&wire).unwrap_or(127);
        self.levels.insert(from, (level, Instant::now()));

        if !self.should_forward(from) {
            self.stats.frames_dropped += 1;
            return;
        }
        self.fan_out(from, &wire).await;
    }

    /// Writes one wire packet to every participant except its sender.
    async fn fan_out(&mut self, from: PeerId, wire: &[u8]) {
        let targets: Vec<PeerId> = self.links.keys().copied().filter(|p| *p != from).collect();
        for target in targets {
            let Some(call) = self.links.get_mut(&target) else {
                continue;
            };
            match call.send_encrypted(wire).await {
                Ok(()) => self.stats.frames_forwarded += 1,
                // The link is still connecting, so this copy cannot go yet.
                Err(MediaError::NotConnected) => self.stats.frames_dropped += 1,
                Err(e) => {
                    tracing::debug!(target, error = %e, "dropping forwarder link after a send failure");
                    self.links.remove(&target);
                    self.stats.links_lost += 1;
                }
            }
        }
    }

    fn should_forward(&self, from: PeerId) -> bool {
        match self.mode {
            Mode::Video720p => true,
            Mode::AudioTop3 => self.top_speakers().contains(&from),
        }
    }
}
