//! The encoder side: raw frames in, compressed chunks out.

pub mod h264;
#[cfg(feature = "svt-av1")]
pub mod svt;

#[cfg(feature = "svt-av1")]
use std::collections::VecDeque;

use crate::codec::Codec;
use crate::error::{Result, VideoError};
use crate::frame::Frame;

/// Settings an encoder is built with. Size and rate are fixed for one encoder instance.
/// A resolution or fps change means building a new encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Frames between forced keyframes. A keyframe is always the first frame.
    pub keyframe_interval: u32,
}

/// One compressed unit of video, ready for the packet queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedChunk {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Presentation time, taken from the input frame that produced this chunk.
    pub pts_us: u64,
}

/// A live encoder. Output can lag input because of look-ahead, so `encode` may return
/// nothing for some frames and several chunks for others.
pub trait VideoEncoder: Send {
    fn config(&self) -> EncoderConfig;

    /// Feeds one frame. The frame must match the configured size.
    fn encode(&mut self, frame: &Frame) -> Result<Vec<EncodedChunk>>;

    /// Makes the next encoded frame a keyframe.
    fn request_keyframe(&mut self);

    /// Changes the target bitrate. The change applies from the next frame. Backends that
    /// cannot change rate in place rebuild their encoder, which emits a keyframe.
    fn set_bitrate(&mut self, kbps: u32) -> Result<()>;
}

/// Builds an encoder for `codec`.
pub fn open(codec: Codec, config: EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
    Ok(match codec {
        #[cfg(feature = "svt-av1")]
        Codec::Av1 => Box::new(svt::SvtEncoder::new(config)?),
        #[cfg(not(feature = "svt-av1"))]
        Codec::Av1 => {
            return Err(VideoError::Unsupported(
                "AV1 encoding needs the svt-av1 feature",
            ))
        }
        Codec::H264 => Box::new(h264::H264Encoder::new(config)?),
    })
}

pub(crate) fn validate(config: &EncoderConfig) -> Result<()> {
    if config.width == 0
        || config.height == 0
        || !config.width.is_multiple_of(2)
        || !config.height.is_multiple_of(2)
    {
        return Err(VideoError::InvalidSize {
            width: config.width,
            height: config.height,
        });
    }
    if config.fps == 0 || config.bitrate_kbps == 0 || config.keyframe_interval == 0 {
        return Err(VideoError::Encode(
            "fps, bitrate and keyframe interval must be above zero".into(),
        ));
    }
    Ok(())
}

/// Rejects a frame whose size differs from the encoder's.
pub(crate) fn check_frame(frame: &Frame, config: &EncoderConfig) -> Result<()> {
    if frame.width() != config.width || frame.height() != config.height {
        return Err(VideoError::Encode(format!(
            "frame is {}x{}, encoder is {}x{}",
            frame.width(),
            frame.height(),
            config.width,
            config.height
        )));
    }
    Ok(())
}

/// Maps an encoder's output index back to the timestamp of the input frame it came from.
///
/// Encoders with look-ahead emit frames in input order, so output index `n` is the frame that
/// was submitted `n` frames after the index this queue started at.
#[cfg(feature = "svt-av1")]
pub(crate) struct PtsQueue {
    pts: VecDeque<u64>,
    base: u64,
    last: u64,
}

#[cfg(feature = "svt-av1")]
impl PtsQueue {
    pub(crate) fn new() -> Self {
        Self {
            pts: VecDeque::new(),
            base: 0,
            last: 0,
        }
    }

    pub(crate) fn push(&mut self, pts_us: u64) {
        self.pts.push_back(pts_us);
    }

    pub(crate) fn take(&mut self, index: u64) -> u64 {
        let position = index.saturating_sub(self.base) as usize;
        let pts = self.pts.get(position).copied().unwrap_or(self.last);
        self.pts.drain(..(position + 1).min(self.pts.len()));
        self.base = index + 1;
        self.last = pts;
        pts
    }
}
