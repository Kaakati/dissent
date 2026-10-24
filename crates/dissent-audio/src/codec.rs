//! Frame codecs. The pipeline talks to them through [`Encoder`] and [`Decoder`], so tests can
//! use [`PcmEncoder`] and [`PcmDecoder`] and the app uses [`OpusEncoder`] and [`OpusDecoder`].
//!
//! Every frame is 20 ms of mono 48 kHz audio, which is [`FRAME_SAMPLES`] samples.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::error::{AudioError, Result};
use crate::opus_ffi as ffi;

/// Sample rate of every stream in the pipeline.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one 20 ms frame at [`SAMPLE_RATE`].
pub const FRAME_SAMPLES: usize = 960;
/// Lowest bitrate the Opus encoder accepts from this crate.
pub const MIN_BITRATE: u32 = 16_000;
/// Highest bitrate the Opus encoder accepts from this crate.
pub const MAX_BITRATE: u32 = 64_000;
/// Upper bound on one Opus packet. libopus never emits more than 1275 bytes per frame.
pub const MAX_PACKET_BYTES: usize = 1500;

/// Encodes 20 ms frames into packets.
pub trait Encoder: Send {
    /// Encodes exactly [`FRAME_SAMPLES`] samples and returns the packet.
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>>;
    /// Sets the target bitrate in bits per second. Values are clamped to 16 to 64 kbps.
    fn set_bitrate(&mut self, bps: u32) -> Result<()>;
    /// Sets the expected packet loss percentage, 0 to 100. The encoder spends more bits on
    /// in-band redundancy as this rises.
    fn set_loss_percent(&mut self, percent: u8) -> Result<()>;
}

/// Decodes packets into 20 ms frames, with loss concealment and FEC recovery.
pub trait Decoder: Send {
    /// Decodes one packet into `out`, which must hold [`FRAME_SAMPLES`] samples. `None` means the
    /// packet was lost, and the decoder conceals it (PLC).
    fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> Result<usize>;
    /// Recovers the frame that was lost just before `next_packet`, using the redundancy carried
    /// in `next_packet`. Returns [`AudioError::Unsupported`] when the codec has no redundancy.
    fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32]) -> Result<usize>;
}

fn check_len(expected: usize, got: usize) -> Result<()> {
    if expected == got {
        Ok(())
    } else {
        Err(AudioError::FrameSize { expected, got })
    }
}

fn check_ret(call: &'static str, code: c_int) -> Result<c_int> {
    if code < 0 {
        Err(AudioError::Opus { call, code })
    } else {
        Ok(code)
    }
}

/// Opus encoder: VOIP mode, 48 kHz mono, in-band FEC and DTX on, VBR, voice signal hint.
pub struct OpusEncoder {
    raw: NonNull<ffi::OpusEncoder>,
}

// SAFETY: libopus encoder state has no thread affinity and is only used through `&mut self`.
unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    /// Creates an encoder at `bitrate_bps` (clamped to 16 to 64 kbps) that expects
    /// `loss_percent` packet loss.
    pub fn new(bitrate_bps: u32, loss_percent: u8) -> Result<Self> {
        let mut err: c_int = 0;
        // SAFETY: plain FFI constructor. The status and the returned pointer are checked below.
        let raw = unsafe {
            ffi::opus_encoder_create(SAMPLE_RATE as i32, 1, ffi::OPUS_APPLICATION_VOIP, &mut err)
        };
        check_ret("opus_encoder_create", err)?;
        let raw = NonNull::new(raw).ok_or(AudioError::Opus {
            call: "opus_encoder_create",
            code: -7,
        })?;
        let mut enc = OpusEncoder { raw };
        enc.ctl(
            ffi::OPUS_SET_SIGNAL_REQUEST,
            ffi::OPUS_SIGNAL_VOICE,
            "signal",
        )?;
        enc.ctl(ffi::OPUS_SET_VBR_REQUEST, 1, "vbr")?;
        enc.ctl(ffi::OPUS_SET_INBAND_FEC_REQUEST, 1, "inband_fec")?;
        enc.ctl(ffi::OPUS_SET_DTX_REQUEST, 1, "dtx")?;
        enc.ctl(ffi::OPUS_SET_COMPLEXITY_REQUEST, 8, "complexity")?;
        enc.set_bitrate(bitrate_bps)?;
        enc.set_loss_percent(loss_percent)?;
        Ok(enc)
    }

    /// True while DTX holds the encoder in discontinuous mode, which means the last frame was
    /// silence and sent as a tiny packet.
    pub fn in_dtx(&mut self) -> bool {
        let mut v: i32 = 0;
        // SAFETY: the request writes one opus_int32 through the pointer passed.
        let code = unsafe {
            ffi::opus_encoder_ctl(self.raw.as_ptr(), ffi::OPUS_GET_IN_DTX_REQUEST, &mut v)
        };
        code == ffi::OPUS_OK && v != 0
    }

    fn ctl(&mut self, request: c_int, value: i32, call: &'static str) -> Result<()> {
        // SAFETY: every request used here takes a single opus_int32 argument.
        let code = unsafe { ffi::opus_encoder_ctl(self.raw.as_ptr(), request, value) };
        check_ret(call, code).map(|_| ())
    }
}

impl Encoder for OpusEncoder {
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        check_len(FRAME_SAMPLES, pcm.len())?;
        let mut out = vec![0u8; MAX_PACKET_BYTES];
        // SAFETY: `pcm` holds FRAME_SAMPLES floats and `out` is MAX_PACKET_BYTES long.
        let n = unsafe {
            ffi::opus_encode_float(
                self.raw.as_ptr(),
                pcm.as_ptr(),
                FRAME_SAMPLES as c_int,
                out.as_mut_ptr(),
                MAX_PACKET_BYTES as i32,
            )
        };
        let n = check_ret("opus_encode_float", n)? as usize;
        out.truncate(n);
        Ok(out)
    }

    fn set_bitrate(&mut self, bps: u32) -> Result<()> {
        let bps = bps.clamp(MIN_BITRATE, MAX_BITRATE) as i32;
        self.ctl(ffi::OPUS_SET_BITRATE_REQUEST, bps, "bitrate")
    }

    fn set_loss_percent(&mut self, percent: u8) -> Result<()> {
        let p = i32::from(percent.min(100));
        self.ctl(
            ffi::OPUS_SET_PACKET_LOSS_PERC_REQUEST,
            p,
            "packet_loss_perc",
        )
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        // SAFETY: `raw` came from opus_encoder_create and is destroyed exactly once.
        unsafe { ffi::opus_encoder_destroy(self.raw.as_ptr()) }
    }
}

/// Opus decoder: 48 kHz mono. Lost packets are concealed by libopus PLC, and FEC recovers a
/// lost frame from the packet that follows it.
pub struct OpusDecoder {
    raw: NonNull<ffi::OpusDecoder>,
}

// SAFETY: libopus decoder state has no thread affinity and is only used through `&mut self`.
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    /// Creates a decoder.
    pub fn new() -> Result<Self> {
        let mut err: c_int = 0;
        // SAFETY: plain FFI constructor. The status and the returned pointer are checked below.
        let raw = unsafe { ffi::opus_decoder_create(SAMPLE_RATE as i32, 1, &mut err) };
        check_ret("opus_decoder_create", err)?;
        let raw = NonNull::new(raw).ok_or(AudioError::Opus {
            call: "opus_decoder_create",
            code: -7,
        })?;
        Ok(OpusDecoder { raw })
    }

    fn run(&mut self, packet: Option<&[u8]>, out: &mut [f32], fec: bool) -> Result<usize> {
        check_len(FRAME_SAMPLES, out.len())?;
        let (ptr, len) = match packet {
            Some(p) => (p.as_ptr(), p.len() as i32),
            None => (std::ptr::null(), 0),
        };
        // SAFETY: `ptr` is null with len 0 (PLC) or points at `len` readable bytes. `out` holds
        // FRAME_SAMPLES floats.
        let n = unsafe {
            ffi::opus_decode_float(
                self.raw.as_ptr(),
                ptr,
                len,
                out.as_mut_ptr(),
                FRAME_SAMPLES as c_int,
                c_int::from(fec),
            )
        };
        Ok(check_ret("opus_decode_float", n)? as usize)
    }
}

impl Decoder for OpusDecoder {
    fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> Result<usize> {
        self.run(packet, out, false)
    }

    fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32]) -> Result<usize> {
        self.run(Some(next_packet), out, true)
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: `raw` came from opus_decoder_create and is destroyed exactly once.
        unsafe { ffi::opus_decoder_destroy(self.raw.as_ptr()) }
    }
}

impl<D: Decoder + ?Sized> Decoder for Box<D> {
    fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> Result<usize> {
        (**self).decode(packet, out)
    }

    fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32]) -> Result<usize> {
        (**self).decode_fec(next_packet, out)
    }
}

/// Test-only passthrough encoder. Frames are sent as 16-bit little-endian PCM, so the jitter
/// buffer and pipeline can be tested without Opus. It has no redundancy.
#[derive(Debug, Default)]
pub struct PcmEncoder;

/// Test-only passthrough decoder for [`PcmEncoder`]. Concealment is silence and FEC is unsupported.
#[derive(Debug, Default)]
pub struct PcmDecoder;

impl Encoder for PcmEncoder {
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        check_len(FRAME_SAMPLES, pcm.len())?;
        Ok(pcm
            .iter()
            .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
            .collect())
    }

    fn set_bitrate(&mut self, _bps: u32) -> Result<()> {
        Ok(())
    }

    fn set_loss_percent(&mut self, _percent: u8) -> Result<()> {
        Ok(())
    }
}

impl Decoder for PcmDecoder {
    fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> Result<usize> {
        check_len(FRAME_SAMPLES, out.len())?;
        match packet {
            None => out.fill(0.0),
            Some(p) => {
                check_len(FRAME_SAMPLES * 2, p.len())?;
                for (dst, src) in out.iter_mut().zip(p.as_chunks::<2>().0) {
                    *dst = f32::from(i16::from_le_bytes(*src)) / 32767.0;
                }
            }
        }
        Ok(FRAME_SAMPLES)
    }

    fn decode_fec(&mut self, _next_packet: &[u8], _out: &mut [f32]) -> Result<usize> {
        Err(AudioError::Unsupported("forward error correction"))
    }
}
