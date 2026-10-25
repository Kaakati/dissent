//! Adaptive jitter buffer, a simplified NetEQ.
//!
//! Packets go in with their sequence number, RTP timestamp, payload and arrival time. The caller
//! calls [`JitterBuffer::pop`] once per 20 ms playout tick, and each call yields exactly one
//! frame. The buffer does three things:
//!
//! Slots come from the RTP timestamp, not from the packet's `seq`. The sender's clock counts every
//! frame it captured, including the silent ones it does not always send, so a gap in the stream
//! is silence in time. A missing slot after a silent packet plays as silence and is counted as a
//! silent gap, not as loss. Other missing slots are concealed as before.
//!
//! - It estimates network jitter from arrival times. The target delay is the 95th percentile of
//!   the jitter, clamped to 20 to 200 ms. The target grows at once and shrinks slowly.
//! - A missing frame is recovered with Opus FEC from the next packet when that packet is present,
//!   and concealed with PLC otherwise. Packets that arrive after their slot are dropped as late.
//! - It grows the delay by holding playout for one tick and concealing that tick, whenever the
//!   buffer is shallower than the target. It shrinks the delay by skipping a frame. Skips happen
//!   at silence, or on speech only when the excess is large.

use std::collections::{BTreeMap, VecDeque};

use crate::codec::{Decoder, FRAME_SAMPLES};
use crate::error::{AudioError, Result};

/// Playout tick length in milliseconds.
pub const FRAME_MS: u64 = 20;
/// Smallest target delay the buffer will use.
pub const MIN_TARGET_MS: f64 = 20.0;
/// Target delay before any jitter has been measured.
const INITIAL_TARGET_MS: f64 = 40.0;
/// Largest target delay the buffer will use.
pub const MAX_TARGET_MS: f64 = 200.0;

/// Arrival samples kept for the jitter estimate, one per packet (about four seconds).
const JITTER_WINDOW: usize = 200;
/// Percentile of the jitter distribution the target delay follows.
const PERCENTILE: f64 = 0.95;
/// Fraction of the gap closed per packet when the target shrinks.
const SHRINK_RATE: f64 = 0.02;
/// Consecutive concealed frames before the buffer outputs silence and restarts.
const MAX_CONCEALED_RUN: u32 = 25;
/// A packet this many sequence numbers ahead of the cursor means the stream restarted.
const RESYNC_AHEAD: u32 = 100;
/// Extra buffered audio, in ms above target, that triggers a skip of one silent frame.
const SHRINK_SLACK_MS: f64 = 60.0;
/// Extra buffered audio, in ms above target, that triggers a skip of one frame of any kind.
const EXCESS_SLACK_MS: f64 = 120.0;
/// Buffered audio, in ms below target, that triggers a held tick to add delay.
const GROW_SLACK_MS: f64 = 20.0;
/// Minimum ticks between two delay insertions, so growth is gradual.
const INSERT_SPACING_TICKS: u64 = 3;

/// One packet as it arrives from the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Sequence number of the frame the packet came from. The buffer does not use it for timing;
    /// slots come from `timestamp`.
    pub seq: u32,
    /// RTP timestamp in samples at 48 kHz. The playout slot is this clock in 20 ms frames.
    pub timestamp: u32,
    /// Codec payload.
    pub payload: Vec<u8>,
    /// The sender marked the frame silent, as RFC 6464 audio level 127. A slot after a silent
    /// packet that never arrives is a silent gap.
    pub silent: bool,
    /// Local arrival time in milliseconds.
    pub arrival_ms: u64,
}

/// How [`JitterBuffer::pop`] produced a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playout {
    /// Decoded from its own packet.
    Decoded,
    /// Recovered from the redundancy in the next packet.
    Fec,
    /// Concealed by the decoder's packet loss concealment.
    Concealed,
    /// Output silence. The buffer had not started, a delay adjustment repeated silence, the slot
    /// was a silent gap the sender never sent, or the loss ran past the concealment budget.
    Silence,
}

/// Counters and the current delay, as reported by [`JitterBuffer::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct JitterStats {
    /// Current target delay in milliseconds.
    pub target_delay_ms: f64,
    /// Audio buffered ahead of the playout cursor, in milliseconds.
    pub buffered_ms: f64,
    /// Packets offered to [`JitterBuffer::push`].
    pub received: u64,
    /// Packets that arrived after their playout slot and were dropped.
    pub late: u64,
    /// Packets already buffered, offered again, and dropped.
    pub duplicates: u64,
    /// Playout slots whose packet never arrived in time.
    pub lost: u64,
    /// Lost slots filled by FEC from the next packet.
    pub fec_recovered: u64,
    /// Lost slots filled by PLC.
    pub concealed: u64,
    /// Lost slots output as silence because the concealment budget ran out.
    pub silenced: u64,
    /// Silent frames repeated to add delay.
    pub stretched: u64,
    /// Silent packets skipped to remove delay.
    pub compressed: u64,
    /// Slots after a silent packet that the sender never sent. Played as silence, not loss.
    pub silent_gaps: u64,
}

/// Estimates the target delay from packet arrival times.
#[derive(Debug)]
struct DelayEstimator {
    /// Arrival time minus media time, in milliseconds, for the last packets.
    transit: VecDeque<f64>,
    target_ms: f64,
}

impl DelayEstimator {
    fn new() -> Self {
        DelayEstimator {
            transit: VecDeque::with_capacity(JITTER_WINDOW),
            target_ms: INITIAL_TARGET_MS,
        }
    }

    fn observe(&mut self, arrival_ms: u64, timestamp: u32) {
        let media_ms = f64::from(timestamp) / 48.0;
        self.transit.push_back(arrival_ms as f64 - media_ms);
        if self.transit.len() > JITTER_WINDOW {
            self.transit.pop_front();
        }
        let min = self.transit.iter().copied().fold(f64::INFINITY, f64::min);
        let mut jitter: Vec<f64> = self.transit.iter().map(|t| t - min).collect();
        jitter.sort_by(f64::total_cmp);
        let idx = ((jitter.len() - 1) as f64 * PERCENTILE).round() as usize;
        let raw = jitter[idx].clamp(MIN_TARGET_MS, MAX_TARGET_MS);
        if raw > self.target_ms {
            self.target_ms = raw;
        } else {
            self.target_ms += (raw - self.target_ms) * SHRINK_RATE;
        }
    }
}

/// One buffered packet.
#[derive(Debug)]
struct Slot {
    payload: Vec<u8>,
    silent: bool,
}

/// Adaptive jitter buffer over a [`Decoder`].
pub struct JitterBuffer<D: Decoder> {
    decoder: D,
    buffer: BTreeMap<u32, Slot>,
    delay: DelayEstimator,
    /// Sequence number of the next slot to play. `None` while the buffer waits to start.
    cursor: Option<u32>,
    /// Arrival time of the first packet since the last restart. Playout starts one target delay
    /// after it.
    first_arrival_ms: Option<u64>,
    concealed_run: u32,
    /// True when the last slot played was a silent packet, so a missing slot is a silent gap.
    last_silent: bool,
    /// The last timestamp seen and its unwrapped sample clock.
    clock: Option<(u32, i64)>,
    ticks: u64,
    last_insert_tick: u64,
    stats: JitterStats,
}

impl<D: Decoder> JitterBuffer<D> {
    /// Creates an empty buffer that decodes with `decoder`.
    pub fn new(decoder: D) -> Self {
        JitterBuffer {
            decoder,
            buffer: BTreeMap::new(),
            delay: DelayEstimator::new(),
            cursor: None,
            first_arrival_ms: None,
            concealed_run: 0,
            last_silent: false,
            clock: None,
            ticks: 0,
            last_insert_tick: 0,
            stats: JitterStats::default(),
        }
    }

    /// Current target delay in milliseconds.
    pub fn target_delay_ms(&self) -> f64 {
        self.delay.target_ms
    }

    /// Counters and the current delay.
    pub fn stats(&self) -> JitterStats {
        let mut s = self.stats;
        s.target_delay_ms = self.delay.target_ms;
        s.buffered_ms = self.buffered_frames() as f64 * FRAME_MS as f64;
        s
    }

    /// Playout slot of a timestamp: the unwrapped sample clock in 20 ms frames. The sender's clock
    /// wraps at 2^32 samples, so each step is taken as a signed difference from the last one.
    fn slot(&mut self, timestamp: u32) -> u32 {
        let ext = match self.clock {
            None => i64::from(timestamp),
            Some((last, ext)) => {
                let step = i64::from(timestamp.wrapping_sub(last) as i32);
                (ext + step).max(0)
            }
        };
        self.clock = Some((timestamp, ext));
        (ext as u64 / FRAME_SAMPLES as u64) as u32
    }

    /// Adds a packet that has just arrived.
    pub fn push(&mut self, packet: Packet) {
        self.stats.received += 1;
        self.delay.observe(packet.arrival_ms, packet.timestamp);
        let seq = self.slot(packet.timestamp);

        if let Some(cursor) = self.cursor {
            if seq < cursor {
                self.stats.late += 1;
                return;
            }
            if seq - cursor > RESYNC_AHEAD {
                // The sender restarted or was silent for a long time. Rebuffer from here.
                self.restart();
            }
        }
        if self.buffer.contains_key(&seq) {
            self.stats.duplicates += 1;
            return;
        }
        if self.first_arrival_ms.is_none() {
            self.first_arrival_ms = Some(packet.arrival_ms);
        }
        self.buffer.insert(
            seq,
            Slot {
                payload: packet.payload,
                silent: packet.silent,
            },
        );
    }

    /// Produces the frame for the current 20 ms tick and writes it into `out`.
    ///
    /// The caller calls this once per tick, and every call produces one frame.
    pub fn pop(&mut self, now_ms: u64, out: &mut [f32]) -> Result<Playout> {
        if out.len() != FRAME_SAMPLES {
            return Err(AudioError::FrameSize {
                expected: FRAME_SAMPLES,
                got: out.len(),
            });
        }
        self.ticks += 1;
        if self.cursor.is_none() {
            // Playout starts one target delay after the first packet, using the current target.
            let start = self
                .first_arrival_ms
                .map(|t| t + self.delay.target_ms.round() as u64);
            match (start, self.buffer.keys().next().copied()) {
                (Some(start), Some(first)) if now_ms >= start => self.cursor = Some(first),
                _ => return Ok(self.silence(out)),
            }
        }
        let mut seq = self.cursor.unwrap_or_default();

        // Delay adjustment. Growing holds the cursor and conceals this tick. Shrinking skips a
        // frame, but only at silence unless the excess is large.
        let target = self.delay.target_ms;
        let ahead_ms = self.buffered_frames() as f64 * FRAME_MS as f64;
        let present = self.buffer.get(&seq);
        let silent_next = present.is_some_and(|p| p.silent);
        let excess = ahead_ms - target;
        if present.is_some()
            && (excess > EXCESS_SLACK_MS || (silent_next && excess > SHRINK_SLACK_MS))
        {
            self.buffer.remove(&seq);
            seq += 1;
            self.cursor = Some(seq);
            self.stats.compressed += 1;
        } else if ahead_ms > 0.0
            && ahead_ms + GROW_SLACK_MS < target
            && self.ticks - self.last_insert_tick >= INSERT_SPACING_TICKS
        {
            self.last_insert_tick = self.ticks;
            self.stats.stretched += 1;
            return match self.decoder.decode(None, out) {
                Ok(_) => Ok(Playout::Concealed),
                Err(_) => Ok(self.silence(out)),
            };
        }

        if let Some(slot) = self.buffer.remove(&seq) {
            if self.decoder.decode(Some(&slot.payload), out).is_ok() {
                self.concealed_run = 0;
                self.last_silent = slot.silent;
                self.cursor = Some(seq + 1);
                return Ok(Playout::Decoded);
            }
        }

        // The packet for this slot is missing. After a silent packet, the sender did not send
        // this slot because it was silence, so it plays as silence.
        if self.last_silent {
            self.stats.silent_gaps += 1;
            self.cursor = Some(seq + 1);
            return Ok(self.silence(out));
        }

        // Otherwise it is a loss. Try FEC from the next packet first.
        self.stats.lost += 1;
        self.last_silent = false;
        let fec = match self.buffer.get(&(seq + 1)) {
            Some(next) => self.decoder.decode_fec(&next.payload, out).is_ok(),
            None => false,
        };
        self.cursor = Some(seq + 1);
        if fec {
            self.concealed_run = 0;
            self.stats.fec_recovered += 1;
            return Ok(Playout::Fec);
        }

        if self.concealed_run >= MAX_CONCEALED_RUN {
            self.stats.silenced += 1;
            if self.buffer.is_empty() {
                self.restart();
            }
            return Ok(self.silence(out));
        }
        match self.decoder.decode(None, out) {
            Ok(_) => {
                self.concealed_run += 1;
                self.stats.concealed += 1;
                Ok(Playout::Concealed)
            }
            Err(_) => {
                self.stats.silenced += 1;
                Ok(self.silence(out))
            }
        }
    }

    /// Number of frames buffered ahead of the cursor, counting the cursor slot.
    fn buffered_frames(&self) -> u32 {
        match (self.cursor, self.buffer.keys().next_back()) {
            (Some(cursor), Some(&last)) if last >= cursor => last - cursor + 1,
            _ => 0,
        }
    }

    fn silence(&self, out: &mut [f32]) -> Playout {
        out.fill(0.0);
        Playout::Silence
    }

    fn restart(&mut self) {
        self.buffer.clear();
        self.cursor = None;
        self.first_arrival_ms = None;
        self.concealed_run = 0;
        self.last_silent = false;
    }
}

