//! Raw video frames and the colour conversions the encoders need.
//!
//! Every frame has even dimensions, so the chroma planes of 4:2:0 formats are exact.

use std::borrow::Cow;

use crate::error::{Result, VideoError};

/// Pixel layouts this crate moves between capture, encoder and decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// Planar 4:2:0: a full Y plane, then U, then V, each at half width and height.
    I420,
    /// Semi-planar 4:2:0: a full Y plane, then interleaved UV pairs at half size.
    Nv12,
}

/// One captured or decoded picture. The buffer always holds exactly one frame in `format`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    format: PixelFormat,
    pts_us: u64,
    data: Vec<u8>,
}

impl Frame {
    /// Wraps `data`, which must hold exactly one frame of `format`.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        pts_us: u64,
        data: Vec<u8>,
    ) -> Result<Self> {
        check_size(width, height)?;
        let expected = frame_len(width, height);
        if data.len() != expected {
            return Err(VideoError::FrameSize {
                got: data.len(),
                expected,
            });
        }
        Ok(Self {
            width,
            height,
            format,
            pts_us,
            data,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Presentation time in microseconds from the start of the source.
    pub fn pts_us(&self) -> u64 {
        self.pts_us
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The luma plane. Both formats store it first and unpadded.
    pub fn luma(&self) -> &[u8] {
        &self.data[..(self.width * self.height) as usize]
    }

    /// Returns this frame as I420, converting from NV12 when needed.
    pub fn to_i420(&self) -> Cow<'_, Frame> {
        match self.format {
            PixelFormat::I420 => Cow::Borrowed(self),
            PixelFormat::Nv12 => Cow::Owned(self.nv12_to_i420()),
        }
    }

    fn nv12_to_i420(&self) -> Frame {
        let luma = (self.width * self.height) as usize;
        let chroma = luma / 4;
        let uv = &self.data[luma..];
        let mut out = Vec::with_capacity(frame_len(self.width, self.height));
        out.extend_from_slice(&self.data[..luma]);
        out.extend(uv.iter().step_by(2).take(chroma));
        out.extend(uv.iter().skip(1).step_by(2).take(chroma));
        Frame {
            width: self.width,
            height: self.height,
            format: PixelFormat::I420,
            pts_us: self.pts_us,
            data: out,
        }
    }
}

/// Converts packed 8-bit RGB or RGBA pixels to I420 with BT.601 limited-range coefficients.
///
/// `bytes_per_pixel` is 3 for RGB and 4 for RGBA. The alpha byte is ignored.
pub fn from_packed_rgb(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    pts_us: u64,
    rgb: &[u8],
) -> Result<Frame> {
    check_size(width, height)?;
    let (w, h) = (width as usize, height as usize);
    let expected = w * h * bytes_per_pixel;
    if rgb.len() != expected {
        return Err(VideoError::FrameSize {
            got: rgb.len(),
            expected,
        });
    }
    let pixel = |x: usize, y: usize| {
        let i = (y * w + x) * bytes_per_pixel;
        (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32)
    };

    let mut out = vec![0u8; frame_len(width, height)];
    let (y_plane, chroma) = out.split_at_mut(w * h);
    let (u_plane, v_plane) = chroma.split_at_mut(w * h / 4);

    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = pixel(x, y);
            y_plane[y * w + x] = luma_from_rgb(r, g, b);
        }
    }
    for cy in 0..h / 2 {
        for cx in 0..w / 2 {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (pr, pg, pb) = pixel(cx * 2 + dx, cy * 2 + dy);
                r += pr;
                g += pg;
                b += pb;
            }
            let i = cy * (w / 2) + cx;
            let (r, g, b) = ((r + 2) / 4, (g + 2) / 4, (b + 2) / 4);
            u_plane[i] = chroma_u_from_rgb(r, g, b);
            v_plane[i] = chroma_v_from_rgb(r, g, b);
        }
    }
    Frame::new(width, height, PixelFormat::I420, pts_us, out)
}

fn luma_from_rgb(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8
}

fn chroma_u_from_rgb(r: i32, g: i32, b: i32) -> u8 {
    (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8
}

fn chroma_v_from_rgb(r: i32, g: i32, b: i32) -> u8 {
    (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(16, 240) as u8
}

fn check_size(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(VideoError::InvalidSize { width, height });
    }
    Ok(())
}

/// Bytes in one 4:2:0 frame of the given size. Both supported formats use this.
pub fn frame_len(width: u32, height: u32) -> usize {
    let luma = (width * height) as usize;
    luma + luma / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_odd_or_empty_sizes() {
        assert!(matches!(
            Frame::new(3, 4, PixelFormat::I420, 0, vec![0; 18]),
            Err(VideoError::InvalidSize { .. })
        ));
        assert!(matches!(
            from_packed_rgb(0, 4, 3, 0, &[]),
            Err(VideoError::InvalidSize { .. })
        ));
    }

    #[test]
    fn rejects_wrong_buffer_length() {
        assert_eq!(
            Frame::new(4, 4, PixelFormat::I420, 0, vec![0; 10]),
            Err(VideoError::FrameSize {
                got: 10,
                expected: 24
            })
        );
    }

    #[test]
    fn white_and_black_map_to_limited_range_extremes() {
        let white = from_packed_rgb(2, 2, 3, 0, &[255; 12]).unwrap();
        assert!(white.luma().iter().all(|&y| y == 235));
        assert!(white.data()[4..].iter().all(|&c| c == 128));

        let black = from_packed_rgb(2, 2, 4, 0, &[0; 16]).unwrap();
        assert!(black.luma().iter().all(|&y| y == 16));
    }

    #[test]
    fn nv12_deinterleaves_chroma() {
        let mut data = vec![7u8; 16];
        data.extend_from_slice(&[10, 20, 30, 40, 50, 60, 70, 80]);
        let nv12 = Frame::new(4, 4, PixelFormat::Nv12, 5, data).unwrap();
        let i420 = nv12.to_i420();
        assert_eq!(i420.format(), PixelFormat::I420);
        assert_eq!(i420.pts_us(), 5);
        assert_eq!(&i420.data()[16..20], &[10, 30, 50, 70]);
        assert_eq!(&i420.data()[20..24], &[20, 40, 60, 80]);
    }
}
