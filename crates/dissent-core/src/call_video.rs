//! Video for calls on this device: the camera and the screen it sends, and the previews of the
//! video it receives.
//!
//! Capture blocks, so each capture runs on its own thread. The thread captures, encodes with
//! H.264, and hands each encoded frame to the call loop, which sends it through the mesh or the
//! forwarder. The bitrate follows the link's bandwidth estimate, minus the audio reservation, so
//! video never takes the audio's share. Received frames are decoded per sender and sent to the
//! window as small JPEG previews.

use std::collections::{hash_map::Entry, HashMap};

use dissent_media::{VideoCodec, VideoFrame};

use crate::error::{CoreError, Result};

use dissent_video::Codec;

/// What a screen share shows: a whole display, or one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShareTarget {
    Display { id: u32 },
    Window { id: u32 },
}

/// A display or window the device can share, as the picker lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ShareSource {
    pub target: ShareTarget,
    pub name: String,
    /// True for the primary display, which a share uses when no target is picked.
    pub primary: bool,
}

/// Lists the displays and the titled windows that can be shared.
pub fn list_share_sources() -> Result<Vec<ShareSource>> {
    let mut out = Vec::new();
    for display in dissent_video::screen::list_displays().map_err(camera_error)? {
        out.push(ShareSource {
            target: ShareTarget::Display { id: display.id },
            name: display.name,
            primary: display.primary,
        });
    }
    for window in dissent_video::screen::list_windows().map_err(camera_error)? {
        out.push(ShareSource {
            target: ShareTarget::Window { id: window.id },
            name: if window.app.is_empty() {
                window.title
            } else {
                format!("{} · {}", window.app, window.title)
            },
            primary: false,
        });
    }
    Ok(out)
}

fn camera_error(e: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("capture: {e}"))
}

/// Width of the decoded previews sent to the window.
const PREVIEW_WIDTH: u32 = 320;
/// JPEG quality of the previews, 1 to 100. About 70 keeps a 320 px preview near 15 KB.
const PREVIEW_QUALITY: u8 = 70;
/// Frames per second the previews are timed at, when a frame carries no clock of its own.
const PREVIEW_FPS: u64 = 15;

/// One encoded frame the call loop sends.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub frame_id: u32,
    pub keyframe: bool,
    /// The codec the bytes are encoded with, named in the frame's header on the wire.
    pub codec: VideoCodec,
    pub data: Vec<u8>,
}

pub(crate) use capture::{Capture, CaptureKind};

/// Set by tests to send a deterministic test picture instead of the camera. Process-wide, so a
/// test sets it before it turns the camera on.
static TEST_PATTERN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes the next camera a deterministic test picture, for tests. Off by default.
pub fn set_test_camera_pattern(on: bool) {
    TEST_PATTERN.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Starts the camera for this device: the test picture when a test asked for it, else device 0.
pub(crate) fn start_camera() -> Result<Capture> {
    if TEST_PATTERN.load(std::sync::atomic::Ordering::Relaxed) {
        Capture::test_pattern()
    } else {
        Capture::camera(0)
    }
}

/// One thing this device sends: a capture, the budget that sizes its bitrate, and whether the
/// budget has paused it.
pub(crate) struct Sending {
    pub capture: Capture,
    pub budget: Budget,
    pub paused: bool,
}

impl Sending {
    pub(crate) fn new(capture: Capture) -> Self {
        let budget = Budget::new(capture.kind());
        Self {
            capture,
            budget,
            paused: false,
        }
    }

    /// Takes the link's estimate and the audio target and sizes the capture to fit: the video
    /// rate is what the estimate leaves after the audio reservation, and a budget too small for
    /// video pauses it. Without an estimate yet, nothing changes.
    pub(crate) fn fit(&mut self, available_kbps: Option<u32>, audio_kbps: u32) {
        let Some(available) = available_kbps else {
            return;
        };
        match self.budget.update(available, audio_kbps) {
            Some(kbps) => {
                self.paused = false;
                self.capture.set_bitrate(kbps);
            }
            None => self.paused = true,
        }
    }
}

mod capture {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{self, Receiver, TrySendError};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use dissent_video::encode::{self, EncoderConfig};
    use dissent_video::screen::{Cursor, ScreenSource, ScreenTarget};
    use dissent_video::{CaptureFormat, Codec, VideoSource};

    use dissent_media::VideoCodec;

    use super::{encode_jpeg, Encoded, Preview, PREVIEW_WIDTH};
    use crate::error::{CoreError, Result};

    /// A camera is 640x360 at 15 fps, which is the H.264 rung of the camera ladder.
    const CAMERA_FORMAT: CaptureFormat = CaptureFormat {
        width: 640,
        height: 360,
        fps: 15,
    };
    /// A screen is 640x360 at 10 fps, the H.264 rung of the screen ladder.
    const SCREEN_FORMAT: CaptureFormat = CaptureFormat {
        width: 640,
        height: 360,
        fps: 10,
    };
    /// A keyframe every two seconds at 15 fps, so a late joiner or a lost frame recovers quickly.
    const KEYFRAME_INTERVAL: u32 = 30;
    /// Frames the capture thread may queue for the call loop before it drops one and asks for a
    /// keyframe.
    const QUEUE: usize = 4;
    /// Every this many captured frames, the thread also makes a preview of its own picture for the
    /// window's self tile.
    const PREVIEW_EVERY: u32 = 3;
    /// Start rate before the first bandwidth estimate, in kbps. Inside the H.264 rung's range.
    const START_KBPS: u32 = 300;

    /// Which picture a capture sends, and so how it is paced.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum CaptureKind {
        Camera,
        Screen,
    }

    /// A running capture: its thread, the encoded frames it has made, and the controls the call
    /// loop turns. Dropping it stops the thread.
    pub(crate) struct Capture {
        kind: CaptureKind,
        stop: Arc<AtomicBool>,
        key_request: Arc<AtomicBool>,
        bitrate: Arc<AtomicU32>,
        /// Whether the capture should encode AV1. Set by the call loop, from what the peers agreed to.
        av1: Arc<AtomicBool>,
        rx: Receiver<Encoded>,
        previews: Receiver<Preview>,
        thread: Option<JoinHandle<()>>,
    }

    impl Capture {
        /// Starts a capture. `open` runs on the capture thread, because a camera may only be
        /// used from the thread that opened it, and any error from it comes back here.
        pub(crate) fn start(
            kind: CaptureKind,
            open: impl FnOnce() -> Result<Box<dyn VideoSource>> + Send + 'static,
        ) -> Result<Self> {
            let format = match kind {
                CaptureKind::Camera => CAMERA_FORMAT,
                CaptureKind::Screen => SCREEN_FORMAT,
            };
            let stop = Arc::new(AtomicBool::new(false));
            let key_request = Arc::new(AtomicBool::new(false));
            let bitrate = Arc::new(AtomicU32::new(START_KBPS));
            let (tx, rx) = mpsc::sync_channel::<Encoded>(QUEUE);
            let (preview_tx, previews) = mpsc::sync_channel::<Preview>(1);
            let screen = kind == CaptureKind::Screen;
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
            let thread_stop = stop.clone();
            let thread_key = key_request.clone();
            let thread_bitrate = bitrate.clone();
            let av1 = Arc::new(AtomicBool::new(false));
            let thread_av1 = av1.clone();
            let thread = std::thread::Builder::new()
                .name(format!("dissent-{kind:?}").to_lowercase())
                .spawn(move || {
                    let config = EncoderConfig {
                        width: format.width,
                        height: format.height,
                        fps: format.fps,
                        bitrate_kbps: START_KBPS,
                        keyframe_interval: KEYFRAME_INTERVAL,
                    };
                    let (mut source, mut encoder) = match open().and_then(|source| {
                        let encoder = encode::open(Codec::H264, config).map_err(capture_err)?;
                        Ok((source, encoder))
                    }) {
                        Ok(pair) => {
                            let _ = ready_tx.send(Ok(()));
                            pair
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let mut frame_id = 0u32;
                    let mut captured = 0u32;
                    let mut current = Codec::H264;
                    // AV1 is dropped for good once it cannot keep up with the frame rate.
                    let mut av1_failed = false;
                    let mut av1_ms = 0.0_f64;
                    let mut av1_frames = 0u32;
                    // Pace the capture to its frame rate. A device already delivers at that rate; a
                    // test picture does not, so this keeps both at the same clock.
                    let period = Duration::from_millis(1_000 / u64::from(format.fps));
                    let mut due = std::time::Instant::now();
                    let mut applied = START_KBPS;
                    while !thread_stop.load(Ordering::Relaxed) {
                        let wanted = thread_bitrate.load(Ordering::Relaxed);
                        if wanted != applied && encoder.set_bitrate(wanted).is_ok() {
                            applied = wanted;
                        }
                        if thread_key.swap(false, Ordering::Relaxed) {
                            encoder.request_keyframe();
                        }
                        let Ok(frame) = source.next_frame() else {
                            break;
                        };
                        due += period;
                        let now = std::time::Instant::now();
                        if due > now {
                            std::thread::sleep(due - now);
                        } else {
                            due = now;
                        }
                        captured = captured.wrapping_add(1);
                        if captured.is_multiple_of(PREVIEW_EVERY) {
                            let (width, height, rgb) = frame.to_rgb_preview(PREVIEW_WIDTH);
                            if let Some(jpeg) = encode_jpeg(width, height, &rgb) {
                                // The window may not be reading; a preview it misses is not needed.
                                let _ = preview_tx.try_send(Preview {
                                    sender: 0,
                                    screen,
                                    av1: false,
                                    width,
                                    height,
                                    jpeg,
                                });
                            }
                        }
                        let wanted_av1 = thread_av1.load(Ordering::Relaxed) && !av1_failed;
                        if wanted_av1 != (current == Codec::Av1) {
                            let next = if wanted_av1 { Codec::Av1 } else { Codec::H264 };
                            match encode::open(next, config) {
                                Ok(fresh) => {
                                    encoder = fresh;
                                    current = next;
                                    encoder.request_keyframe();
                                    av1_ms = 0.0;
                                    av1_frames = 0;
                                }
                                Err(_) => av1_failed = true,
                            }
                        }
                        let started = std::time::Instant::now();
                        let Ok(chunks) = encoder.encode(&frame) else {
                            break;
                        };
                        if current == Codec::Av1 {
                            // An exponential average of encode time. AV1 that takes most of a frame
                            // interval cannot keep up with the capture, so it is dropped.
                            let ms = started.elapsed().as_secs_f64() * 1_000.0;
                            av1_ms = if av1_frames == 0 { ms } else { av1_ms * 0.9 + ms * 0.1 };
                            av1_frames += 1;
                            let budget_ms = period.as_secs_f64() * 1_000.0 * 0.8;
                            if av1_frames >= 30 && av1_ms > budget_ms {
                                av1_failed = true;
                            }
                        }
                        let codec = if current == Codec::Av1 { VideoCodec::Av1 } else { VideoCodec::H264 };
                        for chunk in chunks {
                            let encoded = Encoded {
                                frame_id,
                                keyframe: chunk.keyframe,
                                codec,
                                data: chunk.data,
                            };
                            frame_id = frame_id.wrapping_add(1);
                            match tx.try_send(encoded) {
                                Ok(()) => {}
                                // The call loop is behind. Drop the frame and send a keyframe next,
                                // so the receivers resynchronise.
                                Err(TrySendError::Full(_)) => encoder.request_keyframe(),
                                Err(TrySendError::Disconnected(_)) => return,
                            }
                        }
                    }
                })
                .map_err(|e| CoreError::Invalid(format!("capture thread: {e}")))?;
            ready_rx
                .recv()
                .map_err(|_| CoreError::Invalid("capture thread ended before it started".into()))??;
            Ok(Self {
                kind,
                stop,
                key_request,
                bitrate,
                av1,
                rx,
                previews,
                thread: Some(thread),
            })
        }

        /// A camera on device `index`.
        pub(crate) fn camera(index: u32) -> Result<Self> {
            Self::start(CaptureKind::Camera, move || {
                let source = dissent_video::camera::CameraSource::open(index, CAMERA_FORMAT)
                    .map_err(capture_err)?;
                Ok(Box::new(source) as Box<dyn VideoSource>)
            })
        }

        /// A deterministic test picture instead of a camera. For tests only.
        pub(crate) fn test_pattern() -> Result<Self> {
            Self::start(CaptureKind::Camera, || {
                let source = dissent_video::pattern::PatternSource::new(CAMERA_FORMAT, 7);
                Ok(Box::new(source) as Box<dyn VideoSource>)
            })
        }

        /// A display or a window, shared as a screen. `None` is the primary display.
        pub(crate) fn screen(target: Option<super::ShareTarget>) -> Result<Self> {
            Self::start(CaptureKind::Screen, move || {
                let target = match target {
                    Some(super::ShareTarget::Window { id }) => ScreenTarget::Window(id),
                    Some(super::ShareTarget::Display { id }) => ScreenTarget::Display(id),
                    None => {
                        let displays = dissent_video::screen::list_displays().map_err(capture_err)?;
                        let display = displays
                            .iter()
                            .find(|d| d.primary)
                            .or_else(|| displays.first())
                            .ok_or_else(|| CoreError::Invalid("no display to share".into()))?;
                        ScreenTarget::Display(display.id)
                    }
                };
                let source = ScreenSource::open(target, SCREEN_FORMAT, Cursor::Excluded)
                    .map_err(capture_err)?;
                Ok(Box::new(source) as Box<dyn VideoSource>)
            })
        }

        pub(crate) fn kind(&self) -> CaptureKind {
            self.kind
        }

        /// The next encoded frame, if one is ready. Never blocks.
        pub(crate) fn try_next(&self) -> Option<Encoded> {
            self.rx.try_recv().ok()
        }

        /// The latest preview of this capture's own picture, if one is ready. Never blocks.
        pub(crate) fn try_preview(&self) -> Option<Preview> {
            self.previews.try_recv().ok()
        }

        /// Makes the next frame a keyframe, because a peer asked for one.
        pub(crate) fn request_keyframe(&self) {
            self.key_request.store(true, Ordering::Relaxed);
        }

        /// Asks for AV1 when `on`, and H.264 otherwise. The capture switches on its next frame, and
        /// stays on H.264 when AV1 is not built in or cannot keep up.
        pub(crate) fn set_av1(&self, on: bool) {
            self.av1.store(on, Ordering::Relaxed);
        }

        /// Sets the encoder's target rate, in kbps. The thread applies it on its next frame.
        pub(crate) fn set_bitrate(&self, kbps: u32) {
            self.bitrate.store(kbps.max(1), Ordering::Relaxed);
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            // The thread checks this flag before each frame, so it stops within one frame. It is
            // detached rather than joined, so dropping a capture never waits on the camera.
            self.stop.store(true, Ordering::Relaxed);
            drop(self.thread.take());
        }
    }

    fn capture_err(e: impl std::fmt::Display) -> CoreError {
        CoreError::Invalid(format!("capture: {e}"))
    }

}

/// The video budget of one capture: the bitrate it may use, given the link's estimate. It wraps
/// the adapter, so the audio reservation applies as it does everywhere else.
pub(crate) struct Budget {
    adapter: dissent_video::adapt::VideoAdapter,
}

impl Budget {
    /// A budget for a camera or a screen, on the H.264 rungs of its ladder.
    pub(crate) fn new(kind: capture::CaptureKind) -> Self {
        use dissent_video::adapt::Content;
        let content = match kind {
            capture::CaptureKind::Camera => Content::Camera,
            capture::CaptureKind::Screen => Content::Screen,
        };
        Self {
            adapter: dissent_video::adapt::VideoAdapter::with_codecs(content, &[Codec::H264])
                .expect("every content ladder has an H.264 rung"),
        }
    }

    /// Updates the budget from the link's available bandwidth and the audio target, both in
    /// kbps. Returns the bitrate the capture should use, or `None` while the budget is too small
    /// for video at all, which pauses the video.
    pub(crate) fn update(&mut self, available_kbps: u32, audio_target_kbps: u32) -> Option<u32> {
        let plan = self.adapter.update(available_kbps, audio_target_kbps);
        plan.codec.map(|_| plan.target_kbps)
    }
}

/// The decoders of the peers' cameras and screens, one per sender and kind.
#[derive(Default)]
pub(crate) struct RemoteVideo {
    decoders: HashMap<(u64, bool, bool), Box<dyn dissent_video::VideoDecoder>>,
}

impl RemoteVideo {
    /// Drops the decoders of a peer who left, so their pictures stop and memory is freed.
    pub(crate) fn forget(&mut self, sender: u64) {
        self.decoders.retain(|(peer, _, _), _| *peer != sender);
    }
}

/// A decoded picture of one peer's video, ready for the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preview {
    pub sender: u64,
    /// Whether the picture is a shared screen rather than a camera.
    pub screen: bool,
    /// Whether the stream it came from is AV1.
    pub av1: bool,
    pub width: u32,
    pub height: u32,
    /// A JPEG of the picture at the preview size.
    pub jpeg: Vec<u8>,
}

impl RemoteVideo {
    /// Decodes one received frame and returns its preview, or `None` when the frame does not
    /// decode yet, such as a delta before its keyframe.
    pub(crate) fn show(&mut self, frame: &VideoFrame) -> Option<Preview> {
        let codec = match frame.codec {
            VideoCodec::H264 => dissent_video::Codec::H264,
            VideoCodec::Av1 => dissent_video::Codec::Av1,
        };
        let av1 = frame.codec == VideoCodec::Av1;
        let decoder = match self.decoders.entry((frame.sender, frame.screen, av1)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(dissent_video::decode::open(codec).ok()?),
        };
        let pts_us = u64::from(frame.frame_id) * 1_000_000 / PREVIEW_FPS;
        let pictures = decoder.decode(&frame.payload, pts_us).ok()?;
        let picture = pictures.last()?;
        let (width, height, rgb) = picture.to_rgb_preview(PREVIEW_WIDTH);
        let jpeg = encode_jpeg(width, height, &rgb)?;
        Some(Preview {
            sender: frame.sender,
            screen: frame.screen,
            av1: frame.codec == VideoCodec::Av1,
            width,
            height,
            jpeg,
        })
    }
}

/// Encodes packed RGB as a JPEG at the preview quality.
pub(crate) fn encode_jpeg(width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, PREVIEW_QUALITY)
        .encode(rgb, width, height, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lower_estimate_lowers_the_video_bitrate_and_keeps_the_audio_reservation() {
        let mut budget = Budget::new(capture::CaptureKind::Camera);
        // Let the budget settle on a fat link, then drop to a thin one.
        let mut high = None;
        for _ in 0..40 {
            high = budget.update(1_000, 32);
        }
        let mut low = None;
        for _ in 0..40 {
            low = budget.update(300, 32);
        }
        let (high, low) = (high.expect("video on a fat link"), low.expect("video on a thin link"));
        assert!(low < high, "a lower estimate must lower the video rate: {low} vs {high}");
        // The audio reservation is 32 * 5 / 4 + 16 = 56 kbps, so video never plans past what is left.
        assert!(high <= 1_000 - 56, "video stays inside the estimate minus audio: {high}");
        assert!(low <= 300 - 56, "video stays inside the estimate minus audio: {low}");
    }

    #[test]
    fn a_budget_too_small_for_video_pauses_it() {
        let mut budget = Budget::new(capture::CaptureKind::Camera);
        let mut last = Some(0);
        for _ in 0..40 {
            last = budget.update(60, 32);
        }
        assert_eq!(last, None);
    }

    #[test]
    fn a_preview_is_a_small_jpeg() {
        let rgb = vec![120u8; 320 * 180 * 3];
        let jpeg = encode_jpeg(320, 180, &rgb).expect("encodes");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG starts with SOI");
        assert!(jpeg.len() < 40_000, "a flat preview is tiny: {} bytes", jpeg.len());
    }
}
