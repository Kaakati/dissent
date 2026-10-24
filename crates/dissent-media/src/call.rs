//! A 1:1 media session: one str0m `Rtc` on one UDP socket, with SFrame applied to every frame.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::net::UdpSocket;

use crate::e2ee::{SframeReceiver, SframeSender};
use crate::error::{MediaError, Result};
use crate::key::MediaKey;
use crate::signal::Signal;

/// Largest datagram we read. Above the MTU, so nothing a peer sends is truncated.
const RECV_BUF: usize = 2048;
/// How often str0m reports peer and media statistics.
const STATS_INTERVAL: Duration = Duration::from_millis(500);

/// Counters and the latest measurements of a call.
///
/// Packet counts cover every UDP datagram of the session, including ICE and RTCP.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CallStats {
    /// Round-trip time in milliseconds, once str0m has measured it.
    pub rtt_ms: Option<f64>,
    /// UDP datagrams this side sent.
    pub packets_sent: u64,
    /// UDP datagrams this side received.
    pub packets_received: u64,
    /// Packets the remote peer reports as lost, from its receiver reports.
    pub packets_lost: u64,
    /// Audio frames this side encrypted and sent.
    pub frames_sent: u64,
    /// Audio frames that decrypted and were delivered.
    pub frames_received: u64,
    /// Incoming frames that failed SFrame decryption and were dropped.
    pub frames_undecryptable: u64,
}

/// One side of a 1:1 call.
///
/// Create it with [`Call::start_offer`] or [`Call::accept`], move the signals between the peers
/// with any transport, then call [`Call::apply`]. Drive the session with [`Call::next_frame`].
/// str0m does not run its own timers, so `next_frame` and `send_audio_frame` must keep being
/// called while the call is alive.
pub struct Call {
    rtc: Rtc,
    socket: UdpSocket,
    local_addr: SocketAddr,
    tx: SframeSender,
    rx: SframeReceiver,
    pending_offer: Option<SdpPendingOffer>,
    audio_mid: Option<Mid>,
    started: Instant,
    timeout: Instant,
    recv_buf: Vec<u8>,
    inbox: VecDeque<(u64, Vec<u8>)>,
    stats: CallStats,
}

// Keeps Call movable across tasks. A type that is not Send fails to compile here.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Call>();
};

fn new_rtc(start: Instant) -> Rtc {
    Rtc::builder()
        .clear_codecs()
        .enable_opus(true, false)
        .set_stats_interval(Some(STATS_INTERVAL))
        .build(start)
}

fn add_host_candidate(rtc: &mut Rtc, local: SocketAddr) -> Result<()> {
    let candidate = Candidate::host(local, "udp").map_err(|e| MediaError::Ice(e.to_string()))?;
    rtc.add_local_candidate(candidate);
    Ok(())
}

impl Call {
    /// Binds `bind` and creates the caller's offer. Send the returned [`Signal::Offer`] to the
    /// peer, then apply its answer with [`Call::apply`].
    ///
    /// `bind` should be `127.0.0.1:0` in tests. `sender_id` is this side's MLS member index.
    pub async fn start_offer(
        bind: SocketAddr,
        key: MediaKey,
        sender_id: u64,
    ) -> Result<(Self, Signal)> {
        let socket = UdpSocket::bind(bind).await?;
        let local_addr = socket.local_addr()?;
        let mut rtc = new_rtc(Instant::now());
        add_host_candidate(&mut rtc, local_addr)?;

        let mut changes = rtc.sdp_api();
        let mid = changes.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
        let (offer, pending) = changes
            .apply()
            .ok_or(MediaError::BadState("nothing to offer"))?;

        let mut call = Self::from_parts(rtc, socket, local_addr, key, sender_id)?;
        call.pending_offer = Some(pending);
        call.audio_mid = Some(mid);
        Ok((call, Signal::Offer(offer.to_sdp_string())))
    }

    /// Binds `bind` and answers the caller's [`Signal::Offer`]. Send the returned
    /// [`Signal::Answer`] back to the caller.
    pub async fn accept(
        bind: SocketAddr,
        key: MediaKey,
        sender_id: u64,
        offer: Signal,
    ) -> Result<(Self, Signal)> {
        let Signal::Offer(sdp) = offer else {
            return Err(MediaError::BadState("accept needs an offer"));
        };
        let offer =
            SdpOffer::from_sdp_string(&sdp).map_err(|e| MediaError::BadSignal(e.to_string()))?;

        let socket = UdpSocket::bind(bind).await?;
        let local_addr = socket.local_addr()?;
        let mut rtc = new_rtc(Instant::now());
        add_host_candidate(&mut rtc, local_addr)?;
        let answer = rtc.sdp_api().accept_offer(offer)?;

        // The audio track appears through an event once the offer is applied, see `on_event`.
        let call = Self::from_parts(rtc, socket, local_addr, key, sender_id)?;
        Ok((call, Signal::Answer(answer.to_sdp_string())))
    }

    fn from_parts(
        rtc: Rtc,
        socket: UdpSocket,
        local_addr: SocketAddr,
        key: MediaKey,
        sender_id: u64,
    ) -> Result<Self> {
        let now = Instant::now();
        Ok(Self {
            rtc,
            socket,
            local_addr,
            tx: SframeSender::new(&key, sender_id)?,
            rx: SframeReceiver::new(key),
            pending_offer: None,
            audio_mid: None,
            started: now,
            timeout: now,
            recv_buf: vec![0; RECV_BUF],
            inbox: VecDeque::new(),
            stats: CallStats::default(),
        })
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

    /// The address this side's socket is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Whether ICE and DTLS have completed, so media can flow.
    pub fn is_connected(&self) -> bool {
        self.rtc.is_connected()
    }

    /// Drives the session until ICE and DTLS complete. Frames that arrive meanwhile are kept
    /// for [`Call::next_frame`].
    pub async fn wait_connected(&mut self) -> Result<()> {
        loop {
            self.drain()?;
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
    /// media written earlier.
    pub async fn send_audio_frame(&mut self, frame: &[u8]) -> Result<()> {
        if !self.rtc.is_connected() {
            return Err(MediaError::NotConnected);
        }
        let mid = self.audio_mid.ok_or(MediaError::NoAudioTrack)?;
        let pt = self.opus_pt().ok_or(MediaError::NoAudioTrack)?;
        let encrypted = self.tx.encrypt(frame)?;
        let media_time = self.started.elapsed().into();
        let writer = self.rtc.writer(mid).ok_or(MediaError::NoAudioTrack)?;
        writer.write(pt, Instant::now(), media_time, encrypted)?;
        self.stats.frames_sent += 1;
        self.drain()
    }

    /// Waits for the next decrypted frame and returns `(sender id, plaintext)`.
    ///
    /// Returns `Ok(None)` once the call has closed. Frames that fail decryption are counted in
    /// [`CallStats::frames_undecryptable`] and are not returned.
    ///
    /// This method is cancel-safe. Dropping its future between awaits loses no frame.
    pub async fn next_frame(&mut self) -> Result<Option<(u64, Vec<u8>)>> {
        loop {
            if let Some(frame) = self.inbox.pop_front() {
                return Ok(Some(frame));
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
            self.drain()?;
            if self.inbox.is_empty() {
                self.pump_once().await?;
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
    fn drain(&mut self) -> Result<()> {
        loop {
            match self.rtc.poll_output()? {
                Output::Timeout(at) => {
                    self.timeout = at;
                    return Ok(());
                }
                Output::Transmit(transmit) => {
                    self.stats.packets_sent += 1;
                    match self
                        .socket
                        .try_send_to(&transmit.contents[..], transmit.destination)
                    {
                        Ok(_) => {}
                        // UDP may drop a datagram, and the stack recovers from that.
                        Err(e)
                            if matches!(
                                e.kind(),
                                ErrorKind::WouldBlock | ErrorKind::ConnectionReset
                            ) =>
                        {
                            tracing::debug!(error = %e, "dropped outgoing datagram");
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                Output::Event(event) => self.on_event(event),
            }
        }
    }

    /// Waits for one of two things: a datagram from the socket, or str0m's timeout.
    /// It then feeds the input to str0m and returns, so the caller drains again.
    async fn pump_once(&mut self) -> Result<()> {
        let deadline = tokio::time::Instant::from_std(self.timeout);
        tokio::select! {
            received = self.socket.recv_from(&mut self.recv_buf) => {
                let (len, source) = match received {
                    Ok(v) => v,
                    // Windows reports an earlier ICMP port-unreachable as ConnectionReset here.
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok(()),
                    Err(e) => return Err(e.into()),
                };
                self.stats.packets_received += 1;
                let contents = &self.recv_buf[..len];
                match Receive::new(Protocol::Udp, source, self.local_addr, contents) {
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
                    self.stats.rtt_ms = Some(rtt.as_secs_f64() * 1000.0);
                }
            }
            Event::MediaEgressStats(stats) => {
                if let Some(remote) = stats.remote {
                    self.stats.packets_lost = remote.packets_lost;
                }
            }
            _ => {}
        }
    }

    fn on_media(&mut self, encrypted: &[u8]) {
        match self.rx.decrypt(encrypted) {
            Ok((sender, plaintext)) => {
                self.stats.frames_received += 1;
                self.inbox.push_back((sender, plaintext));
            }
            Err(e) => {
                self.stats.frames_undecryptable += 1;
                tracing::debug!(error = %e, "dropped undecryptable frame");
            }
        }
    }
}
