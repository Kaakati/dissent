//! Video for calls on this device: the camera and the screen it sends, and the previews of the
//! video it receives.
//!
//! Capture blocks, so each capture runs on its own thread. The thread captures, encodes with
//! H.264, and hands each encoded frame to the call loop, which sends it through the mesh or the
//! forwarder. The bitrate follows the link's bandwidth estimate, minus the audio reservation, so
//! video never takes the audio's share. Received frames are decoded per sender and sent to the
//! window as small JPEG previews.

use std::collections::{hash_map::Entry, HashMap};
use std::time::{Duration, Instant};

use dissent_media::{VideoCodec, VideoFrame};

use crate::error::{CoreError, Result};

use dissent_video::Codec;
use dissent_video::adapt::{Content, CpuRecovery, VideoAdapter};

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
pub(crate) const PREVIEW_WIDTH: u32 = 320;
/// The narrowest preview a tile gets, however small it is drawn.
const PREVIEW_MIN_WIDTH: u32 = 160;
/// The fastest a tile's preview is made. Every frame is still decoded, so the picture stays current.
pub(crate) const PREVIEW_INTERVAL: Duration = Duration::from_millis(1_000 / 15);
/// The fastest the largest tile's preview is made, so the tile that matters most moves smoothly.
pub(crate) const LARGE_PREVIEW_INTERVAL: Duration = Duration::from_millis(1_000 / 30);
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
    /// The simulcast layer the frame belongs to: 0 is the smallest. A capture that sends one layer sends 0.
    pub layer: u8,
    pub data: Vec<u8>,
}

pub(crate) use capture::{Capture, CaptureKind};

/// Where a camera or screen capture takes its pictures from. An engine uses a device unless a test sets it otherwise. A
/// test engine uses a pattern, or no capture at all, so no test opens a real camera or display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestCapture {
    /// A real camera or display.
    #[default]
    Device,
    /// A deterministic test picture, in place of the camera or the screen.
    Pattern,
    /// No camera or screen. Turning one on is an error.
    Unavailable,
}

/// Starts the camera for an engine, from `source`.
pub(crate) fn start_camera(source: TestCapture) -> Result<Capture> {
    match source {
        TestCapture::Device => Capture::camera(0),
        fake => Capture::fake(CaptureKind::Camera, fake),
    }
}

/// What the capture sends next: the bitrate, the codec, and the size and rate of the frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub kbps: u32,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// One thing this device sends: a capture, the budget that plans it, and whether the budget has
/// paused it.
pub(crate) struct Sending {
    pub capture: Capture,
    pub budget: Budget,
    pub paused: bool,
    /// The simulcast layers the forwarder wants, as a bit mask (bit 0 is the 180p base). `None` is the mesh mode: one
    /// adaptive stream, tagged layer 0. A forwarded call only changes this when the forwarder asks for layers.
    wanted: Option<u8>,
}

/// One simulcast layer a sender can send: its picture size and the rate it costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayerInfo {
    pub width: u32,
    pub height: u32,
    pub kbps: u32,
}

/// The layers below the top rung, as (height, kbps): 180p and 360p. Each is sent only while it is shorter than the top
/// rung, at the top rung's aspect.
const LOWER_LAYERS: [(u32, u32); 2] = [(180, 150), (360, 400)];

/// The simulcast layers of a capture whose top rung is `top`, smallest first. The top rung is the last, so a capture on
/// the 360p rung sends two layers and one on the bottom rung sends one. Lower layers are always H.264: a second AV1
/// encoder per layer costs too much CPU (`docs/design/13-forwarded-calls.md`).
pub(crate) fn simulcast_layers(top: Plan) -> Vec<LayerInfo> {
    let mut out: Vec<LayerInfo> = LOWER_LAYERS
        .iter()
        .filter(|(height, _)| *height < top.height)
        .map(|(height, kbps)| LayerInfo {
            // Even, as the encoders need.
            width: (top.width * height / top.height.max(1)) & !1,
            height: *height,
            kbps: (*kbps).min(top.kbps),
        })
        .collect();
    out.push(LayerInfo { width: top.width, height: top.height, kbps: top.kbps });
    out
}

impl Sending {
    pub(crate) fn new(capture: Capture) -> Self {
        let budget = Budget::new(capture.kind());
        Self {
            capture,
            budget,
            paused: false,
            wanted: None,
        }
    }

    /// Sets the simulcast layers to send: `None` for the mesh mode, `Some(mask)` for the layers in the mask, indexed as
    /// [`Sending::layers`] lists them. The base layer is always sent with layers, whatever the mask: a released viewer, a
    /// cascade child and a forwarder taken over on failover all get layer 0, so it must exist.
    pub(crate) fn set_wanted_layers(&mut self, mask: Option<u8>) {
        self.wanted = mask;
        self.capture.set_layers(mask.map(|m| m | 1));
    }

    /// The layers the forwarder wants, or `None` in the mesh mode.
    pub(crate) fn wanted_layers(&self) -> Option<u8> {
        self.wanted
    }

    /// The layers this sender can send, smallest first: 180p and 360p below the top rung the capture sends now.
    pub(crate) fn layers(&self) -> Vec<LayerInfo> {
        simulcast_layers(self.capture.applied_plan())
    }

    /// What the lower layers being sent cost, in kbps. The top rung is planned in what is left.
    fn lower_layers_kbps(&self) -> u32 {
        let Some(mask) = self.wanted.map(|m| m | 1) else {
            return 0;
        };
        let layers = self.layers();
        layers[..layers.len() - 1]
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, layer)| layer.kbps)
            .sum()
    }

    /// Takes the link's estimate and the audio target and plans the capture to fit: the budget's
    /// plan goes to the capture, which switches to it. A budget too small for video pauses it.
    /// Without an estimate yet, nothing changes.
    pub(crate) fn fit(&mut self, available_kbps: Option<u32>, audio_kbps: u32) {
        if let Some(share) = self.capture.take_cpu_share() {
            self.budget.observe_cpu(Instant::now(), share);
        }
        let Some(available) = available_kbps else {
            return;
        };
        // The lower layers come out of the same uplink as the top rung.
        let available = available.saturating_sub(self.lower_layers_kbps());
        match self.budget.update(available, audio_kbps) {
            Some(plan) => {
                self.paused = false;
                self.capture.set_plan(plan);
            }
            None => self.paused = true,
        }
    }

    /// Whether every peer can decode AV1. The budget plans AV1 only when they all can.
    pub(crate) fn set_av1(&mut self, on: bool) {
        self.budget.set_av1(on);
    }
}

mod capture {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, TrySendError};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use dissent_video::encode::{self, EncoderConfig, VideoEncoder};
    use dissent_video::screen::{Cursor, ScreenSource, ScreenTarget};
    use dissent_video::{CaptureFormat, Codec, Frame, VideoSource};

    use dissent_media::{VideoCodec, MAX_LAYERS};

    use super::{encode_jpeg, simulcast_layers, Encoded, LayerInfo, Plan, Preview, PREVIEW_WIDTH};
    use crate::error::{CoreError, Result};

    /// Keyframes every this many seconds, so a late joiner or a lost frame recovers quickly.
    const KEYFRAME_SECONDS: u32 = 2;
    /// Frames the capture thread may queue for the call loop, per layer, before it drops one and
    /// asks for a keyframe.
    const QUEUE: usize = 4;
    /// Every this many captured frames, the thread also makes a preview of its own picture for the
    /// window's self tile.
    const PREVIEW_EVERY: u32 = 3;
    /// The least time between two changes of size, rate or codec. Reopening a camera is slow and
    /// visible, so a plan that changes faster than this waits for the next interval.
    const RESHAPE_INTERVAL: Duration = Duration::from_secs(5);
    /// Frames whose encode time is averaged into one report of the encoder's speed.
    const SLOW_FRAMES: u32 = 30;
    /// Start rate before the first bandwidth estimate, in kbps. Inside the H.264 rung's range.
    const START_KBPS: u32 = 300;

    /// The plan a camera starts on: H.264 at 640x360, 15 fps, the bottom of its ladder.
    const CAMERA_START: Plan = Plan {
        kbps: START_KBPS,
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 15,
    };
    /// The plan a screen starts on: H.264 at 640x360, 10 fps, the bottom of its ladder.
    const SCREEN_START: Plan = Plan {
        kbps: START_KBPS,
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 10,
    };

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
        /// What the capture should send next. The thread reads it before each frame.
        plan: Arc<Mutex<Plan>>,
        /// What the capture sends now: `plan` once a reshape has applied it.
        applied: Arc<Mutex<Plan>>,
        /// The simulcast layers to send, as a mask over [`simulcast_layers`] of the applied plan.
        /// `None` is one stream at the plan, tagged layer 0.
        layers: Arc<Mutex<Option<u8>>>,
        /// The encoder's latest share of the frame interval, for the call loop to judge.
        cpu: Arc<Mutex<Option<f64>>>,
        rx: Receiver<Encoded>,
        previews: Receiver<Preview>,
        thread: Option<JoinHandle<()>>,
    }

    impl Capture {
        /// Starts a capture on `initial`. `open` makes a source at a size and rate. It runs on the
        /// capture thread, because a camera may only be used from the thread that opened it, and it
        /// runs again each time the size or rate changes. An error from the first open comes back here.
        pub(crate) fn start(
            kind: CaptureKind,
            initial: Plan,
            mut open: impl FnMut(CaptureFormat) -> Result<Box<dyn VideoSource>> + Send + 'static,
        ) -> Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let key_request = Arc::new(AtomicBool::new(false));
            let plan = Arc::new(Mutex::new(initial));
            let applied_plan = Arc::new(Mutex::new(initial));
            let layers = Arc::new(Mutex::new(None));
            let cpu = Arc::new(Mutex::new(None));
            let (tx, rx) = mpsc::sync_channel::<Encoded>(QUEUE * usize::from(MAX_LAYERS));
            let (preview_tx, previews) = mpsc::sync_channel::<Preview>(1);
            let screen = kind == CaptureKind::Screen;
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
            let thread_stop = stop.clone();
            let thread_key = key_request.clone();
            let thread_plan = plan.clone();
            let thread_applied = applied_plan.clone();
            let thread_layers = layers.clone();
            let thread_cpu = cpu.clone();
            let thread = std::thread::Builder::new()
                .name(format!("dissent-{kind:?}").to_lowercase())
                .spawn(move || {
                    let mut applied = initial;
                    let mut source = match open(format_of(applied)) {
                        Ok(source) => Some(source),
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let mut encoder = match encode::open(applied.codec, encoder_config(applied)) {
                        Ok(encoder) => encoder,
                        Err(e) => {
                            let _ = ready_tx.send(Err(capture_err(e)));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    // Each layer numbers its frames on its own. One stream is layer 0.
                    let mut frame_ids = [0u32; MAX_LAYERS as usize];
                    // The encoders of the layers below the top rung, by layer. `None` while not sent.
                    let mut lower: Vec<Option<Lower>> = Vec::new();
                    // The layer the main encoder sent last, or `None` if it was skipped. A change
                    // starts the new layer at a keyframe.
                    let mut main_layer: Option<u8> = Some(0);
                    let mut captured = 0u32;
                    let mut period = frame_period(applied.fps);
                    let mut due = Instant::now();
                    let mut last_reshape = Instant::now();
                    // The encoder's speed over the frames since the last verdict or reshape.
                    let mut ewma_ms: Option<f64> = None;
                    let mut timed = 0u32;
                    while !thread_stop.load(Ordering::Relaxed) {
                        let wanted = *lock(&thread_plan);
                        // A bitrate change goes to the open encoder at once.
                        if wanted.kbps != applied.kbps && encoder.set_bitrate(wanted.kbps).is_ok() {
                            applied.kbps = wanted.kbps;
                            *lock(&thread_applied) = applied;
                        }
                        // A size or rate change reopens the camera, so it waits for the interval. A codec change
                        // only reopens the encoder, which is quick, so it applies at once.
                        let may_reshape = !resized(wanted, applied) || last_reshape.elapsed() >= RESHAPE_INTERVAL;
                        if reshaped(wanted, applied) && may_reshape {
                            if resized(wanted, applied) {
                                last_reshape = Instant::now();
                            }
                            if let Ok(fresh) = encode::open(wanted.codec, encoder_config(wanted)) {
                                let moved = if resized(wanted, applied) {
                                    // A camera may be held by one open handle, so the old source goes first.
                                    drop(source.take());
                                    match open(format_of(wanted)) {
                                        Ok(next) => {
                                            source = Some(next);
                                            true
                                        }
                                        // Back to the old size, so the old encoder still matches.
                                        Err(_) => {
                                            source = open(format_of(applied)).ok();
                                            false
                                        }
                                    }
                                } else {
                                    true
                                };
                                if moved {
                                    encoder = fresh;
                                    encoder.request_keyframe();
                                    applied = wanted;
                                    *lock(&thread_applied) = applied;
                                    period = frame_period(applied.fps);
                                    due = Instant::now();
                                    ewma_ms = None;
                                    timed = 0;
                                }
                            }
                        }
                        // With no source left, the capture cannot continue.
                        let Some(source) = source.as_mut() else {
                            break;
                        };
                        let Ok(frame) = source.next_frame() else {
                            break;
                        };
                        due += period;
                        let now = Instant::now();
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
                        // With layers, the main encoder sends the top one and the lower ones encode
                        // scaled copies. Without, the main encoder is the one stream, layer 0.
                        let mask = *lock(&thread_layers);
                        let table = mask.map(|_| simulcast_layers(applied)).unwrap_or_default();
                        let top = u8::try_from(table.len().saturating_sub(1)).unwrap_or(0);
                        let send_top = mask.is_none_or(|m| m & (1 << top) != 0);
                        let started = Instant::now();
                        let mut out: Vec<(u8, Vec<encode::EncodedChunk>)> = Vec::new();
                        if send_top {
                            if main_layer != Some(top) {
                                encoder.request_keyframe();
                                main_layer = Some(top);
                            }
                            let Ok(chunks) = encoder.encode(&frame) else {
                                break;
                            };
                            out.push((top, chunks));
                        } else {
                            main_layer = None;
                        }
                        lower.resize_with(usize::from(top), || None);
                        for (index, slot) in lower.iter_mut().enumerate() {
                            let wanted = mask.is_some_and(|m| m & (1 << index) != 0);
                            if !wanted {
                                *slot = None;
                                continue;
                            }
                            let chunks = Lower::refresh(slot, table[index], applied.fps)
                                .and_then(|lower| lower.encode(&frame));
                            match chunks {
                                Some(chunks) => out.push((index as u8, chunks)),
                                // A layer that fails is dropped, and tried again on the next frame.
                                None => *slot = None,
                            }
                        }
                        // Encode time in an exponential moving average, reported as a share of the frame
                        // interval. The call loop steps down or climbs on that share.
                        let ms = started.elapsed().as_secs_f64() * 1_000.0;
                        ewma_ms = Some(match ewma_ms {
                            None => ms,
                            Some(prev) => prev * 0.9 + ms * 0.1,
                        });
                        timed += 1;
                        if timed >= SLOW_FRAMES {
                            let share = ewma_ms.unwrap_or(0.0) / (period.as_secs_f64() * 1_000.0);
                            *lock(&thread_cpu) = Some(share);
                            ewma_ms = None;
                            timed = 0;
                        }
                        let mut resync = false;
                        for (layer, chunks) in out {
                            // The lower layers are H.264 whatever the top rung is.
                            let codec = if layer == top { wire_codec(applied.codec) } else { VideoCodec::H264 };
                            for chunk in chunks {
                                let frame_id = &mut frame_ids[usize::from(layer)];
                                let encoded = Encoded {
                                    frame_id: *frame_id,
                                    keyframe: chunk.keyframe,
                                    codec,
                                    layer,
                                    data: chunk.data,
                                };
                                *frame_id = frame_id.wrapping_add(1);
                                match tx.try_send(encoded) {
                                    Ok(()) => {}
                                    // The call loop is behind. Drop the frame and send keyframes
                                    // next, so the receivers resynchronise.
                                    Err(TrySendError::Full(_)) => resync = true,
                                    Err(TrySendError::Disconnected(_)) => return,
                                }
                            }
                        }
                        if thread_key.swap(false, Ordering::Relaxed) || resync {
                            encoder.request_keyframe();
                            for lower in lower.iter_mut().flatten() {
                                lower.encoder.request_keyframe();
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
                plan,
                applied: applied_plan,
                layers,
                cpu,
                rx,
                previews,
                thread: Some(thread),
            })
        }

        /// A camera on device `index`.
        pub(crate) fn camera(index: u32) -> Result<Self> {
            Self::start(CaptureKind::Camera, CAMERA_START, move |format| {
                let source = dissent_video::camera::CameraSource::open(index, format).map_err(capture_err)?;
                Ok(Box::new(source) as Box<dyn VideoSource>)
            })
        }

        /// A deterministic test picture instead of a camera. For tests only.
        /// A capture that stands in for a device: the pattern, or an error for an unavailable device.
        pub(crate) fn fake(kind: CaptureKind, source: super::TestCapture) -> Result<Self> {
            match source {
                super::TestCapture::Pattern => Self::test_pattern(kind),
                _ => Err(CoreError::Invalid("no capture device: a test made it unavailable".into())),
            }
        }

        /// The deterministic test picture, as a camera or as a screen.
        pub(crate) fn test_pattern(kind: CaptureKind) -> Result<Self> {
            let initial = match kind {
                CaptureKind::Camera => CAMERA_START,
                CaptureKind::Screen => SCREEN_START,
            };
            Self::start(kind, initial, |format| {
                let source = dissent_video::pattern::PatternSource::new(format, 7);
                Ok(Box::new(source) as Box<dyn VideoSource>)
            })
        }

        /// A display or a window, shared as a screen. `None` is the primary display.
        pub(crate) fn screen(target: Option<super::ShareTarget>, source: super::TestCapture) -> Result<Self> {
            if source != super::TestCapture::Device {
                return Self::fake(CaptureKind::Screen, source);
            }
            Self::start(CaptureKind::Screen, SCREEN_START, move |format| {
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
                let source = ScreenSource::open(target, format, Cursor::Excluded).map_err(capture_err)?;
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

        /// The plan the capture is sending now. A new plan shows here once the thread applies it.
        pub(crate) fn applied_plan(&self) -> Plan {
            *lock(&self.applied)
        }

        /// Sets the simulcast layers to send, from the next frame. See `Capture::layers`.
        pub(crate) fn set_layers(&self, mask: Option<u8>) {
            *lock(&self.layers) = mask;
        }

        /// The latest preview of this capture's own picture, if one is ready. Never blocks.
        pub(crate) fn try_preview(&self) -> Option<Preview> {
            self.previews.try_recv().ok()
        }

        /// Makes the next frame a keyframe, because a peer asked for one.
        pub(crate) fn request_keyframe(&self) {
            self.key_request.store(true, Ordering::Relaxed);
        }

        /// Sets what the capture sends next. A new bitrate applies at once. A new size, rate or
        /// codec applies once the reshape interval has passed since the last change.
        pub(crate) fn set_plan(&self, plan: Plan) {
            *lock(&self.plan) = Plan {
                kbps: plan.kbps.max(1),
                ..plan
            };
        }

        /// The encoder's share of the frame interval since the last call, if it has reported one.
        pub(crate) fn take_cpu_share(&self) -> Option<f64> {
            lock(&self.cpu).take()
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

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The encoder of one layer below the top rung, fed a scaled copy of each captured frame.
    struct Lower {
        encoder: Box<dyn VideoEncoder>,
        config: EncoderConfig,
    }

    impl Lower {
        /// The layer's encoder, opened or reopened when its size or rate changed, with its bitrate
        /// kept at the layer's. A new encoder starts at a keyframe. `None` if it cannot open.
        fn refresh(slot: &mut Option<Lower>, layer: LayerInfo, fps: u32) -> Option<&mut Lower> {
            let config = EncoderConfig {
                width: layer.width,
                height: layer.height,
                fps,
                bitrate_kbps: layer.kbps.max(1),
                keyframe_interval: fps * KEYFRAME_SECONDS,
            };
            let reopen = slot.as_ref().is_none_or(|l| {
                (l.config.width, l.config.height, l.config.fps) != (config.width, config.height, config.fps)
            });
            if reopen {
                let encoder = encode::open(Codec::H264, config).ok()?;
                *slot = Some(Lower { encoder, config });
            }
            let lower = slot.as_mut()?;
            if lower.config.bitrate_kbps != config.bitrate_kbps && lower.encoder.set_bitrate(config.bitrate_kbps).is_ok() {
                lower.config.bitrate_kbps = config.bitrate_kbps;
            }
            Some(lower)
        }

        fn encode(&mut self, frame: &Frame) -> Option<Vec<encode::EncodedChunk>> {
            let scaled = frame.scaled_i420(self.config.width, self.config.height).ok()?;
            self.encoder.encode(&scaled).ok()
        }
    }

    fn format_of(plan: Plan) -> CaptureFormat {
        CaptureFormat {
            width: plan.width,
            height: plan.height,
            fps: plan.fps,
        }
    }

    fn encoder_config(plan: Plan) -> EncoderConfig {
        EncoderConfig {
            width: plan.width,
            height: plan.height,
            fps: plan.fps,
            bitrate_kbps: plan.kbps,
            keyframe_interval: plan.fps * KEYFRAME_SECONDS,
        }
    }

    fn frame_period(fps: u32) -> Duration {
        Duration::from_millis(1_000 / u64::from(fps.max(1)))
    }

    fn reshaped(a: Plan, b: Plan) -> bool {
        (a.width, a.height, a.fps, a.codec) != (b.width, b.height, b.fps, b.codec)
    }

    fn resized(a: Plan, b: Plan) -> bool {
        (a.width, a.height, a.fps) != (b.width, b.height, b.fps)
    }

    fn wire_codec(codec: Codec) -> VideoCodec {
        match codec {
            Codec::Av1 => VideoCodec::Av1,
            Codec::H264 => VideoCodec::H264,
        }
    }

    fn capture_err(e: impl std::fmt::Display) -> CoreError {
        CoreError::Invalid(format!("capture: {e}"))
    }
}

/// The video budget of one capture: the plan that fits the link's estimate, after the audio
/// reservation. AV1 is in the ladder only while every peer can decode it.
pub(crate) struct Budget {
    content: Content,
    adapter: VideoAdapter,
    av1: bool,
    ceiling: Option<crate::boosts::VideoCeiling>,
    recovery: CpuRecovery,
}

impl Budget {
    /// A budget for a camera or a screen. It starts with H.264 only.
    pub(crate) fn new(kind: capture::CaptureKind) -> Self {
        let content = match kind {
            capture::CaptureKind::Camera => Content::Camera,
            capture::CaptureKind::Screen => Content::Screen,
        };
        Self {
            content,
            adapter: adapter_for(content, false),
            av1: false,
            ceiling: None,
            recovery: CpuRecovery::new(),
        }
    }

    /// Plans AV1 when `on` and this build can encode it, and H.264 otherwise.
    pub(crate) fn set_av1(&mut self, on: bool) {
        if on == self.av1 {
            return;
        }
        self.av1 = on;
        self.adapter = adapter_for(self.content, on);
        if let Some(ceiling) = self.ceiling {
            self.adapter.cap(ceiling.width, ceiling.height, ceiling.fps);
        }
    }

    /// Holds the ladder to a server's boost ceiling. See `boosts`.
    pub(crate) fn cap(&mut self, ceiling: crate::boosts::VideoCeiling) {
        self.ceiling = Some(ceiling);
        self.adapter.cap(ceiling.width, ceiling.height, ceiling.fps);
    }

    /// Takes the encoder's share of the frame interval. A slow encoder steps the plan down a rung,
    /// and a stream that stepped down climbs back after a spell of headroom. See `CpuRecovery`.
    pub(crate) fn observe_cpu(&mut self, now: Instant, share: f64) {
        let stepped_down = self.adapter.cpu_stepped_down();
        let verdict = self.recovery.observe(now, share, stepped_down);
        self.adapter.apply_cpu(verdict);
    }

    /// Updates the budget from the link's available bandwidth and the audio target, both in
    /// kbps. Returns the plan the capture should follow, or `None` while the budget is too small
    /// for video at all, which pauses the video.
    pub(crate) fn update(&mut self, available_kbps: u32, audio_target_kbps: u32) -> Option<Plan> {
        let plan = self.adapter.update(available_kbps, audio_target_kbps);
        let codec = plan.codec?;
        Some(Plan {
            kbps: plan.target_kbps,
            codec,
            width: plan.width,
            height: plan.height,
            fps: plan.fps,
        })
    }
}

/// The adapter for a camera or a screen. AV1 rungs are kept only when `av1` is set, and this
/// build can encode AV1.
fn adapter_for(content: Content, av1: bool) -> VideoAdapter {
    let codecs: &[Codec] = if av1 { Codec::ENCODABLE } else { &[Codec::H264] };
    VideoAdapter::with_codecs(content, codecs).expect("every content ladder has an H.264 rung")
}

/// The decoders of the peers' cameras and screens, one per sender and kind.
#[derive(Default)]
pub(crate) struct RemoteVideo {
    decoders: HashMap<(u64, bool, bool), Box<dyn dissent_video::VideoDecoder>>,
    /// When each peer's picture was last made into a preview, so previews stay within the rate.
    shown: HashMap<(u64, bool), Instant>,
}

impl RemoteVideo {
    /// Drops the decoders of a peer who left, so their pictures stop and memory is freed.
    pub(crate) fn forget(&mut self, sender: u64) {
        self.decoders.retain(|(peer, _, _), _| *peer != sender);
        self.shown.retain(|(peer, _), _| *peer != sender);
    }

    /// Whether any peer's camera or screen is arriving.
    pub(crate) fn receiving(&self) -> bool {
        !self.decoders.is_empty()
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
    /// Decodes one received frame and returns its preview at `max_width`, or `None` when the frame
    /// does not decode yet, such as a delta before its keyframe, or when the last preview is less
    /// than `interval` old.
    pub(crate) fn show(&mut self, frame: &VideoFrame, max_width: u32, interval: Duration) -> Option<Preview> {
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
        let now = Instant::now();
        let key = (frame.sender, frame.screen);
        if self
            .shown
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < interval)
        {
            return None;
        }
        let max_width = max_width.max(PREVIEW_MIN_WIDTH);
        let (width, height, rgb) = picture.to_rgb_preview(max_width);
        let jpeg = encode_jpeg(width, height, &rgb)?;
        self.shown.insert(key, now);
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
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use dissent_video::pattern::PatternSource;
    use dissent_video::{CaptureFormat, VideoSource};

    use super::*;

    /// Starts a capture on a test picture, which records the size and rate of every open.
    fn recording_capture(initial: Plan) -> (capture::Capture, Arc<Mutex<Vec<CaptureFormat>>>) {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let record = opened.clone();
        let capture = capture::Capture::start(capture::CaptureKind::Camera, initial, move |format| {
            record.lock().unwrap().push(format);
            Ok(Box::new(PatternSource::new(format, 7)) as Box<dyn VideoSource>)
        })
        .expect("the capture starts");
        (capture, opened)
    }

    /// Polls `done` until it holds, or until `within` has passed.
    fn wait_for(within: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    #[test]
    fn a_plan_change_reopens_the_capture_and_a_bitrate_change_does_not() {
        let start = Plan {
            kbps: 300,
            codec: Codec::H264,
            width: 640,
            height: 360,
            fps: 15,
        };
        let (capture, opened) = recording_capture(start);

        // A new bitrate goes to the open encoder, so the capture stays as it was.
        capture.set_plan(Plan { kbps: 900, ..start });
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(opened.lock().unwrap().len(), 1, "a bitrate change must not reopen the capture");

        // A new size and rate reopen it. The first change waits out the reshape interval.
        capture.set_plan(Plan {
            kbps: 900,
            codec: Codec::H264,
            width: 1280,
            height: 720,
            fps: 30,
        });
        assert!(
            wait_for(Duration::from_secs(15), || opened.lock().unwrap().len() == 2),
            "a new size and rate reopen the capture"
        );
        let format = opened.lock().unwrap()[1];
        assert_eq!((format.width, format.height, format.fps), (1280, 720, 30));
        assert!(
            wait_for(Duration::from_secs(10), || capture.try_next().is_some()),
            "frames keep coming after the reopen"
        );
    }

    #[cfg(feature = "svt-av1")]
    #[test]
    fn a_codec_change_reopens_only_the_encoder() {
        use dissent_media::VideoCodec;

        let start = Plan {
            kbps: 900,
            codec: Codec::H264,
            width: 640,
            height: 360,
            fps: 15,
        };
        let (capture, opened) = recording_capture(start);
        capture.set_plan(Plan {
            codec: Codec::Av1,
            ..start
        });
        assert!(
            wait_for(Duration::from_secs(15), || matches!(
                capture.try_next(),
                Some(frame) if frame.codec == VideoCodec::Av1
            )),
            "AV1 frames follow the codec change"
        );
        assert_eq!(opened.lock().unwrap().len(), 1, "a codec change keeps the same source");
    }

    #[test]
    fn layers_sit_below_the_top_rung_at_its_aspect() {
        let plan = |width, height, kbps| Plan { kbps, codec: Codec::H264, width, height, fps: 30 };
        let sizes = |top| simulcast_layers(top).iter().map(|l| (l.width, l.height, l.kbps)).collect::<Vec<_>>();
        assert_eq!(sizes(plan(1280, 720, 1500)), [(320, 180, 150), (640, 360, 400), (1280, 720, 1500)]);
        assert_eq!(sizes(plan(640, 360, 300)), [(320, 180, 150), (640, 360, 300)], "a 360p top has one layer below it");
        assert_eq!(sizes(plan(320, 180, 100)), [(320, 180, 100)], "the bottom rung is one layer");
        // A 16:10 screen keeps its shape, with even widths.
        assert_eq!(sizes(plan(1920, 1200, 2500))[..2], [(288, 180, 150), (576, 360, 400)]);
    }

    #[test]
    fn a_capture_asked_for_layers_sends_each_from_a_keyframe_with_its_own_frame_ids() {
        let top = Plan { kbps: 1_500, codec: Codec::H264, width: 1280, height: 720, fps: 30 };
        let (capture, _) = recording_capture(top);
        // One stream first, tagged layer 0.
        assert!(wait_for(Duration::from_secs(10), || capture.try_next().is_some_and(|f| f.layer == 0)));
        capture.set_layers(Some(0b101));
        let mut seen: std::collections::BTreeMap<u8, Vec<Encoded>> = Default::default();
        let got_both = wait_for(Duration::from_secs(20), || {
            while let Some(frame) = capture.try_next() {
                seen.entry(frame.layer).or_default().push(frame);
            }
            [0u8, 2].iter().all(|l| seen.get(l).is_some_and(|f| f.len() >= 10))
        });
        assert!(got_both, "layers 0 and 2 both arrive: {:?}", seen.keys().collect::<Vec<_>>());
        assert!(!seen.contains_key(&1), "layer 1 is not asked for, so not sent");
        // Layer 2 is the main encoder, which starts the switch at a keyframe.
        let top_frames = &seen[&2];
        assert!(top_frames[0].keyframe, "the top layer starts at a keyframe");
        assert!(top_frames.windows(2).all(|p| p[1].frame_id == p[0].frame_id.wrapping_add(1)), "layer 2 counts its own frames");
        // Layer 0 moved from the main encoder to a 180p one, which starts with a keyframe too, and
        // its frame ids count on from the single stream's.
        let base = &seen[&0];
        assert!(base.windows(2).all(|p| p[1].frame_id == p[0].frame_id.wrapping_add(1)), "layer 0 counts on");
        let small_start = base.iter().rposition(|f| f.keyframe).expect("the 180p encoder starts at a keyframe");
        let mut decoder = dissent_video::decode::open(Codec::H264).expect("a decoder");
        let pictures: Vec<_> = base[small_start..]
            .iter()
            .flat_map(|f| decoder.decode(&f.data, u64::from(f.frame_id)).unwrap_or_default())
            .collect();
        let picture = pictures.last().expect("the base layer decodes");
        assert_eq!((picture.width(), picture.height()), (320, 180), "the base layer is 180p");
    }

    #[test]
    fn a_slow_encoder_steps_the_plan_down_and_a_headroom_run_climbs_it_back() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut budget = Budget::new(capture::CaptureKind::Camera);
        let mut plan = None;
        for _ in 0..40 {
            plan = budget.update(3_000, 32);
        }
        assert_eq!(plan.expect("video").height, 720, "a fat link plans 720p on H.264");

        budget.observe_cpu(at(0), 0.95);
        let stepped = budget.update(3_000, 32).expect("video");
        assert_eq!(stepped.height, 360, "the slow encoder drops to 360p");

        for s in (5..40).step_by(5) {
            budget.observe_cpu(at(s), 0.3);
        }
        let climbed = budget.update(3_000, 32).expect("video");
        assert_eq!(climbed.height, 720, "30 s of headroom climbs back to 720p");
    }

    #[test]
    fn av1_is_planned_only_once_it_is_allowed() {
        let mut budget = Budget::new(capture::CaptureKind::Camera);
        let mut plan = None;
        for _ in 0..40 {
            plan = budget.update(3_000, 32);
        }
        assert_eq!(
            plan.expect("video on a fat link").codec,
            Codec::H264,
            "H.264 until every peer can decode AV1"
        );

        budget.set_av1(true);
        for _ in 0..40 {
            plan = budget.update(3_000, 32);
        }
        let expected = if cfg!(feature = "svt-av1") { Codec::Av1 } else { Codec::H264 };
        assert_eq!(
            plan.expect("video on a fat link").codec,
            expected,
            "AV1 is planned once allowed, and only when this build can encode it"
        );
    }

    #[test]
    fn a_lower_estimate_lowers_the_video_bitrate_and_keeps_the_audio_reservation() {
        let mut budget = Budget::new(capture::CaptureKind::Camera);
        // Let the budget settle on a fat link, then drop to a thin one.
        let mut high = None;
        for _ in 0..40 {
            high = budget.update(1_000, 32).map(|p| p.kbps);
        }
        let mut low = None;
        for _ in 0..40 {
            low = budget.update(300, 32).map(|p| p.kbps);
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
            last = budget.update(60, 32).map(|p| p.kbps);
        }
        assert_eq!(last, None);
    }

    #[test]
    #[ignore = "measurement: run in release with --ignored --nocapture"]
    fn probe_remote_tile_cpu() {
        use dissent_video::encode::{self, EncoderConfig};
        use dissent_video::pattern::PatternSource;
        use dissent_video::source::{CaptureFormat, VideoSource};
        let (w, h, fps) = (1280u32, 720u32, 30u32);
        let mut src = PatternSource::new(CaptureFormat { width: w, height: h, fps }, 7);
        let mut enc = encode::open(
            Codec::H264,
            EncoderConfig { width: w, height: h, fps, bitrate_kbps: 2_000, keyframe_interval: 60 },
        )
        .unwrap();
        let mut chunks = Vec::new();
        for _ in 0..120 {
            let frame = src.next_frame().unwrap();
            chunks.extend(enc.encode(&frame).unwrap());
        }
        for (tile, interval) in [(320u32, PREVIEW_INTERVAL), (1280, PREVIEW_INTERVAL), (1280, LARGE_PREVIEW_INTERVAL)] {
            let mut remote = RemoteVideo::default();
            let mut work = Duration::ZERO;
            let mut previews = 0;
            let started = Instant::now();
            for (i, chunk) in chunks.iter().enumerate() {
                let frame = VideoFrame {
                    sender: 1,
                    frame_id: i as u32,
                    keyframe: chunk.keyframe,
                    codec: VideoCodec::H264,
                    screen: false,
                    layer: 0,
                    payload: chunk.data.clone(),
                };
                let t = Instant::now();
                if remote.show(&frame, tile, interval).is_some() {
                    previews += 1;
                }
                work += t.elapsed();
                let due = started + Duration::from_millis(33 * (i as u64 + 1));
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
            let secs = started.elapsed().as_secs_f64();
            println!(
                "PROBE tile {tile} px every {:?}: {previews} previews in {secs:.1} s, show() work {:.1}% of one core",
                interval,
                100.0 * work.as_secs_f64() / secs
            );
        }
    }

    #[test]
    fn a_preview_is_as_wide_as_its_tile_and_no_faster_than_fifteen_a_second() {
        use dissent_video::encode::{self, EncoderConfig};
        use dissent_video::pattern::PatternSource;
        use dissent_video::source::{CaptureFormat, VideoSource};
        let (w, h, fps) = (640u32, 360u32, 30u32);
        let mut src = PatternSource::new(CaptureFormat { width: w, height: h, fps }, 7);
        let mut enc = encode::open(
            Codec::H264,
            EncoderConfig { width: w, height: h, fps, bitrate_kbps: 800, keyframe_interval: 60 },
        )
        .unwrap();
        let mut chunks = Vec::new();
        for _ in 0..3 {
            chunks.extend(enc.encode(&src.next_frame().unwrap()).unwrap());
        }
        let frame = |i: usize| VideoFrame {
            sender: 1,
            frame_id: i as u32,
            keyframe: chunks[i].keyframe,
            codec: VideoCodec::H264,
            screen: false,
            layer: 0,
            payload: chunks[i].data.clone(),
        };

        let mut small = RemoteVideo::default();
        let first = small.show(&frame(0), 160, PREVIEW_INTERVAL).expect("the keyframe shows");
        assert_eq!(first.width, 160, "a 160 px tile gets a 160 px preview");
        assert!(small.show(&frame(1), 160, PREVIEW_INTERVAL).is_none(), "a second frame inside the interval makes no preview");

        let mut large = RemoteVideo::default();
        let wide = large.show(&frame(0), 1_280, PREVIEW_INTERVAL).expect("the keyframe shows");
        assert_eq!(wide.width, w, "a tile wider than the frame gets the frame's own width");
    }

    #[test]
    fn a_preview_is_a_small_jpeg() {
        let rgb = vec![120u8; 320 * 180 * 3];
        let jpeg = encode_jpeg(320, 180, &rgb).expect("encodes");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG starts with SOI");
        assert!(jpeg.len() < 40_000, "a flat preview is tiny: {} bytes", jpeg.len());
    }
}
