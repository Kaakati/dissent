//! Voice pipeline glue. [`VoiceSender`] turns mic audio into packets, and [`VoiceReceiver`] turns
//! packets from each sender into one mixed playout stream.
//!
//! Both sides work at 48 kHz in 20 ms frames and do not touch devices or the network. The app
//! wires them together like this on each side of a call:
//!
//! 1. Capture samples go to [`VoiceSender::push_capture`]. Each returned [`SenderPacket`] goes out
//!    as [`SenderPacket::to_media_frame`] over the call transport.
//! 2. Each received frame goes to [`VoiceReceiver::push_media_frame`], tagged with its sender.
//! 3. Once per 20 ms tick, [`VoiceReceiver::pop_mix`] produces the playout frame. That frame goes
//!    to the playout device and to [`VoiceSender::push_far_reference`], which lets the echo
//!    canceller remove the speaker's sound from the mic.
//!
//! Unvoiced frames, including muted and push-to-talk-closed ones, are digital silence. The encoder
//! turns a run of silence into DTX packets of one or two bytes. The desktop call path does not
//! send unvoiced frames. Their timestamps still advance, so the receiver sees a gap in time rather
//! than loss, and it plays a gap after a quiet packet as silence (see [`QUIET_LEVEL`]).

use std::collections::BTreeMap;

use crate::codec::{Decoder, Encoder, OpusDecoder, OpusEncoder, FRAME_SAMPLES};
use crate::dsp::{
    audio_level, db_to_amp, rms, soft_limit, Agc, EchoCanceller, NoiseSuppressor, VoiceDetector,
    DENOISE_BLOCK,
};
use crate::error::{AudioError, Result};
use crate::jitter::{JitterBuffer, JitterStats, Packet};

/// Audio level reported for silent or gated frames, as in RFC 6464.
pub const SILENT_LEVEL: u8 = 127;

/// Frames at or above this RFC 6464 level, which is -60 dBFS or quieter, count as silence when the
/// receiver decides whether a missing slot is a silent gap. Speech sits well below this.
pub const QUIET_LEVEL: u8 = 60;

/// Options for [`VoiceSender`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SenderConfig {
    /// Opus bitrate in bits per second, clamped to 16 to 64 kbps by the encoder.
    pub bitrate_bps: u32,
    /// Expected packet loss, 0 to 100, which sets how much in-band FEC the encoder adds.
    pub loss_percent: u8,
    /// Run RNNoise noise suppression.
    pub noise_suppression: bool,
    /// Run the AGC.
    pub agc: bool,
}

impl Default for SenderConfig {
    fn default() -> Self {
        SenderConfig {
            bitrate_bps: 32_000,
            loss_percent: 5,
            noise_suppression: true,
            agc: true,
        }
    }
}

/// One encoded frame, ready for the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderPacket {
    /// Frame counter, starting at 0 and increasing by one per frame.
    pub seq: u32,
    /// RTP timestamp in samples at 48 kHz.
    pub timestamp: u32,
    /// RFC 6464 audio level: 0 for full scale down to 127 for silence.
    pub audio_level: u8,
    /// True when the frame was voiced and transmitted, false when it was silent or gated.
    pub voiced: bool,
    /// Opus packet bytes.
    pub payload: Vec<u8>,
}

impl SenderPacket {
    /// The bytes a media frame carries: one audio-level byte, then the Opus packet.
    pub fn to_media_frame(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.payload.len());
        out.push(self.audio_level);
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Mic side: capture, processing, VAD, encoding and gating.
pub struct VoiceSender<E: Encoder> {
    encoder: E,
    config: SenderConfig,
    denoise: NoiseSuppressor,
    agc: Agc,
    vad: VoiceDetector,
    echo: Option<EchoCanceller>,
    accum: Vec<f32>,
    seq: u32,
    muted: bool,
    ptt_enabled: bool,
    ptt_pressed: bool,
}

impl VoiceSender<OpusEncoder> {
    /// A sender with the Opus encoder and `config`.
    pub fn opus(config: SenderConfig) -> Result<Self> {
        let encoder = OpusEncoder::new(config.bitrate_bps, config.loss_percent)?;
        Ok(Self::new(encoder, config))
    }
}

impl<E: Encoder> VoiceSender<E> {
    /// Creates a sender around `encoder`.
    pub fn new(encoder: E, config: SenderConfig) -> Self {
        VoiceSender {
            encoder,
            config,
            denoise: NoiseSuppressor::new(),
            agc: Agc::new(),
            vad: VoiceDetector::default(),
            echo: None,
            accum: Vec::with_capacity(FRAME_SAMPLES * 2),
            seq: 0,
            muted: false,
            ptt_enabled: false,
            ptt_pressed: false,
        }
    }

    /// Turns on echo cancellation with a `tail_ms` filter. Headphones need none.
    pub fn enable_echo_cancellation(&mut self, tail_ms: u32) {
        self.echo = Some(EchoCanceller::with_tail_ms(tail_ms));
    }

    /// Feeds the audio that was just sent to the speaker, as the echo reference.
    pub fn push_far_reference(&mut self, far: &[f32]) {
        if let Some(echo) = self.echo.as_mut() {
            echo.push_far(far);
        }
    }

    /// Mutes or unmutes the mic. While muted, every frame is unvoiced, so the call path sends none.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// True when the mic is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Enables or disables push-to-talk. While enabled, audio goes out only when the key is held.
    pub fn set_push_to_talk(&mut self, enabled: bool) {
        self.ptt_enabled = enabled;
    }

    /// Sets whether the push-to-talk key is held.
    pub fn set_ptt_pressed(&mut self, pressed: bool) {
        self.ptt_pressed = pressed;
    }

    /// True when audio is allowed to leave: not muted, and either push-to-talk is off or held.
    pub fn transmitting(&self) -> bool {
        !self.muted && (!self.ptt_enabled || self.ptt_pressed)
    }

    /// The encoder, for changes such as the bitrate or the expected loss.
    pub fn encoder_mut(&mut self) -> &mut E {
        &mut self.encoder
    }

    /// Adds 48 kHz capture samples and returns every frame that is now complete.
    pub fn push_capture(&mut self, samples: &[f32]) -> Result<Vec<SenderPacket>> {
        self.accum.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.accum.len() >= FRAME_SAMPLES {
            let frame: Vec<f32> = self.accum.drain(..FRAME_SAMPLES).collect();
            out.push(self.process_frame(frame)?);
        }
        Ok(out)
    }

    fn process_frame(&mut self, mut frame: Vec<f32>) -> Result<SenderPacket> {
        if let Some(echo) = self.echo.as_mut() {
            echo.process(&mut frame);
        }

        let vad_probability = if self.config.noise_suppression {
            let mut sum = 0.0;
            let mut blocks = 0.0;
            for block in frame.as_chunks_mut::<DENOISE_BLOCK>().0 {
                sum += self.denoise.process_block(block);
                blocks += 1.0;
            }
            sum / blocks
        } else if rms(&frame) > db_to_amp(-45.0) {
            1.0
        } else {
            0.0
        };

        let voiced_speech = self.vad.update(vad_probability);
        if self.config.agc {
            self.agc.process(&mut frame);
            soft_limit(&mut frame);
        }

        let transmit = self.transmitting();
        let voiced = voiced_speech && transmit;
        if !voiced {
            // An unvoiced frame is digital zero. The encoder turns a run of them into DTX packets.
            frame.fill(0.0);
        }
        let audio_level = if voiced {
            audio_level(&frame)
        } else {
            SILENT_LEVEL
        };
        let payload = self.encoder.encode(&frame)?;

        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        Ok(SenderPacket {
            seq,
            timestamp: seq.wrapping_mul(FRAME_SAMPLES as u32),
            audio_level,
            voiced,
            payload,
        })
    }
}

/// Factory that makes one decoder per sender.
pub type DecoderFactory = Box<dyn FnMut() -> Result<Box<dyn Decoder>> + Send>;

struct Speaker {
    jitter: JitterBuffer<Box<dyn Decoder>>,
    level: u8,
    frame: Vec<f32>,
}

/// Playout side: one jitter buffer and decoder per sender, mixed to one stream.
pub struct VoiceReceiver {
    factory: DecoderFactory,
    speakers: BTreeMap<u64, Speaker>,
    max_mixed: usize,
}

impl VoiceReceiver {
    /// Creates a receiver that mixes up to `max_mixed` senders per tick. Each sender gets a
    /// decoder from `factory`.
    pub fn new(max_mixed: usize, factory: DecoderFactory) -> Self {
        VoiceReceiver {
            factory,
            speakers: BTreeMap::new(),
            max_mixed: max_mixed.max(1),
        }
    }

    /// A receiver that decodes with Opus.
    pub fn opus(max_mixed: usize) -> Self {
        Self::new(
            max_mixed,
            Box::new(|| Ok(Box::new(OpusDecoder::new()?) as Box<dyn Decoder>)),
        )
    }

    /// Adds one received media frame from `sender`. The frame is an audio-level byte followed by
    /// the Opus packet, as [`SenderPacket::to_media_frame`] writes it.
    pub fn push_media_frame(
        &mut self,
        sender: u64,
        seq: u32,
        timestamp: u32,
        arrival_ms: u64,
        frame: &[u8],
    ) -> Result<()> {
        let (&level, payload) = frame.split_first().ok_or(AudioError::FrameSize {
            expected: 1,
            got: 0,
        })?;
        if !self.speakers.contains_key(&sender) {
            let decoder = (self.factory)()?;
            self.speakers.insert(
                sender,
                Speaker {
                    jitter: JitterBuffer::new(decoder),
                    level: SILENT_LEVEL,
                    frame: vec![0.0; FRAME_SAMPLES],
                },
            );
        }
        if let Some(speaker) = self.speakers.get_mut(&sender) {
            speaker.level = level;
            speaker.jitter.push(Packet {
                seq,
                timestamp,
                payload: payload.to_vec(),
                arrival_ms,
                silent: level >= QUIET_LEVEL,
            });
        }
        Ok(())
    }

    /// Produces the mixed playout frame for this tick into `out`.
    ///
    /// Every sender's jitter buffer advances one frame. The `max_mixed` senders with the loudest
    /// recent audio level are summed and clamped to -1.0 to 1.0.
    pub fn pop_mix(&mut self, now_ms: u64, out: &mut [f32]) -> Result<()> {
        if out.len() != FRAME_SAMPLES {
            return Err(AudioError::FrameSize {
                expected: FRAME_SAMPLES,
                got: out.len(),
            });
        }
        out.fill(0.0);
        for speaker in self.speakers.values_mut() {
            speaker.jitter.pop(now_ms, &mut speaker.frame)?;
        }

        let mut ranked: Vec<(u8, u64)> =
            self.speakers.iter().map(|(id, s)| (s.level, *id)).collect();
        ranked.sort_unstable();
        for (_, id) in ranked.into_iter().take(self.max_mixed) {
            if let Some(speaker) = self.speakers.get(&id) {
                for (dst, src) in out.iter_mut().zip(speaker.frame.iter()) {
                    *dst += src;
                }
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        Ok(())
    }

    /// Removes a sender, such as when they leave the call. Returns true if they were present.
    pub fn remove_sender(&mut self, sender: u64) -> bool {
        self.speakers.remove(&sender).is_some()
    }

    /// Jitter and loss counters for every sender.
    pub fn stats(&self) -> BTreeMap<u64, JitterStats> {
        self.speakers
            .iter()
            .map(|(id, s)| (*id, s.jitter.stats()))
            .collect()
    }
}

impl std::fmt::Debug for VoiceReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceReceiver")
            .field("senders", &self.speakers.len())
            .field("max_mixed", &self.max_mixed)
            .finish_non_exhaustive()
    }
}
