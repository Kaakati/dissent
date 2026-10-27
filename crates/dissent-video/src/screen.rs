//! Screen and window capture through xcap, on Windows, macOS and Linux.
//!
//! A display on Linux is a stream. xcap's recorder reads the X11 root window, or opens a
//! ScreenCast session through the desktop portal on Wayland, which shows the system's own
//! picker. Other targets are grabbed when a frame is due. Each Wayland grab would be a separate
//! portal screenshot, so window capture is for X11, Windows and macOS.
//!
//! Capture is pulled, not pushed. `next_frame` sleeps to the frame rate and then takes the
//! newest picture, so a slow grab shows up as a lower real rate rather than as a backlog.

use std::borrow::Cow;
#[cfg(target_os = "linux")]
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use image::imageops::FilterType;
use image::RgbaImage;

use crate::camera::capture_error;
use crate::error::{Result, VideoError};
use crate::frame::{from_packed_rgb, Frame};
use crate::source::{CaptureFormat, VideoSource};

/// How long the first picture of a display stream may take. On Wayland that includes the portal's
/// picker, which waits for the user.
#[cfg(target_os = "linux")]
const FIRST_PICTURE: Duration = Duration::from_secs(30);

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

/// Lists windows that are on screen with a title. Minimised windows are left out.
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

pub struct ScreenSource {
    feed: Feed,
    format: CaptureFormat,
    period: Duration,
    next_due: Instant,
    started: Instant,
}

/// Where the pictures come from.
enum Feed {
    /// Each frame grabs the target again. Each frame looks the target up, because xcap's handles
    /// are not `Send` on every platform.
    Grab(ScreenTarget),
    #[cfg(target_os = "linux")]
    Stream(Stream),
}

/// A display stream on Linux. The desktop sends a picture when the screen changes, so the last one
/// is kept and repeated until a new one arrives.
#[cfg(target_os = "linux")]
struct Stream {
    recorder: xcap::VideoRecorder,
    pictures: Receiver<xcap::Frame>,
    latest: Option<RgbaImage>,
}

#[cfg(target_os = "linux")]
impl Stream {
    /// The newest picture, waiting for the first one if none has arrived yet.
    fn newest(&mut self) -> Result<&RgbaImage> {
        let mut arrived = None;
        while let Ok(frame) = self.pictures.try_recv() {
            arrived = Some(frame);
        }
        if arrived.is_none() && self.latest.is_none() {
            arrived = self.pictures.recv_timeout(FIRST_PICTURE).ok();
        }
        if let Some(frame) = arrived {
            self.latest = Some(
                RgbaImage::from_raw(frame.width, frame.height, frame.raw)
                    .ok_or_else(|| VideoError::Capture("screen picture has the wrong size".into()))?,
            );
        }
        self.latest
            .as_ref()
            .ok_or_else(|| VideoError::Capture("the screen stream sent no picture".into()))
    }
}

#[cfg(target_os = "linux")]
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.recorder.stop();
    }
}

impl ScreenSource {
    /// Checks that the target exists now, so a missing display or window fails here rather
    /// than on the first frame.
    pub fn open(target: ScreenTarget, format: CaptureFormat, cursor: Cursor) -> Result<Self> {
        if cursor == Cursor::Included {
            return Err(VideoError::Unsupported("capturing the cursor"));
        }
        let feed = Feed::open(target)?;
        let now = Instant::now();
        Ok(Self {
            feed,
            format,
            period: Duration::from_micros(1_000_000 / format.fps.max(1) as u64),
            next_due: now,
            started: now,
        })
    }
}

impl Feed {
    fn open(target: ScreenTarget) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if let ScreenTarget::Display(id) = target {
                let (recorder, pictures) = find_monitor(id)?.video_recorder().map_err(capture_error)?;
                recorder.start().map_err(capture_error)?;
                return Ok(Self::Stream(Stream {
                    recorder,
                    pictures,
                    latest: None,
                }));
            }
        }
        grab(target)?;
        Ok(Self::Grab(target))
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

        let owned;
        let picture: &RgbaImage = match &mut self.feed {
            Feed::Grab(target) => {
                owned = grab(*target)?;
                &owned
            }
            #[cfg(target_os = "linux")]
            Feed::Stream(stream) => stream.newest()?,
        };
        let (w, h) = (self.format.width, self.format.height);
        let scaled = if picture.dimensions() == (w, h) {
            Cow::Borrowed(picture)
        } else {
            Cow::Owned(image::imageops::resize(picture, w, h, FilterType::Triangle))
        };
        let pts_us = self.started.elapsed().as_micros() as u64;
        from_packed_rgb(w, h, 4, pts_us, scaled.as_raw())
    }
}

fn find_monitor(id: u32) -> Result<xcap::Monitor> {
    xcap::Monitor::all()
        .map_err(capture_error)?
        .into_iter()
        .find(|m| m.id().ok() == Some(id))
        .ok_or_else(|| VideoError::Capture(format!("no display with id {id}")))
}

/// Looks up `target` and captures it at native size.
fn grab(target: ScreenTarget) -> Result<RgbaImage> {
    match target {
        ScreenTarget::Display(id) => find_monitor(id)?.capture_image().map_err(capture_error),
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
