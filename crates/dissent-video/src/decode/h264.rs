//! H.264 decoder on OpenH264. It shares the encoder's bundled source, so no system library is needed.

use openh264::decoder::{DecodedYUV, Decoder};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

use super::VideoDecoder;
use crate::error::{Result, VideoError};
use crate::frame::{Frame, PixelFormat};

pub struct H264Decoder {
    decoder: Decoder,
}

impl H264Decoder {
    pub fn new() -> Result<Self> {
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), Default::default())
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        Ok(Self { decoder })
    }
}

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Vec<Frame>> {
        let picture = self
            .decoder
            .decode(data)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        match picture {
            None => Ok(Vec::new()),
            Some(yuv) => Ok(vec![copy_picture(&yuv, pts_us)?]),
        }
    }
}

/// Copies an OpenH264 picture into a packed I420 frame, dropping the row padding.
fn copy_picture(yuv: &DecodedYUV<'_>, pts_us: u64) -> Result<Frame> {
    let (w, h) = yuv.dimensions();
    let (ys, us, vs) = yuv.strides();
    let mut out = Vec::with_capacity(w * h + 2 * (w / 2) * (h / 2));
    for (plane, stride, pw, ph) in [
        (yuv.y(), ys, w, h),
        (yuv.u(), us, w / 2, h / 2),
        (yuv.v(), vs, w / 2, h / 2),
    ] {
        for row in 0..ph {
            out.extend_from_slice(&plane[row * stride..row * stride + pw]);
        }
    }
    Frame::new(w as u32, h as u32, PixelFormat::I420, pts_us, out)
}
