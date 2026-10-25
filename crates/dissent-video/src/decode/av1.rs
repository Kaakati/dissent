//! AV1 decoder on rav1d, a pure Rust port of dav1d. Built without its assembly routines,
//! so no nasm is needed. The decoder exposes a dav1d-shaped C API, and this module is the
//! only place that calls it.
//!
//! The unsafe blocks rely on three rules from rav1d's API:
//! - `dav1d_data_create` allocates a buffer that `dav1d_send_data` takes ownership of
//!   once it succeeds. On `EAGAIN` the caller still owns the buffer and must resend it.
//! - `dav1d_get_picture` fills a picture that the caller must release with
//!   `dav1d_picture_unref` after copying its planes.
//! - `dav1d_close` takes the context back out of the option it was opened into.

use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::slice;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};
use rav1d::Dav1dResult;

use super::VideoDecoder;
use crate::error::{Result, VideoError};
use crate::frame::{Frame, PixelFormat};

/// dav1d reports errors as negated errno values. `EAGAIN` means "call the other side first".
/// The errno value is platform-specific, and rav1d's error enum is not public.
#[cfg(target_os = "macos")]
const EAGAIN: i32 = -35;
#[cfg(not(target_os = "macos"))]
const EAGAIN: i32 = -11;

pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
}

// SAFETY: the context is only reached through `&mut self`, so one thread uses it at a time.
// rav1d's context holds no thread-affine state, so moving it to another thread is sound.
unsafe impl Send for Av1Decoder {}

impl Av1Decoder {
    /// Opens a decoder tuned for real-time calls: one thread and no frame delay.
    pub fn new() -> Result<Self> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` writes every field of the settings before it returns.
        let mut settings = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            settings.assume_init()
        };
        settings.n_threads = 1;
        settings.max_frame_delay = 1;

        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: both pointers refer to live locals, and `dav1d_open` writes the context into `ctx`.
        let result = unsafe {
            dav1d_open(
                Some(NonNull::from(&mut ctx)),
                Some(NonNull::from(&mut settings)),
            )
        };
        check(result)?;
        Ok(Self { ctx })
    }

    fn ctx(&self) -> Result<Dav1dContext> {
        self.ctx
            .ok_or_else(|| VideoError::Decode("decoder is closed".into()))
    }

    /// Drains every picture the decoder has ready.
    fn pictures(&mut self, pts_us: u64) -> Result<Vec<Frame>> {
        let mut out = Vec::new();
        loop {
            let mut picture = Dav1dPicture::default();
            // SAFETY: `picture` is a live local that `dav1d_get_picture` fills on success.
            let result =
                unsafe { dav1d_get_picture(self.ctx().ok(), Some(NonNull::from(&mut picture))) };
            if result.0 == EAGAIN {
                return Ok(out);
            }
            check(result)?;
            let frame = copy_picture(&picture, pts_us);
            // SAFETY: the picture was filled by `dav1d_get_picture` and its planes are copied.
            unsafe { dav1d_picture_unref(Some(NonNull::from(&mut picture))) };
            out.push(frame?);
        }
    }
}

impl VideoDecoder for Av1Decoder {
    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Vec<Frame>> {
        let mut input = Dav1dData::default();
        // SAFETY: `input` is a live local. The buffer is copied in before it is sent.
        let buffer = unsafe { dav1d_data_create(Some(NonNull::from(&mut input)), data.len()) };
        if buffer.is_null() {
            return Err(VideoError::Decode("out of memory for input".into()));
        }
        // SAFETY: `buffer` has room for `data.len()` bytes, as `dav1d_data_create` allocated it.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len()) };

        let mut out = Vec::new();
        loop {
            // SAFETY: `input` is live and still owns its buffer until `dav1d_send_data` succeeds.
            let result =
                unsafe { dav1d_send_data(self.ctx().ok(), Some(NonNull::from(&mut input))) };
            if result.0 == EAGAIN {
                // The decoder's queue is full. Drain pictures, then resend the same buffer.
                let drained = self.pictures(pts_us)?;
                if drained.is_empty() {
                    return Err(VideoError::Decode("decoder stalled".into()));
                }
                out.extend(drained);
                continue;
            }
            check(result)?;
            break;
        }
        out.extend(self.pictures(pts_us)?);
        Ok(out)
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        // SAFETY: `self.ctx` came from `dav1d_open`, and `dav1d_close` takes it back out.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
    }
}

fn check(result: Dav1dResult) -> Result<()> {
    if result.0 == 0 {
        Ok(())
    } else {
        Err(VideoError::Decode(format!("dav1d error {}", result.0)))
    }
}

/// Copies an 8-bit 4:2:0 picture into an owned I420 frame.
fn copy_picture(picture: &Dav1dPicture, pts_us: u64) -> Result<Frame> {
    if picture.p.layout != DAV1D_PIXEL_LAYOUT_I420 || picture.p.bpc != 8 {
        return Err(VideoError::Decode(
            "only 8-bit 4:2:0 pictures are supported".into(),
        ));
    }
    let (w, h) = (picture.p.w as usize, picture.p.h as usize);
    let (cw, ch) = (w / 2, h / 2);
    let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
    let planes = [(0, w, h), (1, cw, ch), (2, cw, ch)];
    for (plane, pw, ph) in planes {
        let base = picture.data[plane]
            .ok_or_else(|| VideoError::Decode("picture plane is missing".into()))?
            .as_ptr() as *const u8;
        let stride = picture.stride[(plane > 0) as usize] as usize;
        for row in 0..ph {
            // SAFETY: rav1d keeps `pw` bytes at `row * stride` inside each plane of the picture.
            let line = unsafe { slice::from_raw_parts(base.add(row * stride), pw) };
            out.extend_from_slice(line);
        }
    }
    Frame::new(
        picture.p.w as u32,
        picture.p.h as u32,
        PixelFormat::I420,
        pts_us,
        out,
    )
}
