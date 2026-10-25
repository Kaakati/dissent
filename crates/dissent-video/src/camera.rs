//! Webcam capture through nokhwa. Media Foundation on Windows, AVFoundation on macOS and
//! V4L2 on Linux. The device picks the mode closest to the one asked for, and the source
//! scales whatever arrives to the requested size.
//!
//! Not built on Linux: nokhwa's V4L2 backend needs libclang, which a plain build must not need.
//! Linux camera capture is the next step, see `docs/design/04-calls.md`.

#![cfg(not(target_os = "linux"))]

use std::time::Instant;

use image::imageops::FilterType;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
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

pub struct CameraSource {
    camera: Camera,
    format: CaptureFormat,
    started: Instant,
}

impl CameraSource {
    /// Opens camera `index` and starts its stream. `format` is the size and rate to aim for.
    pub fn open(index: u32, format: CaptureFormat) -> Result<Self> {
        let wanted = CameraFormat::new(
            Resolution::new(format.width, format.height),
            FrameFormat::MJPEG,
            format.fps,
        );
        let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(wanted));
        let mut camera =
            Camera::new(CameraIndex::Index(index), requested).map_err(capture_error)?;
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
