//! Errors for capture, encoding, decoding and frame handling.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VideoError {
    #[error("invalid frame size {width}x{height}: both sides must be non-zero and even")]
    InvalidSize { width: u32, height: u32 },

    #[error("frame data is {got} bytes, expected {expected}")]
    FrameSize { got: usize, expected: usize },

    #[error("encoder: {0}")]
    Encode(String),

    #[error("decoder: {0}")]
    Decode(String),

    #[error("capture: {0}")]
    Capture(String),

    #[error("not supported on this platform or build: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, VideoError>;
