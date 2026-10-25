//! Screen and window capture through xcap, on Windows and macOS.
//!
//! xcap's Linux backend needs the pipewire, wayland, EGL and xcb development packages at
//! build time. The CI image does not install them, so Linux builds get the same API with
//! every call returning `VideoError::Unsupported`. Adding Linux capture means adding those
//! packages to the workflow first.
//!
//! Capture is pulled, not pushed. `next_frame` sleeps to the frame rate and then grabs the
//! target, so a slow grab shows up as a lower real rate rather than as a backlog.

use std::thread;
use std::time::{Duration, Instant};

use image::imageops::FilterType;
use image::RgbaImage;

#[cfg(not(target_os = "linux"))]
use crate::camera::capture_error;
use crate::error::{Result, VideoError};
use crate::frame::{from_packed_rgb, Frame};
use crate::source::{CaptureFormat, VideoSource};

#[cfg(target_os = "linux")]
const LINUX_UNSUPPORTED: &str = "screen capture on Linux needs the xcap Linux backend";

/// A display the system reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// A visible top-level window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app: String,
}

/// What to capture: a whole display, or one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenTarget {
    Display(u32),
    Window(u32),
}

/// Whether the pointer appears in the capture. xcap's backends do not draw the cursor, so
/// only `Excluded` is supported today. `Included` returns an error and is kept so the
/// option exists in the API when a backend can honour it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Excluded,
    Included,
}

#[cfg(not(target_os = "linux"))]
pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    let monitors = xcap::Monitor::all().map_err(capture_error)?;
    monitors
        .iter()
        .map(|m| {
            Ok(DisplayInfo {
                id: m.id().map_err(capture_error)?,
                name: m.name().map_err(capture_error)?,
                width: m.width().map_err(capture_error)?,
                height: m.height().map_err(capture_error)?,
                primary: m.is_primary().map_err(capture_error)?,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    Err(VideoError::Unsupported(LINUX_UNSUPPORTED))
}

/// Lists windows that are on screen with a title. Minimised windows are left out.
#[cfg(not(target_os = "linux"))]
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let windows = xcap::Window::all().map_err(capture_error)?;
    let mut out = Vec::new();
    for w in &windows {
        let title = w.title().map_err(capture_error)?;
        if title.is_empty() || w.is_minimized().map_err(capture_error)? {
            continue;
        }
        out.push(WindowInfo {
            id: w.id().map_err(capture_error)?,
            title,
            app: w.app_name().map_err(capture_error)?,
        });
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    Err(VideoError::Unsupported(LINUX_UNSUPPORTED))
}

pub struct ScreenSource {
    target: ScreenTarget,
    format: CaptureFormat,
    period: Duration,
    next_due: Instant,
    started: Instant,
}

impl ScreenSource {
    /// Checks that the target exists now, so a missing display or window fails here rather
    /// than on the first frame. Each frame looks the target up again, because xcap's
    /// handles are not `Send` on every platform.
    pub fn open(target: ScreenTarget, format: CaptureFormat, cursor: Cursor) -> Result<Self> {
        if cursor == Cursor::Included {
            return Err(VideoError::Unsupported("capturing the cursor"));
        }
        grab(target)?;
        let now = Instant::now();
        Ok(Self {
            target,
            format,
            period: Duration::from_micros(1_000_000 / format.fps.max(1) as u64),
            next_due: now,
            started: now,
        })
    }
}

impl VideoSource for ScreenSource {
    fn format(&self) -> CaptureFormat {
        self.format
    }

    fn next_frame(&mut self) -> Result<Frame> {
        let now = Instant::now();
        if now < self.next_due {
            thread::sleep(self.next_due - now);
        }
        self.next_due = Instant::now().max(self.next_due) + self.period;

        let grabbed = grab(self.target)?;
        let (w, h) = (self.format.width, self.format.height);
        let scaled = if grabbed.dimensions() == (w, h) {
            grabbed
        } else {
            image::imageops::resize(&grabbed, w, h, FilterType::Triangle)
        };
        let pts_us = self.started.elapsed().as_micros() as u64;
        from_packed_rgb(w, h, 4, pts_us, scaled.as_raw())
    }
}

/// Looks up `target` and captures it at native size.
#[cfg(not(target_os = "linux"))]
fn grab(target: ScreenTarget) -> Result<RgbaImage> {
    match target {
        ScreenTarget::Display(id) => {
            let monitor = xcap::Monitor::all()
                .map_err(capture_error)?
                .into_iter()
                .find(|m| m.id().ok() == Some(id))
                .ok_or_else(|| VideoError::Capture(format!("no display with id {id}")))?;
            monitor.capture_image().map_err(capture_error)
        }
        ScreenTarget::Window(id) => {
            let window = xcap::Window::all()
                .map_err(capture_error)?
                .into_iter()
                .find(|w| w.id().ok() == Some(id))
                .ok_or_else(|| VideoError::Capture(format!("no window with id {id}")))?;
            window.capture_image().map_err(capture_error)
        }
    }
}

#[cfg(target_os = "linux")]
fn grab(_target: ScreenTarget) -> Result<RgbaImage> {
    Err(VideoError::Unsupported(LINUX_UNSUPPORTED))
}
