//! A call participant that sends through a peer-SFU forwarder, with a pre-warmed backup.
//!
//! The participant keeps a link to every forwarder in its pool. Only the active forwarder gets
//! its frames, so the backup stays idle. If the active forwarder closes its link, fails a send,
//! or stays silent for [`SILENCE_TIMEOUT`], the participant switches to the backup and elects a
//! new backup from the rest of the pool.

use std::collections::{BTreeMap, BTreeSet};
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::auth::{forget_into, learn_into, Auth, LinkConfig, RxState};
use dissent_identity::DeviceCert;
use crate::call::{Call, CallStats};
use crate::e2ee::{epoch_lsb, redundancy_for, SframeSender, DEFAULT_LEVEL};
use crate::election::{elect, ranked, Candidate, Election, Mode, PeerId};
use crate::error::{MediaError, Result};
use crate::frame::ReceivedFrame;
use crate::key::MediaKey;
use crate::signal::Signal;
use crate::transport::PacketTransport;
use crate::video::{VideoFrame, VideoMeta};

/// How long the active forwarder may stay silent before the participant fails over.
pub const SILENCE_TIMEOUT: Duration = Duration::from_millis(500);

/// Frames remembered per sender for dropping duplicates, about 5 s at 20 ms per frame.
const DEDUP_WINDOW: usize = 256;

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
type FrameOut = Result<Option<ReceivedFrame>>;
type VideoOut = Result<Option<VideoFrame>>;

/// One participant of a forwarded call.
pub struct Participant<T: PacketTransport> {
    my_id: PeerId,
    key: MediaKey,
    /// The epoch of `key`. Links opened from now on use it.
    epoch: u64,
    tx: SframeSender,
    video_tx: SframeSender,
    mode: Mode,
    participants: usize,
    candidates: Vec<Candidate>,
    links: BTreeMap<PeerId, Call<T>>,
    link: LinkConfig,
    active: Option<PeerId>,
    backup: Option<PeerId>,
    last_active_rx: Instant,
    /// Sequence numbers already delivered, per sender. A frame can arrive over both forwarders while
    /// a failover switches them, and it must play once.
    delivered: BTreeMap<u64, BTreeSet<u32>>,
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
        let video_tx = SframeSender::new(&media_key.video(), my_id, 0)?;
        Ok(Self {
            my_id,
            key: media_key,
            epoch: 0,
            tx,
            video_tx,
            mode,
            participants,
            candidates,
            links: BTreeMap::new(),
            link,
            active: None,
            backup: None,
            last_active_rx: Instant::now(),
            delivered: BTreeMap::new(),
            failovers: Vec::new(),
            rotation: 0,
        })
    }

    /// Replaces the media key, typically after the MLS group changes. Frames from now on are
    /// encrypted under `new_key` at `new_epoch`. Frames of the previous epoch still decrypt for
    /// the grace period on every forwarder link, and the epoch before that no longer does.
    /// Forwarders hold no key, so a rekey changes nothing for them.
    ///
    /// `new_epoch` must differ from the current epoch in its low key-id bits, which is 8 bits.
    pub fn set_key(&mut self, new_key: MediaKey, new_epoch: u64) -> Result<()> {
        if epoch_lsb(new_epoch) == epoch_lsb(self.epoch) {
            return Err(MediaError::BadState(
                "the new epoch must differ from the current one in its key id bits",
            ));
        }
        let tx = self.tx.rekeyed(&new_key, self.my_id, new_epoch)?;
        let video_tx = self
            .video_tx
            .rekeyed(&new_key.video(), self.my_id, new_epoch)?;
        let now = Instant::now();
        for call in self.links.values_mut() {
            call.rotate_keys(new_key.clone(), new_epoch, now)?;
        }
        self.tx = tx;
        self.video_tx = video_tx;
        self.key = new_key;
        self.epoch = new_epoch;
        Ok(())
    }

    /// Starts verifying `sender`'s digests with its device certificate. Call it when a member
    /// joins. Fails on an unauthenticated participant.
    pub fn learn_cert(&mut self, sender_id: u64, cert: DeviceCert) -> Result<()> {
        learn_into(&self.link.state, sender_id, cert)
    }

    /// Stops verifying `sender_id`'s digests, and forgets its replay state. Call it when a member
    /// leaves. A removed member is cut off from new frames only by a rekey.
    pub fn forget_cert(&mut self, sender_id: u64) {
        forget_into(&self.link.state, sender_id);
    }

    /// Opens a link to the forwarder `sfu`, and returns the offer to send to it.
    pub async fn offer_sfu(&mut self, sfu: PeerId, transport: T) -> Result<Signal> {
        let (call, offer) = Call::offer_at(
            transport,
            self.key.clone(),
            self.my_id,
            self.epoch,
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
    /// The smallest bandwidth estimate among the forwarder links, in kbps. `None` until any link
    /// has an estimate.
    /// Whether the forwarder link agreed to AV1. False without a link.
    pub fn peer_has_av1(&self) -> bool {
        self.active.and_then(|active| self.links.get(&active)).is_some_and(Call::peer_has_av1)
    }

    pub fn video_available_kbps(&self) -> Option<u32> {
        self.links.values().filter_map(Call::video_available_kbps).min()
    }

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
        self.send_frame(frame, None, level).await
    }

    /// Like [`Participant::send_audio_frame`], but stamps the frame with `timestamp`, the
    /// sender's sample clock at 48 kHz.
    pub async fn send_audio_frame_at(
        &mut self,
        frame: &[u8],
        timestamp: u32,
        level: u8,
    ) -> Result<()> {
        self.send_frame(frame, Some(timestamp), level).await
    }

    async fn send_frame(&mut self, frame: &[u8], timestamp: Option<u32>, level: u8) -> Result<()> {
        let loss = self
            .active
            .and_then(|active| self.links.get(&active))
            .map_or(0.0, Call::loss_fraction);
        self.tx.set_red(redundancy_for(loss, self.tx.red()));
        let wire = self.tx.encrypt(frame, timestamp, level)?;
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

    /// Sends one encoded video frame to the active forwarder. Audio decides failover, so a video
    /// send that fails is not retried on another forwarder. The frame is encrypted once, with
    /// the video key.
    pub async fn send_video(&mut self, frame: &[u8], meta: VideoMeta) -> Result<()> {
        let ciphertext = self.video_tx.encrypt(frame, None, DEFAULT_LEVEL)?;
        let Some(active) = self.active else {
            return Err(MediaError::NoSfu);
        };
        let Some(call) = self.links.get_mut(&active) else {
            return Ok(());
        };
        match call
            .send_video_ciphertext(&ciphertext, meta)
            .await
        {
            Ok(()) | Err(MediaError::NotConnected) | Err(MediaError::NoVideoTrack) => Ok(()),
            Err(e) => {
                tracing::debug!(sfu = active, error = %e, "forwarder video send failed");
                Ok(())
            }
        }
    }

    /// Whether the active forwarder has asked for a keyframe since the last call.
    pub fn take_keyframe_request(&mut self) -> bool {
        self.links
            .values_mut()
            .fold(false, |asked, call| call.take_keyframe_request() | asked)
    }

    /// Waits for the next video frame from any forwarder link. Returns `Ok(None)` once no link is
    /// left. Cancel-safe.
    pub async fn next_video_frame(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            if self.links.is_empty() {
                return Ok(None);
            }
            let (peer, outcome) = {
                let mut futs: Vec<(PeerId, Boxed<'_, VideoOut>)> = self
                    .links
                    .iter_mut()
                    .map(|(peer, call)| (*peer, Box::pin(call.next_video_frame()) as Boxed<'_, VideoOut>))
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
            };
            match outcome {
                Ok(Some(frame)) => return Ok(Some(frame)),
                Ok(None) | Err(_) => {
                    self.links.remove(&peer);
                }
            }
        }
    }

    /// Waits for the next frame from the call and returns `(sender id, plaintext)`.
    ///
    /// Returns `Ok(None)` once no forwarder link is left. Failover runs here, because this is
    /// where link errors and silence are noticed. This method is cancel-safe.
    pub async fn next_frame(&mut self) -> Result<Option<(PeerId, Vec<u8>)>> {
        Ok(self
            .next_received_frame()
            .await?
            .map(|frame| (frame.sender, frame.payload)))
    }

    /// Waits for the next frame from the call, with its sequence number, timestamp and level.
    ///
    /// Returns `Ok(None)` once no forwarder link is left. Failover runs here, because this is
    /// where link errors and silence are noticed. Cancel-safe, like [`Participant::next_frame`].
    pub async fn next_received_frame(&mut self) -> Result<Option<ReceivedFrame>> {
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
                    if !self.first_delivery(frame.sender, frame.seq) {
                        continue;
                    }
                    return Ok(Some(frame));
                }
                Some((sfu, Ok(None) | Err(_))) => self.drop_link(sfu),
                None => {
                    // Silence with no other forwarder to move to keeps the link. Dropping it
                    // would end the call, and a slow forwarder can recover on its own.
                    if let Some(active) = self.active {
                        if self.links.len() > 1 {
                            self.fail_over(active, FailoverReason::Silence);
                        } else {
                            self.last_active_rx = Instant::now();
                        }
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
            .map(|(peer, call)| (*peer, Box::pin(call.next_received_frame()) as Boxed<'_, FrameOut>))
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
    /// Records a delivered frame and reports whether it is the first copy of it. Keeps the last
    /// `DEDUP_WINDOW` sequence numbers per sender.
    fn first_delivery(&mut self, sender: u64, seq: u32) -> bool {
        let seen = self.delivered.entry(sender).or_default();
        let fresh = seen.insert(seq);
        while seen.len() > DEDUP_WINDOW {
            seen.pop_first();
        }
        fresh
    }

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
