//! Webcam capture through nokhwa. Media Foundation on Windows, AVFoundation on macOS and
//! V4L2 on Linux. The device picks the mode closest to the one asked for, and the source
//! scales whatever arrives to the requested size.

use std::time::Instant;

use image::imageops::FilterType;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
};
use nokhwa::Camera;

use crate::error::{Result, VideoError};
use crate::frame::{from_packed_rgb, Frame};
use crate::source::{CaptureFormat, VideoSource};

/// A camera the system reports, by the index that `CameraSource::open` takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    pub index: u32,
    pub name: String,
}

/// Lists the cameras the platform backend can open. Cameras reported by index only are listed.
pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    let devices = nokhwa::query(ApiBackend::Auto).map_err(capture_error)?;
    Ok(devices
        .iter()
        .filter_map(|info| match info.index() {
            CameraIndex::Index(index) => Some(CameraDevice {
                index: *index,
                name: info.human_name(),
            }),
            CameraIndex::String(_) => None,
        })
        .collect())
}

/// The mode to run for a plan of `wanted`: the smallest mode at or above that size that runs at or
/// above its frame rate. Among modes of that size, NV12 comes first, then MJPEG, then anything else
/// (YUYV usually caps at 5 to 10 fps over USB at 1080p). On the Insta360 Link 2 Pro, NV12 at 1080p30
/// delivered 29 fps, and MJPEG at 1080p30 and 720p30 delivered about 17. `None` when no mode fits,
/// and the default mode is used.
pub fn pick_mode(modes: &[CameraFormat], wanted: CaptureFormat) -> Option<CameraFormat> {
    modes
        .iter()
        .filter(|mode| {
            let size = mode.resolution();
            size.width() >= wanted.width && size.height() >= wanted.height && mode.frame_rate() >= wanted.fps
        })
        .min_by_key(|mode| {
            let size = mode.resolution();
            (
                u64::from(size.width()) * u64::from(size.height()),
                format_rank(mode.format()),
                mode.frame_rate(),
            )
        })
        .copied()
}

/// How much a format is preferred when two modes have the same size. Lower is better.
fn format_rank(format: FrameFormat) -> u8 {
    match format {
        FrameFormat::NV12 => 0,
        FrameFormat::MJPEG => 1,
        _ => 2,
    }
}

pub struct CameraSource {
    camera: Camera,
    format: CaptureFormat,
    started: Instant,
}

impl CameraSource {
    /// Opens camera `index` and starts its stream. `format` is the size and rate to aim for.
    ///
    /// The camera is opened on its default mode first, which lists the modes it really has. Then
    /// the mode nearest the request is set. A device that cannot set it keeps the default.
    pub fn open(index: u32, format: CaptureFormat) -> Result<Self> {
        let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
        let mut camera = Camera::new(CameraIndex::Index(index), any).map_err(capture_error)?;
        let modes = camera.compatible_camera_formats().unwrap_or_default();
        if let Some(mode) = pick_mode(&modes, format) {
            let exact = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(mode));
            let _ = camera.set_camera_requset(exact);
        }
        camera.open_stream().map_err(capture_error)?;
        Ok(Self {
            camera,
            format,
            started: Instant::now(),
        })
    }

    /// The mode the device actually delivers, before scaling.
    pub fn device_format(&self) -> CaptureFormat {
        let mode = self.camera.camera_format();
        CaptureFormat {
            width: mode.resolution().width(),
            height: mode.resolution().height(),
            fps: mode.frame_rate(),
        }
    }
}

impl VideoSource for CameraSource {
    fn format(&self) -> CaptureFormat {
        self.format
    }

    fn next_frame(&mut self) -> Result<Frame> {
        let buffer = self.camera.frame().map_err(capture_error)?;
        let decoded = buffer.decode_image::<RgbFormat>().map_err(capture_error)?;
        let (w, h) = (self.format.width, self.format.height);
        let scaled = if decoded.dimensions() == (w, h) {
            decoded
        } else {
            image::imageops::resize(&decoded, w, h, FilterType::Triangle)
        };
        let pts_us = self.started.elapsed().as_micros() as u64;
        from_packed_rgb(w, h, 3, pts_us, scaled.as_raw())
    }
}

pub(crate) fn capture_error(error: impl std::fmt::Display) -> VideoError {
    VideoError::Capture(error.to_string())
}

#[cfg(test)]
mod tests {
    use nokhwa::utils::Resolution;

    use super::*;

    fn mode(width: u32, height: u32, format: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new(Resolution::new(width, height), format, fps)
    }

    const WANT_720P30: CaptureFormat = CaptureFormat {
        width: 1280,
        height: 720,
        fps: 30,
    };
    const WANT_1080P30: CaptureFormat = CaptureFormat {
        width: 1920,
        height: 1080,
        fps: 30,
    };

    #[test]
    fn the_smallest_mode_at_the_size_and_rate_is_picked() {
        let modes = [
            mode(1920, 1080, FrameFormat::MJPEG, 30),
            mode(1280, 720, FrameFormat::MJPEG, 30),
            mode(640, 480, FrameFormat::MJPEG, 30),
        ];
        assert_eq!(pick_mode(&modes, WANT_720P30), Some(modes[1]));
    }

    #[test]
    fn a_mode_too_slow_or_too_small_is_never_picked() {
        let modes = [
            mode(1280, 720, FrameFormat::MJPEG, 15),
            mode(1024, 768, FrameFormat::MJPEG, 30),
            mode(1920, 1080, FrameFormat::MJPEG, 30),
        ];
        assert_eq!(pick_mode(&modes, WANT_720P30), Some(modes[2]));
    }

    #[test]
    fn at_one_size_nv12_then_mjpeg_then_yuyv() {
        let all = [
            mode(1920, 1080, FrameFormat::YUYV, 30),
            mode(1920, 1080, FrameFormat::MJPEG, 30),
            mode(1920, 1080, FrameFormat::NV12, 30),
        ];
        assert_eq!(pick_mode(&all, WANT_1080P30).map(|m| m.format()), Some(FrameFormat::NV12));
        let without_nv12 = [all[0], all[1]];
        assert_eq!(pick_mode(&without_nv12, WANT_1080P30).map(|m| m.format()), Some(FrameFormat::MJPEG));
    }

    #[test]
    fn size_comes_before_format() {
        let modes = [
            mode(1920, 1080, FrameFormat::NV12, 30),
            mode(1280, 720, FrameFormat::MJPEG, 30),
        ];
        assert_eq!(pick_mode(&modes, WANT_720P30), Some(modes[1]));
    }

    #[test]
    fn yuyv_is_used_only_when_nothing_else_fits() {
        let modes = [
            mode(1280, 720, FrameFormat::YUYV, 30),
            mode(1920, 1080, FrameFormat::MJPEG, 15),
        ];
        assert_eq!(pick_mode(&modes, WANT_720P30), Some(modes[0]));
    }

    #[test]
    fn nothing_that_fits_gives_no_mode() {
        let modes = [mode(640, 480, FrameFormat::MJPEG, 30)];
        assert_eq!(pick_mode(&modes, WANT_720P30), None);
    }
}
