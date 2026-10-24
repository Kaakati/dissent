//! A 1:1 media session: one str0m `Rtc` over a [`PacketTransport`], with SFrame applied to every
//! frame.
//!
//! A call can also be keyless. A keyless call is the forwarder's side of a link. It carries the
//! same encrypted frames without holding a key, so it can forward them but never read them.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::auth::{frame_hash, is_digest, Auth, FrameSigner, LinkConfig, RxState, Step};
use crate::bwe::BandwidthEstimator;
use crate::e2ee::{
    epoch_lsb, peek_counter, redundancy_for, SframeReceiver, SframeSender, DEFAULT_LEVEL,
    EPOCH_GRACE,
};
use crate::error::{MediaError, Result};
use crate::frame::{received, ReceivedFrame};
use crate::key::MediaKey;
use crate::signal::Signal;
use crate::transport::{PacketTransport, UdpTransport};

/// How often str0m reports peer and media statistics.
const STATS_INTERVAL: Duration = Duration::from_millis(500);

/// Counters and the latest measurements of a call.
///
/// Packet counts cover every datagram of the session, including ICE and RTCP.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CallStats {
    /// Round-trip time in milliseconds, once str0m has measured it.
    pub rtt_ms: Option<f64>,
    /// Datagrams this side sent.
    pub packets_sent: u64,
    /// Datagrams this side received.
    pub packets_received: u64,
    /// Packets the remote peer reports as lost, from its receiver reports.
    pub packets_lost: u64,
    /// Audio frames this side encrypted and sent.
    pub frames_sent: u64,
    /// Audio frames that decrypted and were delivered.
    pub frames_received: u64,
    /// Incoming frames that failed SFrame decryption and were dropped.
    pub frames_undecryptable: u64,
    /// Incoming frames rejected as replays, or as older than the replay window.
    pub frames_replayed: u64,
    /// Frames that no digest verified in time, or that a digest never covered. A lost digest
    /// causes this, so it is tolerated and mutes no one.
    pub frames_unauthenticated: u64,
    /// Times a sender's frame did not match its signed digest. Each one mutes that sender for
    /// the rest of the epoch.
    pub impersonation_detected: u64,
}

/// The keys of a call that has them.
struct Keys {
    tx: SframeSender,
    rx: SframeReceiver,
}

/// One side of a 1:1 call.
///
/// Create it with [`Call::start_offer`] or [`Call::accept`] (UDP), or with [`Call::offer_over`] or
/// [`Call::accept_over`] for any [`PacketTransport`]. Move the signals between the peers with any
/// transport, then call [`Call::apply`]. Drive the session with [`Call::next_frame`].
/// str0m does not run its own timers, so `next_frame` and `send_audio_frame` must keep being
/// called while the call is alive.
pub struct Call<T: PacketTransport = UdpTransport> {
    rtc: Rtc,
    transport: T,
    keys: Option<Keys>,
    signer: Option<FrameSigner>,
    state: Arc<Mutex<RxState>>,
    pending_offer: Option<SdpPendingOffer>,
    audio_mid: Option<Mid>,
    started: Instant,
    timeout: Instant,
    inbox: VecDeque<ReceivedFrame>,
    raw_inbox: VecDeque<Vec<u8>>,
    /// Whether media on this link is consumed. A standby link stays connected but does not listen.
    listening: bool,
    bwe: BandwidthEstimator,
    bitrate_cb: Option<Box<dyn FnMut(u32) + Send>>,
    loss: f32,
    red_override: Option<bool>,
    stats: CallStats,
}

// Keeps Call movable across tasks. A type that is not Send fails to compile here.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Call>();
};

/// Smallest MTU str0m accepts as a target.
const MTU_MIN: usize = 576;
/// Largest MTU str0m accepts as a target.
const MTU_MAX: usize = 1500;

fn new_rtc(start: Instant, max_datagram: Option<usize>) -> Result<Rtc> {
    let mut config = Rtc::builder()
        .clear_codecs()
        .enable_opus(true, false)
        .set_stats_interval(Some(STATS_INTERVAL));
    if let Some(max) = max_datagram {
        if !(MTU_MIN..=MTU_MAX).contains(&max) {
            return Err(MediaError::BadState(
                "transport datagram size is outside the MTU range",
            ));
        }
        config = config.set_mtu(max..=max);
    }
    Ok(config.build(start))
}

fn add_host_candidate(rtc: &mut Rtc, local: SocketAddr) -> Result<()> {
    let candidate = Candidate::host(local, "udp").map_err(|e| MediaError::Ice(e.to_string()))?;
    rtc.add_local_candidate(candidate);
    Ok(())
}

impl Call<UdpTransport> {
    /// Binds `bind` over UDP and creates the caller's offer. Send the returned [`Signal::Offer`]
    /// to the peer, then apply its answer with [`Call::apply`].
    ///
    /// `bind` should be `127.0.0.1:0` in tests. `sender_id` is this side's MLS member index.
    pub async fn start_offer(
        bind: SocketAddr,
        key: MediaKey,
        sender_id: u64,
    ) -> Result<(Self, Signal)> {
        Self::offer_over(UdpTransport::bind(bind).await?, key, sender_id).await
    }

    /// Binds `bind` over UDP and answers the caller's [`Signal::Offer`]. Send the returned
    /// [`Signal::Answer`] back to the caller.
    pub async fn accept(
        bind: SocketAddr,
        key: MediaKey,
        sender_id: u64,
        offer: Signal,
    ) -> Result<(Self, Signal)> {
        Self::accept_over(UdpTransport::bind(bind).await?, key, sender_id, offer).await
    }
}

impl<T: PacketTransport> Call<T> {
    /// Creates the caller's offer over `transport`, at key epoch 0. Its
    /// [`PacketTransport::local_addr`] becomes the host candidate.
    pub async fn offer_over(transport: T, key: MediaKey, sender_id: u64) -> Result<(Self, Signal)> {
        Self::offer_at(transport, key, sender_id, 0, LinkConfig::plain()).await
    }

    /// Answers the caller's [`Signal::Offer`] over `transport`, at key epoch 0.
    pub async fn accept_over(
        transport: T,
        key: MediaKey,
        sender_id: u64,
        offer: Signal,
    ) -> Result<(Self, Signal)> {
        Self::accept_at(transport, key, sender_id, 0, offer, LinkConfig::plain()).await
    }

    /// Like [`Call::offer_over`], with the frames keyed to `epoch` and the given link
    /// configuration. A mesh and a participant pass their shared state through `link`.
    pub(crate) async fn offer_at(
        transport: T,
        key: MediaKey,
        sender_id: u64,
        epoch: u64,
        link: LinkConfig,
    ) -> Result<(Self, Signal)> {
        let keys = Some(Keys {
            tx: SframeSender::new(&key, sender_id, epoch)?,
            rx: SframeReceiver::new(key, epoch),
        });
        let signer = Self::signer_for(&link, sender_id, epoch);
        link.state
            .lock()
            .expect("receiver state")
            .accept_epoch(epoch_lsb(epoch));
        Self::offer_with(transport, keys, signer, link.state).await
    }

    /// Like [`Call::accept_over`], with the frames keyed to `epoch` and the given link
    /// configuration.
    pub(crate) async fn accept_at(
        transport: T,
        key: MediaKey,
        sender_id: u64,
        epoch: u64,
        offer: Signal,
        link: LinkConfig,
    ) -> Result<(Self, Signal)> {
        let keys = Some(Keys {
            tx: SframeSender::new(&key, sender_id, epoch)?,
            rx: SframeReceiver::new(key, epoch),
        });
        let signer = Self::signer_for(&link, sender_id, epoch);
        link.state
            .lock()
            .expect("receiver state")
            .accept_epoch(epoch_lsb(epoch));
        Self::answer(transport, keys, signer, link.state, offer).await
    }

    /// Offers as a keyless call. A forwarder uses it for its upstream link.
    pub(crate) async fn offer_blind(transport: T) -> Result<(Self, Signal)> {
        let state = Arc::new(Mutex::new(RxState::new(None)));
        Self::offer_with(transport, None, None, state).await
    }

    /// Offers a call whose frames are signed and whose members' digests are checked, at key
    /// epoch 0. This is the 1:1 call with sender authentication.
    pub async fn offer_authenticated(
        transport: T,
        key: MediaKey,
        sender_id: u64,
        auth: Auth,
    ) -> Result<(Self, Signal)> {
        let link = LinkConfig::authenticated(auth.clone(), LinkConfig::shared_state(&auth));
        Self::offer_at(transport, key, sender_id, 0, link).await
    }

    /// Answers an offer as an authenticated 1:1 call, at key epoch 0.
    pub async fn accept_authenticated(
        transport: T,
        key: MediaKey,
        sender_id: u64,
        auth: Auth,
        offer: Signal,
    ) -> Result<(Self, Signal)> {
        let link = LinkConfig::authenticated(auth.clone(), LinkConfig::shared_state(&auth));
        Self::accept_at(transport, key, sender_id, 0, offer, link).await
    }

    fn signer_for(link: &LinkConfig, sender_id: u64, epoch: u64) -> Option<FrameSigner> {
        link.auth.as_ref().map(|auth| {
            FrameSigner::new(
                auth.device.clone(),
                sender_id,
                epoch_lsb(epoch) as u8,
                auth.digest_frames,
            )
        })
    }

    async fn offer_with(
        transport: T,
        keys: Option<Keys>,
        signer: Option<FrameSigner>,
        state: Arc<Mutex<RxState>>,
    ) -> Result<(Self, Signal)> {
        let mut rtc = new_rtc(Instant::now(), transport.max_datagram_size())?;
        add_host_candidate(&mut rtc, transport.local_addr())?;

        let mut changes = rtc.sdp_api();
        let mid = changes.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
        let (offer, pending) = changes
            .apply()
            .ok_or(MediaError::BadState("nothing to offer"))?;

        let mut call = Self::from_parts(rtc, transport, keys, signer, state);
        call.pending_offer = Some(pending);
        call.audio_mid = Some(mid);
        Ok((call, Signal::Offer(offer.to_sdp_string())))
    }

    /// Answers an offer with a keyless call. It forwards encrypted frames and cannot read them.
    pub(crate) async fn accept_blind(transport: T, offer: Signal) -> Result<(Self, Signal)> {
        let state = Arc::new(Mutex::new(RxState::new(None)));
        Self::answer(transport, None, None, state, offer).await
    }

    async fn answer(
        transport: T,
        keys: Option<Keys>,
        signer: Option<FrameSigner>,
        state: Arc<Mutex<RxState>>,
        offer: Signal,
    ) -> Result<(Self, Signal)> {
        let Signal::Offer(sdp) = offer else {
            return Err(MediaError::BadState("accept needs an offer"));
        };
        let offer =
            SdpOffer::from_sdp_string(&sdp).map_err(|e| MediaError::BadSignal(e.to_string()))?;

        let mut rtc = new_rtc(Instant::now(), transport.max_datagram_size())?;
        add_host_candidate(&mut rtc, transport.local_addr())?;
        let answer = rtc.sdp_api().accept_offer(offer)?;

        // The audio track appears through an event once the offer is applied, see `on_event`.
        let call = Self::from_parts(rtc, transport, keys, signer, state);
        Ok((call, Signal::Answer(answer.to_sdp_string())))
    }

    fn from_parts(
        rtc: Rtc,
        transport: T,
        keys: Option<Keys>,
        signer: Option<FrameSigner>,
        state: Arc<Mutex<RxState>>,
    ) -> Self {
        let now = Instant::now();
        Self {
            rtc,
            transport,
            keys,
            signer,
            state,
            pending_offer: None,
            audio_mid: None,
            started: now,
            timeout: now,
            inbox: VecDeque::new(),
            raw_inbox: VecDeque::new(),
            listening: true,
            bwe: BandwidthEstimator::new(),
            bitrate_cb: None,
            loss: 0.0,
            red_override: None,
            stats: CallStats::default(),
        }
    }

    /// Applies the peer's answer or a remote candidate.
    pub fn apply(&mut self, signal: Signal) -> Result<()> {
        match signal {
            Signal::Answer(sdp) => {
                let answer = SdpAnswer::from_sdp_string(&sdp)
                    .map_err(|e| MediaError::BadSignal(e.to_string()))?;
                let pending = self
                    .pending_offer
                    .take()
                    .ok_or(MediaError::BadState("no pending offer to answer"))?;
                self.rtc.sdp_api().accept_answer(pending, answer)?;
                Ok(())
            }
            Signal::Candidate(text) => {
                let candidate = Candidate::from_sdp_string(&text)
                    .map_err(|e| MediaError::Ice(e.to_string()))?;
                self.rtc.add_remote_candidate(candidate);
                Ok(())
            }
            Signal::Offer(_) => Err(MediaError::BadState("an offer cannot be applied to a call")),
        }
    }

    /// The transport this call sends and receives on.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport's local address, which is this side's host candidate.
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }

    /// Whether ICE and DTLS have completed, so media can flow.
    pub fn is_connected(&self) -> bool {
        self.rtc.is_connected()
    }

    /// Drives the session until ICE and DTLS complete. Frames that arrive meanwhile are kept
    /// for [`Call::next_frame`].
    pub async fn wait_connected(&mut self) -> Result<()> {
        loop {
            self.drain().await?;
            if self.rtc.is_connected() {
                return Ok(());
            }
            if !self.rtc.is_alive() {
                return Err(MediaError::Closed);
            }
            self.pump_once().await?;
        }
    }

    /// Encrypts one audio frame with SFrame and sends it as an Opus payload.
    ///
    /// Fails with [`MediaError::NotConnected`] before ICE and DTLS complete, because str0m drops
    /// media written earlier. Fails with [`MediaError::BadState`] on a keyless call.
    pub async fn send_audio_frame(&mut self, frame: &[u8]) -> Result<()> {
        self.send_frame(frame, None).await
    }

    /// Like [`Call::send_audio_frame`], but stamps the frame with `timestamp`, the sender's
    /// sample clock at 48 kHz. Use it when the capture clock is not the frame count.
    pub async fn send_audio_frame_at(&mut self, frame: &[u8], timestamp: u32) -> Result<()> {
        self.send_frame(frame, Some(timestamp)).await
    }

    async fn send_frame(&mut self, frame: &[u8], timestamp: Option<u32>) -> Result<()> {
        self.check_sendable()?;
        let keys = self
            .keys
            .as_mut()
            .ok_or(MediaError::BadState("a keyless call cannot encrypt"))?;
        let on = self
            .red_override
            .unwrap_or_else(|| redundancy_for(self.loss, keys.tx.red()));
        keys.tx.set_red(on);
        let encrypted = keys.tx.encrypt(frame, timestamp, DEFAULT_LEVEL)?;
        self.write_encrypted(encrypted).await
    }

    /// Sends a frame that the caller has already encrypted. A mesh encrypts once and sends the
    /// result on every call, and a forwarder sends the frames it relays.
    pub(crate) async fn send_encrypted(&mut self, encrypted: &[u8]) -> Result<()> {
        self.check_sendable()?;
        self.write_encrypted(encrypted.to_vec()).await
    }

    /// Accepts frames of `epoch` from now on. The previous epoch keeps decrypting for the grace
    /// period, see [`EPOCH_GRACE`]. Digests of the new epoch are signed under its key id bits.
    pub(crate) fn rotate_keys(&mut self, key: MediaKey, epoch: u64, now: Instant) -> Result<()> {
        let keys = self
            .keys
            .as_mut()
            .ok_or(MediaError::BadState("a keyless call cannot rotate keys"))?;
        keys.rx.rotate(key, epoch, now, EPOCH_GRACE)?;
        // Digests of the new epoch are accepted from now on. The old one keeps its grace period.
        self.state
            .lock()
            .expect("receiver state")
            .accept_epoch(epoch_lsb(epoch));
        if let Some(signer) = self.signer.as_mut() {
            signer.set_epoch(epoch_lsb(epoch) as u8);
        }
        Ok(())
    }

    fn check_sendable(&self) -> Result<()> {
        if !self.rtc.is_connected() {
            return Err(MediaError::NotConnected);
        }
        if self.audio_mid.is_none() {
            return Err(MediaError::NoAudioTrack);
        }
        Ok(())
    }

    /// Writes one encrypted frame, and any digest that its batch closes.
    async fn write_encrypted(&mut self, encrypted: Vec<u8>) -> Result<()> {
        let digests = match (self.signer.as_mut(), peek_counter(&encrypted)) {
            (Some(signer), Some(counter)) => signer.record(counter, &encrypted, Instant::now()),
            _ => Vec::new(),
        };
        self.stats.frames_sent += 1;
        self.write_raw(encrypted).await?;
        for digest in digests {
            self.write_raw(digest).await?;
        }
        Ok(())
    }

    /// Writes one payload on the audio track, without counting it as a frame.
    async fn write_raw(&mut self, payload: Vec<u8>) -> Result<()> {
        let mid = self.audio_mid.ok_or(MediaError::NoAudioTrack)?;
        let pt = self.opus_pt().ok_or(MediaError::NoAudioTrack)?;
        let media_time = self.started.elapsed().into();
        let writer = self.rtc.writer(mid).ok_or(MediaError::NoAudioTrack)?;
        writer.write(pt, Instant::now(), media_time, payload)?;
        self.drain().await
    }

    /// Waits for the next decrypted frame and returns `(sender id, plaintext)`.
    ///
    /// Returns `Ok(None)` once the call has closed. Frames that fail decryption are counted in
    /// [`CallStats::frames_undecryptable`] and are not returned.
    ///
    /// This method is cancel-safe. Dropping its future between awaits loses no received frame.
    pub async fn next_frame(&mut self) -> Result<Option<(u64, Vec<u8>)>> {
        Ok(self
            .next_received_frame()
            .await?
            .map(|frame| (frame.sender, frame.payload)))
    }

    /// Waits for the next decrypted frame, with its sequence number, timestamp and audio level.
    ///
    /// Returns `Ok(None)` once the call has closed. Frames that fail decryption are counted in
    /// [`CallStats::frames_undecryptable`] and are not returned. Cancel-safe, like
    /// [`Call::next_frame`].
    pub async fn next_received_frame(&mut self) -> Result<Option<ReceivedFrame>> {
        loop {
            if let Some(frame) = self.inbox.pop_front() {
                return Ok(Some(frame));
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
            self.drain().await?;
            if self.inbox.is_empty() {
                self.pump_once().await?;
            }
        }
    }

    /// Waits for the next encrypted frame, exactly as it arrived, for a keyless call. Cancel-safe.
    pub(crate) async fn next_raw(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(frame) = self.raw_inbox.pop_front() {
                return Ok(Some(frame));
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
            self.drain().await?;
            if self.raw_inbox.is_empty() {
                self.pump_once().await?;
            }
        }
    }

    /// Registers the callback that receives the target audio bitrate, in kbps, each time it
    /// changes. The audio pipeline uses it to set Opus's bitrate, which stays within 16 to 64.
    pub fn on_target_bitrate(&mut self, callback: impl FnMut(u32) + Send + 'static) {
        self.bitrate_cb = Some(Box::new(callback));
    }

    /// The current target audio bitrate in kbps, from the bandwidth estimate.
    pub fn target_bitrate_kbps(&self) -> u32 {
        self.bwe.target_kbps()
    }

    /// Forces audio redundancy on or off. `None` lets it follow the packet loss the peer reports,
    /// which turns it on above 10% loss.
    pub fn set_redundancy(&mut self, on: Option<bool>) {
        self.red_override = on;
    }

    /// The loss the peer reports on this call's media, as a fraction from 0 to 1.
    pub fn loss_fraction(&self) -> f32 {
        self.loss
    }

    /// Feeds a round-trip sample to the estimator, and tells the callback when the target moves.
    fn update_bitrate(&mut self, rtt_ms: f64) {
        let before = self.bwe.target_kbps();
        let target = self.bwe.update(rtt_ms, f64::from(self.loss));
        if target != before {
            if let Some(callback) = self.bitrate_cb.as_mut() {
                callback(target);
            }
        }
    }

    /// A snapshot of the call's counters.
    pub fn stats(&self) -> CallStats {
        self.stats
    }

    fn opus_pt(&self) -> Option<Pt> {
        self.rtc
            .codec_config()
            .find(|p| p.spec().codec == Codec::Opus)
            .map(|p| p.pt())
    }

    /// Transmits every queued datagram and handles every queued event. Stops at str0m's next
    /// timeout, which it records for [`Call::pump_once`].
    async fn drain(&mut self) -> Result<()> {
        loop {
            match self.rtc.poll_output()? {
                Output::Timeout(at) => {
                    self.timeout = at;
                    return Ok(());
                }
                Output::Transmit(transmit) => {
                    self.stats.packets_sent += 1;
                    self.transport
                        .send(transmit.destination, &transmit.contents)
                        .await?;
                }
                Output::Event(event) => self.on_event(event),
            }
        }
    }

    /// Waits for one of three things: a datagram from the transport, str0m's timeout, or the
    /// close of the open digest batch. It then acts on that event and returns, so the caller
    /// drains again.
    async fn pump_once(&mut self) -> Result<()> {
        let deadline = tokio::time::Instant::from_std(self.timeout);
        let (has_batch, batch_due) = match self.signer.as_ref().and_then(FrameSigner::due) {
            Some(due) => (true, tokio::time::Instant::from_std(due)),
            None => (false, deadline),
        };
        tokio::select! {
            received = self.transport.recv() => {
                let (source, bytes) = match received {
                    Ok(v) => v,
                    // Windows reports an earlier ICMP port-unreachable as ConnectionReset here.
                    Err(MediaError::Io(e)) if e.kind() == ErrorKind::ConnectionReset => return Ok(()),
                    Err(e) => return Err(e),
                };
                self.stats.packets_received += 1;
                let local = self.transport.local_addr();
                match Receive::new(Protocol::Udp, source, local, &bytes) {
                    Ok(receive) => {
                        self.rtc.handle_input(Input::Receive(Instant::now(), receive))?;
                    }
                    Err(e) => tracing::debug!(error = %e, "dropped malformed datagram"),
                }
                Ok(())
            }
            _ = tokio::time::sleep_until(deadline) => {
                self.rtc.handle_input(Input::Timeout(Instant::now()))?;
                Ok(())
            }
            _ = tokio::time::sleep_until(batch_due), if has_batch => {
                let digests = self.signer.as_mut().map(FrameSigner::flush).unwrap_or_default();
                for digest in digests {
                    self.write_raw(digest).await?;
                }
                Ok(())
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::MediaAdded(added) if matches!(added.kind, MediaKind::Audio) => {
                self.audio_mid = Some(added.mid);
            }
            Event::MediaData(data) => self.on_media(&data.data),
            Event::PeerStats(stats) => {
                if let Some(rtt) = stats.rtt {
                    let rtt_ms = rtt.as_secs_f64() * 1000.0;
                    self.stats.rtt_ms = Some(rtt_ms);
                    self.update_bitrate(rtt_ms);
                }
            }
            Event::MediaEgressStats(stats) => {
                if let Some(remote) = stats.remote {
                    self.stats.packets_lost = remote.packets_lost;
                }
                if let Some(loss) = stats.loss {
                    self.loss = loss;
                }
            }
            _ => {}
        }
    }

    /// Turns media consumption on or off for this link. A standby link keeps its handshake and
    /// timers, but its media is dropped before any state sees it, so the copy it carries cannot
    /// stand in for the active link's copy.
    pub(crate) fn set_listening(&mut self, listening: bool) {
        self.listening = listening;
    }

    fn on_media(&mut self, wire: &[u8]) {
        if !self.listening {
            return;
        }
        let now = Instant::now();
        if self.keys.is_none() {
            // A keyless call hands the encrypted frame on untouched, digests included.
            self.stats.frames_received += 1;
            self.raw_inbox.push_back(wire.to_vec());
            return;
        }
        if is_digest(wire) {
            let step = self
                .state
                .lock()
                .expect("receiver state")
                .on_digest(wire, now);
            self.absorb(step);
            return;
        }
        let Some(keys) = self.keys.as_mut() else {
            return;
        };
        if let Some(ended) = keys.rx.expire_previous(now) {
            self.state.lock().expect("receiver state").drop_epoch(ended);
        }
        match keys.rx.open(wire, now) {
            Ok(opened) => {
                let mut step = Step::default();
                {
                    let mut state = self.state.lock().expect("receiver state");
                    if let Some(recovered) = opened.recovered {
                        step.extend(state.on_redundant(
                            opened.sender,
                            opened.lsb,
                            recovered.counter,
                            recovered.hash,
                            recovered.plaintext,
                            now,
                        ));
                    }
                    step.extend(state.on_frame(
                        opened.sender,
                        opened.lsb,
                        opened.counter,
                        frame_hash(wire),
                        opened.plaintext,
                        now,
                    ));
                }
                self.absorb(step);
            }
            Err(e) => {
                self.stats.frames_undecryptable += 1;
                tracing::debug!(error = %e, "dropped undecryptable frame");
            }
        }
    }

    /// Records what a receiver step did, and queues the frames it plays.
    fn absorb(&mut self, step: Step) {
        self.stats.frames_replayed += step.replayed;
        self.stats.frames_unauthenticated += step.unauthenticated;
        self.stats.impersonation_detected += step.impersonations;
        self.stats.frames_received += step.deliver.len() as u64;
        for (sender, stamped) in step.deliver {
            match received(sender, &stamped) {
                Ok(frame) => self.inbox.push_back(frame),
                Err(e) => tracing::debug!(error = %e, "dropped a malformed frame"),
            }
        }
    }
}
