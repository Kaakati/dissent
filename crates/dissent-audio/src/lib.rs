//! Voice pipeline: capture, processing, Opus, jitter buffer and playout. See
//! `docs/design/09-audio.md`.
//!
//! The crate is framework-free. It does not depend on Tauri or on the media transport. The
//! pipeline turns mic frames into packets, and packets back into playout frames.

pub mod chain;
pub mod codec;
pub mod device;
pub mod dsp;
mod error;
pub mod gate;
pub mod jitter;
mod opus_ffi;
pub mod meter;
pub mod pipeline;
pub mod resample;

pub use codec::{
    Decoder, Encoder, OpusDecoder, OpusEncoder, PcmDecoder, PcmEncoder, FRAME_SAMPLES, MAX_BITRATE,
    MAX_PACKET_BYTES, MIN_BITRATE, SAMPLE_RATE,
};
pub use chain::{CaptureChain, ChainSettings, FrameReport, STANDARD_STRENGTH, STRONG_STRENGTH};
pub use device::{
    default_input_name, default_output_name, kind_hint, list_inputs, list_outputs, CaptureStream,
    DeviceInfo, DeviceKind, PlayoutStream,
};
pub use dsp::{apply_gain, audio_level, Agc, EchoCanceller, NoiseSuppressor, VoiceDetector};
pub use gate::{Threshold, VoiceGate};
pub use meter::LevelMeter;
pub use error::{AudioError, Result};
pub use jitter::{JitterBuffer, JitterStats, Packet, Playout};
pub use pipeline::{SenderConfig, SenderPacket, VoiceReceiver, VoiceSender, QUIET_LEVEL};
