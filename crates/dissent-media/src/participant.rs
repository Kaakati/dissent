//! A call participant that sends through a peer-SFU forwarder, with a pre-warmed backup.
//!
//! The participant keeps a link to every forwarder in its pool. Only the active forwarder gets
//! its frames, so the backup stays idle. If the active forwarder closes its link, fails a send,
//! or stays silent for [`SILENCE_TIMEOUT`], the participant switches to the backup and elects a
//! new backup from the rest of the pool.

use std::collections::BTreeMap;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::auth::{Auth, LinkConfig, RxState};
use crate::call::{Call, CallStats};
use crate::e2ee::SframeSender;
use crate::election::{elect, ranked, Candidate, Election, Mode, PeerId};
use crate::error::{MediaError, Result};
use crate::key::MediaKey;
use crate::signal::Signal;
use crate::transport::PacketTransport;

/// How long the active forwarder may stay silent before the participant fails over.
pub const SILENCE_TIMEOUT: Duration = Duration::from_millis(500);

/// Why the participant left a forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverReason {
    /// The link closed or failed while receiving.
    LinkError,
    /// A send through the link failed.
    SendFailed,
    /// No frame arrived from the active forwarder within [`SILENCE_TIMEOUT`].
    Silence,
}

/// One switch of the active forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failover {
    /// The forwarder the participant left.
    pub from: PeerId,
    /// The forwarder it switched to, or `None` when the pool has none left.
    pub to: Option<PeerId>,
    /// Why it switched.
    pub reason: FailoverReason,
    /// When it switched.
    pub at: Instant,
}

type Boxed<'a, O> = Pin<Box<dyn Future<Output = O> + Send + 'a>>;
type FrameOut = Result<Option<(u64, Vec<u8>)>>;

/// One participant of a forwarded call.
pub struct Participant<T: PacketTransport> {
    my_id: PeerId,
    key: MediaKey,
    tx: SframeSender,
    mode: Mode,
    participants: usize,
    candidates: Vec<Candidate>,
    links: BTreeMap<PeerId, Call<T>>,
    link: LinkConfig,
    active: Option<PeerId>,
    backup: Option<PeerId>,
    last_active_rx: Instant,
    failovers: Vec<Failover>,
    rotation: usize,
}

impl<T: PacketTransport> Participant<T> {
    /// A participant with no forwarder links yet.
    ///
    /// `candidates` is the gossiped set of peers that could forward. `participants` is the size
    /// of the call, which the election uses to check capacity.
    pub fn new(
        media_key: MediaKey,
        my_id: PeerId,
        candidates: Vec<Candidate>,
        participants: usize,
        mode: Mode,
    ) -> Result<Self> {
        let link = LinkConfig {
            auth: None,
            state: Arc::new(Mutex::new(RxState::new(None))),
        };
        Self::build(media_key, my_id, candidates, participants, mode, link)
    }

    /// A participant that signs its frames and checks the digests of every candidate's
    /// members. `auth` holds this participant's device key and every member's certificate.
    pub fn new_authenticated(
        media_key: MediaKey,
        my_id: PeerId,
        candidates: Vec<Candidate>,
        participants: usize,
        mode: Mode,
        auth: Auth,
    ) -> Result<Self> {
        let link = LinkConfig::authenticated(auth.clone(), LinkConfig::shared_state(&auth));
        Self::build(media_key, my_id, candidates, participants, mode, link)
    }

    fn build(
        media_key: MediaKey,
        my_id: PeerId,
        candidates: Vec<Candidate>,
        participants: usize,
        mode: Mode,
        link: LinkConfig,
    ) -> Result<Self> {
        let tx = SframeSender::new(&media_key, my_id, 0)?;
        Ok(Self {
            my_id,
            key: media_key,
            tx,
            mode,
            participants,
            candidates,
            links: BTreeMap::new(),
            link,
            active: None,
            backup: None,
            last_active_rx: Instant::now(),
            failovers: Vec::new(),
            rotation: 0,
        })
    }

    /// Opens a link to the forwarder `sfu`, and returns the offer to send to it.
    pub async fn offer_sfu(&mut self, sfu: PeerId, transport: T) -> Result<Signal> {
        let (call, offer) = Call::offer_at(
            transport,
            self.key.clone(),
            self.my_id,
            0,
            self.link.clone(),
        )
        .await?;
        self.links.insert(sfu, call);
        Ok(offer)
    }

    /// Applies the forwarder's answer to the offer for `sfu`.
    pub fn apply_sfu(&mut self, sfu: PeerId, answer: Signal) -> Result<()> {
        self.links
            .get_mut(&sfu)
            .ok_or(MediaError::BadState("no link to this forwarder"))?
            .apply(answer)
    }

    /// Sends through `primary`, with `backup` standing by. Both must already be linked. A cascade
    /// uses this to give each participant its assigned child forwarder.
    pub fn assign_forwarders(&mut self, primary: PeerId, backup: Option<PeerId>) -> Result<()> {
        if !self.links.contains_key(&primary) {
            return Err(MediaError::BadState("no link to the primary forwarder"));
        }
        self.active = Some(primary);
        self.backup = backup.filter(|standby| self.links.contains_key(standby));
        self.sync_listening();
        Ok(())
    }

    /// Only the active link's media is consumed. The others stay connected, so a failover needs
    /// no handshake, and their copies of the same frames are not counted twice.
    fn sync_listening(&mut self) {
        let active = self.active;
        for (peer, call) in self.links.iter_mut() {
            call.set_listening(Some(*peer) == active);
        }
    }

    /// Chooses the active forwarder and the backup from the pool of linked candidates.
    pub fn elect(&mut self) -> Result<()> {
        let alive = self.alive_candidates();
        let next = elect(&alive, self.participants, self.mode).ok_or(MediaError::NoSfu)?;
        self.active = Some(next.primary);
        self.backup = next.backup;
        self.sync_listening();
        Ok(())
    }

    /// Waits until every forwarder link has connected. Frames that arrive meanwhile are kept.
    pub async fn wait_connected(&mut self) -> Result<()> {
        let mut pending: Vec<Option<Boxed<'_, Result<()>>>> = self
            .links
            .values_mut()
            .map(|call| Some(Box::pin(call.wait_connected()) as Boxed<'_, Result<()>>))
            .collect();
        let result = poll_fn(|cx| {
            let mut all_done = true;
            for slot in pending.iter_mut() {
                if let Some(fut) = slot.as_mut() {
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(Ok(())) => *slot = None,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => all_done = false,
                    }
                }
            }
            if all_done {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await;
        self.last_active_rx = Instant::now();
        result
    }

    /// The active forwarder.
    pub fn active(&self) -> Option<PeerId> {
        self.active
    }

    /// The standby forwarder.
    pub fn backup(&self) -> Option<PeerId> {
        self.backup
    }

    /// Every switch of the active forwarder so far, oldest first.
    pub fn failovers(&self) -> &[Failover] {
        &self.failovers
    }

    /// The forwarder links still open.
    pub fn linked(&self) -> Vec<PeerId> {
        self.links.keys().copied().collect()
    }

    /// The counters of each forwarder link.
    pub fn link_stats(&self) -> BTreeMap<PeerId, CallStats> {
        self.links
            .iter()
            .map(|(peer, call)| (*peer, call.stats()))
            .collect()
    }

    /// Encrypts one frame once and sends it through the active forwarder.
    ///
    /// If the send fails, the participant fails over and retries on the new active forwarder.
    /// A frame sent while that forwarder is still connecting is dropped, and the call reports
    /// success for it. Fails with [`MediaError::NoSfu`] when no forwarder is left.
    pub async fn send_audio_frame(&mut self, frame: &[u8], level: u8) -> Result<()> {
        let wire = self.tx.encrypt(frame, level)?;
        loop {
            let active = self.active.ok_or(MediaError::NoSfu)?;
            if !self.links.contains_key(&active) {
                self.fail_over(active, FailoverReason::LinkError);
                continue;
            }
            let call = self
                .links
                .get_mut(&active)
                .expect("the link was checked above");
            match call.send_encrypted(&wire).await {
                Ok(()) => return Ok(()),
                Err(MediaError::NotConnected) => return Ok(()),
                Err(e) => {
                    tracing::debug!(sfu = active, error = %e, "forwarder send failed");
                    self.fail_over(active, FailoverReason::SendFailed);
                }
            }
        }
    }

    /// Waits for the next frame from the call and returns `(sender id, plaintext)`.
    ///
    /// Returns `Ok(None)` once no forwarder link is left. Failover runs here, because this is
    /// where link errors and silence are noticed. This method is cancel-safe.
    pub async fn next_frame(&mut self) -> Result<Option<(PeerId, Vec<u8>)>> {
        loop {
            if self.links.is_empty() {
                return Ok(None);
            }
            let event = if self.active.is_some() {
                let deadline =
                    tokio::time::Instant::from_std(self.last_active_rx + SILENCE_TIMEOUT);
                tokio::select! {
                    ev = self.poll_links() => Some(ev),
                    _ = tokio::time::sleep_until(deadline) => None,
                }
            } else {
                Some(self.poll_links().await)
            };

            match event {
                Some((sfu, Ok(Some(frame)))) => {
                    if Some(sfu) == self.active {
                        self.last_active_rx = Instant::now();
                    }
                    return Ok(Some(frame));
                }
                Some((sfu, Ok(None) | Err(_))) => self.drop_link(sfu),
                None => {
                    if let Some(active) = self.active {
                        self.fail_over(active, FailoverReason::Silence);
                    }
                }
            }
        }
    }

    /// Polls every link from a rotating position, so a busy link cannot starve a standby.
    async fn poll_links(&mut self) -> (PeerId, FrameOut) {
        let mut futs: Vec<(PeerId, Boxed<'_, FrameOut>)> = self
            .links
            .iter_mut()
            .map(|(peer, call)| (*peer, Box::pin(call.next_frame()) as Boxed<'_, FrameOut>))
            .collect();
        let count = futs.len();
        let start = self.rotation % count.max(1);
        self.rotation = self.rotation.wrapping_add(1);
        poll_fn(|cx| {
            for step in 0..count {
                let (peer, fut) = &mut futs[(start + step) % count];
                if let Poll::Ready(out) = fut.as_mut().poll(cx) {
                    return Poll::Ready((*peer, out));
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Removes a link that closed. If it was the active forwarder, the participant fails over.
    fn drop_link(&mut self, sfu: PeerId) {
        if Some(sfu) == self.active {
            self.fail_over(sfu, FailoverReason::LinkError);
        } else {
            self.links.remove(&sfu);
            self.reelect_backup();
        }
    }

    /// Leaves `failed` for good: removes its link, promotes the best remaining forwarder, and
    /// elects a new backup from what is left.
    fn fail_over(&mut self, failed: PeerId, reason: FailoverReason) {
        self.links.remove(&failed);
        let alive = self.alive_candidates();
        let order = ranked(&alive, self.participants, self.mode);
        let to = order.first().copied();
        self.failovers.push(Failover {
            from: failed,
            to,
            reason,
            at: Instant::now(),
        });
        self.active = to;
        self.backup = order.get(1).copied();
        self.last_active_rx = Instant::now();
        self.sync_listening();
    }

    /// Picks the backup again after a standby link closed, keeping the active forwarder.
    fn reelect_backup(&mut self) {
        let order = ranked(&self.alive_candidates(), self.participants, self.mode);
        self.backup = order.into_iter().find(|p| Some(*p) != self.active);
    }

    fn alive_candidates(&self) -> Vec<Candidate> {
        self.candidates
            .iter()
            .filter(|c| self.links.contains_key(&c.peer))
            .cloned()
            .collect()
    }

    /// The election a participant would run now, for tests and diagnostics.
    pub fn current_election(&self) -> Option<Election> {
        elect(&self.alive_candidates(), self.participants, self.mode)
    }

    /// This participant's id.
    pub fn id(&self) -> PeerId {
        self.my_id
    }
}
