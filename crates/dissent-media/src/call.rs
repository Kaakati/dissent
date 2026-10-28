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

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, KeyframeRequestKind, MediaKind, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::auth::{
    forget_into, frame_hash, is_digest, learn_into, Auth, FrameSigner, LinkConfig, RxState, Step,
};
use crate::bwe::BandwidthEstimator;
use crate::e2ee::{
    epoch_lsb, peek_counter, peek_sender, redundancy_for, SframeReceiver, SframeSender,
    DEFAULT_LEVEL, EPOCH_GRACE,
};
use crate::error::{MediaError, Result};
use crate::frame::{payload_of_stamp, received, ReceivedFrame};
use crate::key::MediaKey;
use crate::signal::Signal;
use crate::transport::{PacketTransport, UdpTransport};
use crate::video::{
    carry, layer_key, nal_of, stream_key, uncarry, RawFrame, RawVideo, VideoFrame, VideoGate,
    VideoMeta, VideoReplay,
};
use dissent_identity::DeviceCert;

/// Shortest gap between two keyframe requests from one side.
const PLI_RETRY: Duration = Duration::from_millis(200);

/// The bitrate str0m's bandwidth estimate starts from, in kbps. Video is sized to it until the
/// first estimate arrives.
pub(crate) const BWE_INITIAL_KBPS: u64 = 1_000;

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
    /// The sending bitrate the bandwidth estimate targets, in kbps, within the Opus range. It
    /// starts at a default, and moves with the measured RTT and loss, see
    /// [`Call::target_bitrate_kbps`].
    pub target_kbps: u32,
    /// What this side sent on the link, in kbps, every byte of every datagram, smoothed over the
    /// stats intervals. `None` until two stats reports have arrived. It is a floor for what the link
    /// carries, not an estimate of what it could: str0m's TWCC estimate stays at its starting value
    /// unless the call sets a desired bitrate and probes for it.
    pub sent_kbps: Option<u32>,
}

/// The keys of a call that has them.
struct Keys {
    tx: SframeSender,
    rx: SframeReceiver,
    sender_id: u64,
    /// Video frames use their own key, see [`MediaKey::video`].
    video_tx: SframeSender,
    video_rx: SframeReceiver,
}

impl Keys {
    fn new(key: MediaKey, sender_id: u64, epoch: u64) -> Result<Self> {
        let video = key.video();
        Ok(Self {
            tx: SframeSender::new(&key, sender_id, epoch)?,
            rx: SframeReceiver::new(key, epoch),
            sender_id,
            video_tx: SframeSender::new(&video, sender_id, epoch)?,
            video_rx: SframeReceiver::new(video, epoch),
        })
    }
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
    /// The video line this side offered. It becomes `video_mid` once the answer keeps it.
    offered_video_mid: Option<Mid>,
    /// Set once the peer keeps the video line. Stays unset for `m=video 0`.
    video_mid: Option<Mid>,
    video_inbox: VecDeque<VideoFrame>,
    video_raw_inbox: VecDeque<RawVideo>,
    video_gate: VideoGate,
    /// When this side last asked for a keyframe, to keep the requests a sane rate.
    last_pli: Option<Instant>,
    /// Set when the peer asks this side for a keyframe, see [`Call::take_keyframe_request`].
    keyframe_requested: bool,
    video_replay: VideoReplay,
    /// The latest bandwidth estimate from str0m, in kbps, and the callback that hears it.
    video_available_kbps: Option<u32>,
    video_available_cb: Option<Box<dyn FnMut(u32) + Send>>,
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
    /// Bytes sent by the last stats report, and when it came. See `CallStats::sent_kbps`.
    sent_mark: Option<(u64, Instant)>,
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
/// Bytes a packet adds around its RTP payload: the RTP header and extensions, and the SRTP tag.
const RTP_OVERHEAD: usize = 128;

fn new_rtc(start: Instant, max_datagram: Option<usize>) -> Result<Rtc> {
    let mut config = Rtc::builder()
        .clear_codecs()
        .enable_opus(true, false)
        .enable_h264(true)
        .enable_av1(true)
        .enable_bwe(Some(Bitrate::kbps(BWE_INITIAL_KBPS)))
        .set_stats_interval(Some(STATS_INTERVAL));
    if let Some(max) = max_datagram {
        if !(MTU_MIN..=MTU_MAX).contains(&max) {
            return Err(MediaError::BadState(
                "transport datagram size is outside the MTU range",
            ));
        }
        // str0m's MTU is the RTP payload limit. The SRTP tag and the RTP header and extensions sit
        // on top of it, so the datagram limit is kept for them. Without the margin a full-size
        // video packet is too big for the transport, and the call drops the peer.
        let mtu = max.saturating_sub(RTP_OVERHEAD).max(MTU_MIN);
        config = config.set_mtu(mtu..=mtu);
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
        let keys = Some(Keys::new(key, sender_id, epoch)?);
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
        let keys = Some(Keys::new(key, sender_id, epoch)?);
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
        // Every offer carries a video line. A peer without video answers `m=video 0`, which
        // means no video, not failure.
        let video = changes.add_media(MediaKind::Video, Direction::SendRecv, None, None, None);
        let (offer, pending) = changes
            .apply()
            .ok_or(MediaError::BadState("nothing to offer"))?;

        let mut call = Self::from_parts(rtc, transport, keys, signer, state);
        call.pending_offer = Some(pending);
        call.audio_mid = Some(mid);
        call.offered_video_mid = Some(video);
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
            offered_video_mid: None,
            video_mid: None,
            video_inbox: VecDeque::new(),
            video_raw_inbox: VecDeque::new(),
            video_gate: VideoGate::default(),
            last_pli: None,
            keyframe_requested: false,
            video_replay: VideoReplay::default(),
            video_available_kbps: None,
            video_available_cb: None,
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
            sent_mark: None,
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
                // An answer that rejects the video line (`m=video 0`) leaves the call audio-only.
                self.video_mid = self
                    .offered_video_mid
                    .filter(|mid| self.rtc.media(*mid).is_some_and(|m| !m.disabled()));
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

    /// Starts verifying `sender`'s digests with its device certificate. Call it when a member
    /// joins. Fails on an unauthenticated link.
    pub fn learn_cert(&mut self, sender_id: u64, cert: DeviceCert) -> Result<()> {
        learn_into(&self.state, sender_id, cert)
    }

    /// Stops verifying `sender_id`'s digests, and forgets its replay state. Call it when a member
    /// leaves. A removed member is cut off from new frames only by a rekey.
    pub fn forget_cert(&mut self, sender_id: u64) {
        forget_into(&self.state, sender_id);
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
        let video = key.video();
        keys.rx.rotate(key, epoch, now, EPOCH_GRACE)?;
        keys.video_rx
            .rotate(video.clone(), epoch, now, EPOCH_GRACE)?;
        keys.video_tx = keys.video_tx.rekeyed(&video, keys.sender_id, epoch)?;
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

    /// Whether the peer answered the video line with media. False for `m=video 0`, which means
    /// the call is audio-only.
    pub fn has_video(&self) -> bool {
        self.video_mid.is_some()
    }

    /// Whether the peer agreed to AV1 in this call's negotiation. Both sides offer AV1, so it is
    /// on when both can decode it. Frames still ride the H.264 carrier, and the AV1 bitstream
    /// is named in the frame's own header, see `video.rs`.
    pub fn peer_has_av1(&self) -> bool {
        self.rtc
            .codec_config()
            .params()
            .iter()
            .any(|p| p.spec().codec == Codec::Av1)
    }

    /// The latest available bandwidth from str0m's estimate, in kbps. `None` until the first
    /// estimate arrives, which needs BWE on and sustained traffic.
    pub fn video_available_kbps(&self) -> Option<u32> {
        self.video_available_kbps
    }

    /// Registers the callback that hears each new bandwidth estimate, in kbps. The video budget
    /// is the estimate minus the audio reservation, see `VideoAdapter::update`.
    pub fn on_video_available(&mut self, callback: impl FnMut(u32) + Send + 'static) {
        self.video_available_cb = Some(Box::new(callback));
    }

    /// Sends one encoded video frame of the stream described by `meta`. Each stream numbers its
    /// frames from 0. Fails with [`MediaError::NoVideoTrack`] when the peer has no video line.
    pub async fn send_video(&mut self, frame: &[u8], meta: VideoMeta) -> Result<()> {
        if self.video_mid.is_none() {
            return Err(MediaError::NoVideoTrack);
        }
        let keys = self
            .keys
            .as_mut()
            .ok_or(MediaError::BadState("a keyless call cannot encrypt"))?;
        let ciphertext = keys.video_tx.encrypt(frame, None, DEFAULT_LEVEL)?;
        self.send_video_ciphertext(&ciphertext, meta).await
    }

    /// Sends a frame that the caller already encrypted with this call's video key. A mesh or a
    /// participant encrypts once and sends the same ciphertext on each link.
    pub(crate) async fn send_video_ciphertext(&mut self, ciphertext: &[u8], meta: VideoMeta) -> Result<()> {
        if self.video_mid.is_none() {
            return Err(MediaError::NoVideoTrack);
        }
        let nal = carry(meta, ciphertext);
        self.write_video(nal).await
    }

    /// Waits for the next decrypted video frame.
    ///
    /// Returns `Ok(None)` once the call has closed. Frames that fail decryption or replay checks
    /// are counted in [`CallStats::frames_undecryptable`] and are not returned. Cancel-safe.
    pub async fn next_video_frame(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            if let Some(frame) = self.video_inbox.pop_front() {
                return Ok(Some(frame));
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
            if self.video_gate.waiting() {
                self.request_keyframe_throttled();
            }
            self.drain().await?;
            if self.video_inbox.is_empty() {
                self.pump_once().await?;
            }
        }
    }

    /// Waits for the next frame a keyless call received, audio or video, exactly as it arrived,
    /// so a forwarder can route it without a key. Cancel-safe.
    pub(crate) async fn next_raw_any(&mut self) -> Result<Option<RawFrame>> {
        loop {
            if let Some(frame) = self.raw_inbox.pop_front() {
                return Ok(Some(RawFrame::Audio(frame)));
            }
            if let Some(video) = self.video_raw_inbox.pop_front() {
                return Ok(Some(RawFrame::Video(video)));
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
            self.drain().await?;
            if self.raw_inbox.is_empty() && self.video_raw_inbox.is_empty() {
                self.pump_once().await?;
            }
        }
    }

    /// Whether the peer has asked for a keyframe since the last call. The caller should answer
    /// with `send_video(..., keyframe: true)`.
    pub fn take_keyframe_request(&mut self) -> bool {
        std::mem::take(&mut self.keyframe_requested)
    }

    /// Asks the peer for a keyframe now, without the throttle that incoming gaps use.
    pub fn request_keyframe(&mut self) -> Result<()> {
        let Some(mid) = self.video_mid else {
            return Err(MediaError::NoVideoTrack);
        };
        if let Some(mut writer) = self.rtc.writer(mid) {
            if writer.is_request_keyframe_possible(KeyframeRequestKind::Pli) {
                writer.request_keyframe(None, KeyframeRequestKind::Pli)?;
            }
        }
        self.last_pli = Some(Instant::now());
        Ok(())
    }

    /// Asks for a keyframe, at most once per [`PLI_RETRY`].
    fn request_keyframe_throttled(&mut self) {
        let now = Instant::now();
        if self
            .last_pli
            .is_some_and(|t| now.duration_since(t) < PLI_RETRY)
        {
            return;
        }
        if let Err(e) = self.request_keyframe() {
            tracing::debug!(error = %e, "could not ask for a keyframe");
        }
    }

    /// Sends a carrier NAL that a forwarder received, unchanged.
    pub(crate) async fn send_raw_video(&mut self, nal: &[u8]) -> Result<()> {
        if self.video_mid.is_none() {
            return Err(MediaError::NoVideoTrack);
        }
        self.write_video(nal.to_vec()).await
    }

    /// Writes one carrier NAL on the video track.
    async fn write_video(&mut self, nal: Vec<u8>) -> Result<()> {
        let mid = self.video_mid.ok_or(MediaError::NoVideoTrack)?;
        let media_time = self.started.elapsed().into();
        let writer = self.rtc.writer(mid).ok_or(MediaError::NoVideoTrack)?;
        // Pick the payload type from the writer's own list, which holds what the peer accepted.
        let pt = writer
            .payload_params()
            .filter(|p| p.spec().codec == Codec::H264)
            .find(|p| p.resend().is_some())
            .or_else(|| writer.payload_params().find(|p| p.spec().codec == Codec::H264))
            .map(|p| p.pt())
            .ok_or(MediaError::NoVideoTrack)?;
        writer.write(pt, Instant::now(), media_time, nal)?;
        self.drain().await
    }

    /// Handles one carrier NAL from the video line. A keyless call gates it by its clear header
    /// and queues it for its forwarder. A keyed call decrypts it, drops replays, gates it, and
    /// queues the frame.
    fn on_video(&mut self, delivered: &[u8]) {
        let nal = match nal_of(delivered) {
            Ok(nal) => nal,
            Err(e) => {
                tracing::debug!(error = %e, "dropped a malformed video frame");
                return;
            }
        };
        let (header, ciphertext) = match uncarry(nal) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(error = %e, "dropped a malformed video frame");
                return;
            }
        };
        if self.keys.is_none() {
            let Some(sender) = peek_sender(&ciphertext) else {
                return;
            };
            // Layers arrive interleaved, each with its own frame ids, so each is gated on its own.
            let stream = layer_key(stream_key(sender, header.screen), header.layer);
            let admit = self
                .video_gate
                .admit(stream, header.frame_id, header.keyframe);
            if admit.need_keyframe {
                self.request_keyframe_throttled();
            }
            if admit.deliver {
                self.video_raw_inbox.push_back(RawVideo {
                    nal: nal.to_vec(),
                    sender,
                    screen: header.screen,
                    keyframe: header.keyframe,
                    layer: header.layer,
                });
            }
            return;
        }
        let opened = match self.keys.as_ref() {
            Some(keys) => keys.video_rx.open(&ciphertext, Instant::now()),
            None => return,
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.stats.frames_undecryptable += 1;
                tracing::debug!(error = %e, "dropped an undecryptable video frame");
                return;
            }
        };
        if !self.video_replay.accept(opened.sender, opened.counter) {
            self.stats.frames_replayed += 1;
            return;
        }
        let admit = self.video_gate.admit(
            stream_key(opened.sender, header.screen),
            header.frame_id,
            header.keyframe,
        );
        if admit.need_keyframe {
            self.request_keyframe_throttled();
        }
        if !admit.deliver {
            return;
        }
        match payload_of_stamp(&opened.plaintext) {
            Ok(payload) => self.video_inbox.push_back(VideoFrame {
                sender: opened.sender,
                frame_id: header.frame_id,
                keyframe: header.keyframe,
                codec: header.codec,
                screen: header.screen,
                layer: header.layer,
                payload: payload.to_vec(),
            }),
            Err(e) => tracing::debug!(error = %e, "dropped a malformed video payload"),
        }
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

    /// Updates the smoothed send rate from a stats report's byte count.
    fn note_sent(&mut self, bytes: u64, at: Instant) {
        if let Some((before, then)) = self.sent_mark {
            let ms = at.saturating_duration_since(then).as_millis() as u64;
            // Bits per millisecond is kbps.
            if let Some(kbps) = bytes.saturating_sub(before).saturating_mul(8).checked_div(ms) {
                let kbps = u32::try_from(kbps).unwrap_or(u32::MAX);
                // Smoothed a little, so one burst or gap does not swing the announced capacity.
                self.stats.sent_kbps = Some(match self.stats.sent_kbps {
                    Some(old) => ((u64::from(old) * 7 + u64::from(kbps) * 3) / 10) as u32,
                    None => kbps,
                });
            }
        }
        self.sent_mark = Some((bytes, at));
    }

    /// A snapshot of the call's counters.
    pub fn stats(&self) -> CallStats {
        CallStats {
            target_kbps: self.bwe.target_kbps(),
            ..self.stats
        }
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
            Event::MediaAdded(added) if matches!(added.kind, MediaKind::Video) => {
                if self.rtc.media(added.mid).is_some_and(|m| !m.disabled()) {
                    self.video_mid = Some(added.mid);
                }
            }
            Event::MediaData(data) if Some(data.mid) == self.video_mid => {
                self.on_video(&data.data);
            }
            Event::MediaData(data) => self.on_media(&data.data),
            Event::KeyframeRequest(req) if Some(req.mid) == self.video_mid => {
                self.keyframe_requested = true;
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                let kbps = (estimate.as_u64() / 1_000) as u32;
                self.video_available_kbps = Some(kbps);
                if let Some(callback) = self.video_available_cb.as_mut() {
                    callback(kbps);
                }
            }
            Event::PeerStats(stats) => {
                self.note_sent(stats.peer_bytes_tx, stats.timestamp);
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
