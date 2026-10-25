//! The decoder side: compressed chunks in, raw frames out.

pub mod av1;
pub mod h264;

use crate::codec::Codec;
use crate::error::Result;
use crate::frame::Frame;

/// A live decoder. It buffers as little as it can, so a chunk usually yields its frame on
/// the same call.
pub trait VideoDecoder: Send {
    /// Decodes one chunk. The returned frames take `pts_us` from the chunk, which is exact
    /// for real-time streams with no hidden frames.
    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Vec<Frame>>;
}

/// Builds a decoder for `codec`.
pub fn open(codec: Codec) -> Result<Box<dyn VideoDecoder>> {
    Ok(match codec {
        Codec::Av1 => Box::new(av1::Av1Decoder::new()?),
        Codec::H264 => Box::new(h264::H264Decoder::new()?),
    })
}
