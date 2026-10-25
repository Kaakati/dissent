//! Video for Dissent calls: capture, AV1 and H.264 codecs, bitrate adaptation and send priority.
//!
//! Each piece sits behind a trait so the media layer can swap backends and tests can use
//! deterministic sources. Nothing here touches the network. The media crate wires these
//! parts into a call in a later phase.

pub mod adapt;
pub mod camera;
pub mod codec;
pub mod decode;
pub mod encode;
pub mod error;
pub mod frame;
pub mod metrics;
pub mod pattern;
pub mod queue;
pub mod screen;
pub mod source;

pub use codec::Codec;
pub use decode::VideoDecoder;
pub use encode::{EncodedChunk, EncoderConfig, VideoEncoder};
pub use error::{Result, VideoError};
pub use frame::{Frame, PixelFormat};
pub use source::{CaptureFormat, VideoSource};
