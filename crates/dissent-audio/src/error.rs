//! Error type for the voice pipeline.

use thiserror::Error;

/// Failures from the codec, the device layer or the pipeline.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioError {
    /// libopus returned a negative error code.
    #[error("libopus {call} failed with code {code}")]
    Opus { call: &'static str, code: i32 },
    /// A caller passed a buffer or frame of the wrong size.
    #[error("expected {expected} samples, got {got}")]
    FrameSize { expected: usize, got: usize },
    /// The codec cannot perform the requested operation, such as FEC on PCM.
    #[error("codec does not support {0}")]
    Unsupported(&'static str),
    /// The audio device or host failed. The message comes from cpal.
    #[error("audio device: {0}")]
    Device(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, AudioError>;
