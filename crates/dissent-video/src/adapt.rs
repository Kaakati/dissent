//! Bitrate and quality adaptation for one outgoing video stream.
//!
//! Audio always keeps its reserve. Video gets whatever the link has left after that:
//!
//! ```text
//! audio_reserve = audio_target * 1.25 + AUDIO_OVERHEAD_KBPS
//! budget        = max(0, available - audio_reserve)
//! ```
//!
//! The ladder steps down in the order bitrate, then frame rate, then resolution. Below the
//! lowest rung's floor, video pauses rather than eating into the audio reserve. Steps down
//! take effect after a few samples below the threshold, and steps up need a margin above it
//! for several samples, so a link that hovers near a threshold does not flap.

use std::time::{Duration, Instant};

use crate::codec::Codec;

/// Fixed overhead on top of the audio payload: packet headers, SFrame and digests.
pub const AUDIO_OVERHEAD_KBPS: u32 = 16;

/// Samples below a rung's minimum before the stream drops to a lower rung.
const DOWN_SAMPLES: u32 = 3;
/// Samples above a rung's minimum plus margin before the stream climbs to a higher rung.
const UP_SAMPLES: u32 = 5;
/// How far above a rung's minimum the budget must be before climbing to it.
const UP_MARGIN_PERCENT: u32 = 20;
/// Smallest bitrate change, as a percentage, that is worth a new encoder setting.
/// Decreases are never held back: they protect audio.
const BITRATE_STEP_PERCENT: u32 = 10;

/// The kbps the audio stream reserves before video gets any share of the link.
pub fn audio_reserve_kbps(audio_target_kbps: u32) -> u32 {
    audio_target_kbps.saturating_mul(5) / 4 + AUDIO_OVERHEAD_KBPS
}

/// What the video is for. Each content kind has its own ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// A webcam. Motion matters most, so frame rate is kept as long as possible.
    Camera,
    /// A screen or window. Text needs resolution, so frame rate drops first.
    Screen,
}

/// One step of a ladder: the output size, frame rate and the bitrate range it runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub min_kbps: u32,
    pub max_kbps: u32,
}

/// Camera steps, best first. Frame rate drops before resolution. The lowest rung is H.264 at
/// 360p, so a slow AV1 encoder can fall back to a codec that is cheaper to encode. 1080p60 is
/// AV1 only: at 1080p, H.264 took about 84% of a 60 fps frame on the measuring machine, which
/// leaves no margin, while AV1 took about 55%.
const CAMERA: [Rung; 6] = [
    Rung {
        codec: Codec::Av1,
        width: 1920,
        height: 1080,
        fps: 60,
        min_kbps: 4000,
        max_kbps: 6000,
    },
    Rung {
        codec: Codec::Av1,
        width: 1920,
        height: 1080,
        fps: 30,
        min_kbps: 2000,
        max_kbps: 4000,
    },
    Rung {
        codec: Codec::Av1,
        width: 1280,
        height: 720,
        fps: 30,
        min_kbps: 1000,
        max_kbps: 2500,
    },
    Rung {
        codec: Codec::Av1,
        width: 1280,
        height: 720,
        fps: 15,
        min_kbps: 600,
        max_kbps: 1000,
    },
    Rung {
        codec: Codec::Av1,
        width: 640,
        height: 360,
        fps: 15,
        min_kbps: 250,
        max_kbps: 600,
    },
    Rung {
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 15,
        min_kbps: 150,
        max_kbps: 400,
    },
];

/// Camera steps when AV1 is off: H.264 at 720p, then the 360p fallback. The AV1 ladder cannot
/// hold an H.264 720p rung, because the AV1 360p rung falls back to H.264 360p in cost order, and
/// H.264 720p costs more per frame than AV1 720p, so it would be a step up in cost.
const CAMERA_H264: [Rung; 2] = [
    Rung {
        codec: Codec::H264,
        width: 1280,
        height: 720,
        fps: 30,
        min_kbps: 1000,
        max_kbps: 2500,
    },
    Rung {
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 15,
        min_kbps: 150,
        max_kbps: 400,
    },
];

/// The lowest screen bitrate a person can choose, in kbps.
pub const SCREEN_MIN_KBPS: u32 = 1_500;
/// The highest screen bitrate a person can choose, in kbps.
pub const SCREEN_MAX_KBPS: u32 = 6_000;
/// The screen bitrate until a person chooses one, in kbps. It reaches 30 fps on a 1440p screen.
pub const SCREEN_DEFAULT_KBPS: u32 = 3_000;
/// The largest screen frame. OpenH264 refuses anything above 3840x2160, which is its level 5.2, and a
/// call must be able to fall back to H.264. A larger source is scaled to fit.
pub const SCREEN_MAX_SIZE: (u32, u32) = (3840, 2160);

const P1080_PIXELS: u32 = 1920 * 1080;
/// The frame rates a screen steps through at its own size, with what each needs at 1080p, in kbps.
const SCREEN_RATES: [(u32, u32); 5] = [(60, 3_000), (30, 1_600), (15, 1_200), (10, 800), (5, 500)];
/// The steps after the native size is used up: 720p, as (fps, min kbps).
const SCREEN_720: [(u32, u32); 2] = [(10, 400), (5, 250)];

/// What a screen ladder is built from: the source's size, and the bitrate, resolution and frame rate chosen
/// for the share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenSpec {
    pub source: (u32, u32),
    pub target_kbps: u32,
    /// The tallest picture chosen for the share, or `None` for the source's own size.
    pub max_height: Option<u32>,
    /// The highest frame rate chosen for the share, or `None` for no limit.
    pub max_fps: Option<u32>,
}

impl Default for ScreenSpec {
    fn default() -> Self {
        Self {
            source: (1920, 1080),
            target_kbps: SCREEN_MAX_KBPS,
            max_height: None,
            max_fps: None,
        }
    }
}

/// `width` by `height` scaled down to fit inside `max`, keeping its shape. Never scaled up. Both
/// sides come out even, as the encoders need, and at least 2.
pub fn fit_within(width: u32, height: u32, max: (u32, u32)) -> (u32, u32) {
    let (w, h) = (u64::from(width.max(2)), u64::from(height.max(2)));
    let (max_w, max_h) = (u64::from(max.0), u64::from(max.1));
    // The smaller of the two scales, kept as a fraction so the arithmetic stays exact.
    let (num, den) = if w * max_h > h * max_w { (max_w, w) } else { (max_h, h) };
    let (w, h) = if num >= den { (w, h) } else { (w * num / den, h * num / den) };
    ((w as u32 & !1).max(2), (h as u32 & !1).max(2))
}

/// The screen steps for `spec`, best first. The top steps keep the source's own size and only lower
/// the frame rate, so text stays sharp until the link cannot carry 5 fps of it. Then 720p, then H.264
/// at 360p, for a slow encoder or a thin link.
///
/// A step's minimum follows the square root of its pixels, because screen content does not cost in
/// proportion to its size. A step the chosen bitrate cannot reach is left out, and no step sends more
/// than it. 60 fps is AV1 up to 1080p only: 1440p took about 30 ms a frame with AV1 on the
/// measuring machine, over the 16.7 ms a frame has. H.264 stops at 30 fps. See "Screen share" in
/// `docs/design/04-calls.md`.
pub fn screen_ladder(spec: ScreenSpec, codecs: &[Codec]) -> Vec<Rung> {
    let max_fps = spec.max_fps.unwrap_or(u32::MAX);
    let max = (SCREEN_MAX_SIZE.0, SCREEN_MAX_SIZE.1.min(spec.max_height.unwrap_or(u32::MAX)));
    let (width, height) = fit_within(spec.source.0, spec.source.1, max);
    let target = spec.target_kbps;
    let codec = if codecs.contains(&Codec::Av1) { Codec::Av1 } else { Codec::H264 };
    let scale = (f64::from(width * height) / f64::from(P1080_PIXELS)).sqrt().max(0.4);
    let bottom_size = fit_within(width, height, (640, 360));

    let mut rungs: Vec<Rung> = SCREEN_RATES
        .iter()
        .filter(|(fps, _)| match codec {
            Codec::Av1 => *fps < 60 || width * height <= P1080_PIXELS,
            Codec::H264 => *fps <= 30,
        })
        .map(|(fps, min)| Rung {
            codec,
            width,
            height,
            fps: *fps,
            min_kbps: (f64::from(*min) * scale) as u32,
            max_kbps: target,
        })
        // A climb needs the margin above a rung's minimum, so a rung without it could never be reached.
        .filter(|rung| meets(target, rung.min_kbps, UP_MARGIN_PERCENT))
        .collect();
    let (w720, h720) = fit_within(width, height, (1280, 720));
    if (w720, h720) != (width, height) {
        rungs.extend(SCREEN_720.iter().map(|(fps, min)| Rung {
            codec,
            width: w720,
            height: h720,
            fps: *fps,
            min_kbps: *min,
            max_kbps: target,
        }));
    }
    rungs.push(Rung {
        codec: Codec::H264,
        width: bottom_size.0,
        height: bottom_size.1,
        fps: 10,
        min_kbps: 150,
        max_kbps: 250,
    });
    // Each step must need less than the one above, whatever shape the source has.
    let mut last = u32::MAX;
    rungs.retain(|rung| {
        let keep = rung.min_kbps < last && rung.fps <= max_fps.max(10) && codecs.contains(&rung.codec);
        if keep {
            last = rung.min_kbps;
        }
        keep
    });
    rungs
}

/// How much the estimate must fall between two samples to count as the link pushing back.
const CONGESTION_FALL_PERCENT: u32 = 15;
/// How long a new share believes the estimate. Its first keyframe is its largest, up to a megabyte at
/// 1440p and 6000 kbps. Made at the rate the link is known to carry it is about a third of that.
const TRUST_START: Duration = Duration::from_secs(5);
/// How long a share believes the estimate after the link pushes back. It doubles each time the link
/// pushes back again, and starts over after `TRUST_RESET` with no push back.
const TRUST_HOLD: Duration = Duration::from_secs(10);
const TRUST_HOLD_MAX: Duration = Duration::from_secs(120);
const TRUST_RESET: Duration = Duration::from_secs(300);

/// Decides how much of the link a share may plan for.
///
/// The estimator never rises far above what was acknowledged, and a screen often sends far less than
/// its bitrate, so a share of a still page would stay on the low rungs however fat the link is. So a
/// share picks its rung for its chosen rate, while its encoder stays at the estimate and raises it by
/// filling it. When the estimate falls the link has pushed back, and the estimate is believed for a while. See "Screen share" in `docs/design/04-calls.md`.
#[derive(Debug)]
pub struct ScreenLink {
    last_estimate: u32,
    believe_until: Option<Instant>,
    last_push_back: Option<Instant>,
    hold: Duration,
}

impl Default for ScreenLink {
    fn default() -> Self {
        Self {
            last_estimate: 0,
            believe_until: None,
            last_push_back: None,
            hold: TRUST_HOLD,
        }
    }
}

impl ScreenLink {
    /// Whether the share may send a step above the estimate now, to find out whether the link carries more. Not in its
    /// first seconds, and not while it believes the estimate after a push back, so a thin link is probed at most once
    /// per hold, and the holds grow from 10 s to two minutes.
    pub fn probing(&self, now: Instant) -> bool {
        self.last_estimate > 0 && self.believe_until.is_none_or(|until| now >= until)
    }

    /// What to plan for, in kbps, from the estimate now and what the share wants to send.
    pub fn available(&mut self, now: Instant, estimate: u32, wanted: u32) -> u32 {
        if self.last_estimate == 0 {
            self.believe_until = Some(now + TRUST_START);
        }
        if self.last_push_back.is_some_and(|at| now.duration_since(at) >= TRUST_RESET) {
            self.hold = TRUST_HOLD;
            self.last_push_back = None;
        }
        let fell = u64::from(estimate) * 100 < u64::from(self.last_estimate) * u64::from(100 - CONGESTION_FALL_PERCENT);
        self.last_estimate = estimate;
        if fell {
            // Only one push back counts per hold, as the estimate falls over several samples.
            if self.believe_until.is_none_or(|until| now >= until) {
                self.believe_until = Some(now + self.hold);
                self.hold = (self.hold * 2).min(TRUST_HOLD_MAX);
                self.last_push_back = Some(now);
            }
            return estimate;
        }
        if self.believe_until.is_some_and(|until| now < until) {
            return estimate;
        }
        estimate.max(wanted)
    }
}

/// The camera table an adapter built for `codecs` draws its rungs from. Calls without AV1 get the
/// H.264 camera ladder.
fn camera_table(codecs: &[Codec]) -> &'static [Rung] {
    if codecs.contains(&Codec::Av1) {
        &CAMERA
    } else {
        &CAMERA_H264
    }
}

/// The decision for one sample. A paused plan has no rung and no bitrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoPlan {
    /// Codec for the encoder, or `None` while paused.
    pub codec: Option<Codec>,
    /// Index into the adapter's rungs, or `None` while paused.
    pub rung: Option<usize>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Bitrate for the encoder. Zero while paused.
    pub target_kbps: u32,
}

impl VideoPlan {
    fn paused() -> Self {
        Self {
            codec: None,
            rung: None,
            width: 0,
            height: 0,
            fps: 0,
            target_kbps: 0,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.rung.is_none()
    }
}

/// Keeps one video stream within the bandwidth that audio leaves it.
pub struct VideoAdapter {
    /// The rungs this call can use, in ladder order. Only codecs both sides share are kept.
    ladder: Vec<Rung>,
    rung: Option<usize>,
    /// Lowest quality the CPU governor allows, as a ladder index. Bandwidth can still pick
    /// a lower rung, so the effective rung is the larger of the two indices.
    cpu_floor: usize,
    target_kbps: u32,
    /// What the link left for video at the last sample.
    budget: u32,
    down_streak: u32,
    up_streak: u32,
}

impl VideoAdapter {
    /// Starts paused, with rungs for every codec this build can encode.
    /// The first samples decide whether video goes out.
    pub fn new(content: Content) -> Self {
        Self::with_codecs(content, Codec::ENCODABLE).expect("every content kind has an H.264 rung")
    }

    /// Starts paused, with rungs only for `codecs`. `None` when no rung uses one of them,
    /// which means the peers share no video codec.
    pub fn with_codecs(content: Content, codecs: &[Codec]) -> Option<Self> {
        let ladder = match content {
            Content::Camera => camera_table(codecs).iter().copied().filter(|rung| codecs.contains(&rung.codec)).collect(),
            Content::Screen => screen_ladder(ScreenSpec::default(), codecs),
        };
        Self::from_rungs(ladder)
    }

    /// Starts paused, on `rungs`, best first. `None` when there are none.
    pub fn from_rungs(rungs: Vec<Rung>) -> Option<Self> {
        if rungs.is_empty() {
            return None;
        }
        Some(Self {
            ladder: rungs,
            rung: None,
            cpu_floor: 0,
            target_kbps: 0,
            budget: 0,
            down_streak: 0,
            up_streak: 0,
        })
    }

    /// Moves to new rungs without pausing, as when a shared window resizes or a person chooses another
    /// bitrate. The stream takes the best new rung the last budget clears, and a CPU floor stays at
    /// the same step, so a slow encoder does not climb because of this. An empty list is ignored.
    pub fn set_rungs(&mut self, rungs: Vec<Rung>) {
        if rungs.is_empty() {
            return;
        }
        let (top, floor) = (self.ladder[0], self.ladder[self.cpu_floor]);
        let was_stepped = self.cpu_floor > 0;
        let at_full_size = (floor.width, floor.height) == (top.width, top.height);
        self.ladder = rungs;
        let last = self.ladder.len() - 1;
        self.cpu_floor = if !was_stepped {
            0
        } else if at_full_size {
            // Encode time barely grows with a screen's size, so the same frame rate is the same step.
            self.ladder.iter().position(|rung| rung.fps <= floor.fps).unwrap_or(last)
        } else {
            self.ladder.iter().position(|rung| pixel_rate(*rung) <= pixel_rate(floor)).unwrap_or(last)
        };
        if self.rung.is_some() {
            self.rung = Some(best_rung(&self.ladder, self.budget, 0));
        }
        self.reset_streaks();
        self.retarget(self.budget, true);
    }

    /// The rungs this adapter moves between, best first.
    pub fn rungs(&self) -> &[Rung] {
        &self.ladder
    }

    /// Drops the rungs larger than `width` by `height` or faster than `fps`, as a server's boost
    /// ceiling sets them. The lowest rung always stays, so a ceiling never stops video.
    pub fn cap(&mut self, width: u32, height: u32, fps: u32) {
        let lowest = self.ladder[self.ladder.len() - 1];
        self.ladder.retain(|r| r.width <= width && r.height <= height && r.fps <= fps);
        if self.ladder.is_empty() {
            self.ladder.push(lowest);
        }
        self.rung = self.rung.map(|i| i.min(self.ladder.len() - 1));
        self.cpu_floor = self.cpu_floor.min(self.ladder.len() - 1);
    }

    /// The plan as of the last update.
    pub fn plan(&self) -> VideoPlan {
        match self.effective_rung() {
            None => VideoPlan::paused(),
            Some(index) => {
                let rung = self.ladder[index];
                VideoPlan {
                    codec: Some(rung.codec),
                    rung: Some(index),
                    width: rung.width,
                    height: rung.height,
                    fps: rung.fps,
                    target_kbps: self.target_kbps,
                }
            }
        }
    }

    /// Takes one sample of the link and returns the plan for it.
    ///
    /// `available_kbps` is what the congestion controller estimates the link can carry.
    /// `audio_target_kbps` is the audio bitrate that is always reserved.
    pub fn update(&mut self, available_kbps: u32, audio_target_kbps: u32) -> VideoPlan {
        let ladder = &self.ladder;
        let budget = available_kbps.saturating_sub(audio_reserve_kbps(audio_target_kbps));
        self.budget = budget;
        let floor = ladder[ladder.len() - 1].min_kbps;
        let before = self.rung;

        if budget < floor {
            // Protect audio at once. Video stops without waiting for a streak.
            self.rung = None;
            self.down_streak = 0;
            self.up_streak = 0;
        } else {
            match self.rung {
                None => {
                    if meets(budget, floor, UP_MARGIN_PERCENT) {
                        self.climb(budget, None);
                    } else {
                        self.up_streak = 0;
                    }
                }
                Some(current) => {
                    if budget < ladder[current].min_kbps {
                        self.up_streak = 0;
                        self.down_streak += 1;
                        if self.down_streak >= DOWN_SAMPLES {
                            self.rung = Some(best_rung(ladder, budget, 0));
                            self.reset_streaks();
                        }
                    } else {
                        self.down_streak = 0;
                        let candidate = best_rung(ladder, budget, UP_MARGIN_PERCENT);
                        if candidate < current {
                            self.climb(budget, Some(candidate));
                        } else {
                            self.up_streak = 0;
                        }
                    }
                }
            }
        }

        self.retarget(budget, before != self.rung);
        self.plan()
    }

    /// Counts a climb sample. Climbs apply after `UP_SAMPLES` in a row.
    fn climb(&mut self, budget: u32, candidate: Option<usize>) {
        self.up_streak += 1;
        if self.up_streak >= UP_SAMPLES {
            let target =
                candidate.unwrap_or_else(|| best_rung(&self.ladder, budget, UP_MARGIN_PERCENT));
            self.rung = Some(target);
            self.reset_streaks();
        }
    }

    fn reset_streaks(&mut self) {
        self.down_streak = 0;
        self.up_streak = 0;
    }

    /// The rung to run: the bandwidth choice, but never better than the CPU floor.
    fn effective_rung(&self) -> Option<usize> {
        self.rung.map(|index| index.max(self.cpu_floor))
    }

    /// Applies a CPU verdict by moving the quality floor one rung.
    /// Whether the stream is below its best rung because the encoder could not keep up.
    pub fn cpu_stepped_down(&self) -> bool {
        self.cpu_floor > 0
    }

    pub fn apply_cpu(&mut self, verdict: CpuVerdict) {
        let last = self.ladder.len() - 1;
        match verdict {
            CpuVerdict::Keep => {}
            // One rung below where the stream is now, not below the old floor. A floor left above
            // the bandwidth's rung would otherwise need several verdicts before the plan moved.
            CpuVerdict::Downgrade => {
                let current = self.effective_rung().unwrap_or(self.cpu_floor);
                self.cpu_floor = (current + 1).min(last);
            }
            CpuVerdict::Upgrade => self.cpu_floor = self.cpu_floor.saturating_sub(1),
            CpuVerdict::Recover => self.cpu_floor = 0,
        }
    }

    fn retarget(&mut self, budget: u32, rung_changed: bool) {
        let Some(index) = self.effective_rung() else {
            self.target_kbps = 0;
            return;
        };
        let rung = self.ladder[index];
        let want = budget.min(rung.max_kbps);
        let step = want.abs_diff(self.target_kbps) as u64 * 100
            >= self.target_kbps as u64 * BITRATE_STEP_PERCENT as u64;
        if want < self.target_kbps || rung_changed || step {
            self.target_kbps = want;
        }
    }
}

/// The highest-quality rung whose minimum the budget clears by `margin_percent`.
/// Falls back to the lowest rung when none clears it.
fn best_rung(ladder: &[Rung], budget: u32, margin_percent: u32) -> usize {
    ladder
        .iter()
        .position(|rung| meets(budget, rung.min_kbps, margin_percent))
        .unwrap_or(ladder.len() - 1)
}

fn meets(budget: u32, min_kbps: u32, margin_percent: u32) -> bool {
    budget as u64 * 100 >= min_kbps as u64 * (100 + margin_percent) as u64
}

/// Encode time, as a share of one frame period, above which the encoder counts as falling behind.
const CPU_SLOW_PERCENT: f64 = 80.0;
/// Share of one frame period that the next rung up must fit under before it is tried.
const CPU_FAST_PERCENT: f64 = 50.0;
/// Consecutive slow samples before a downgrade. A single spike does not count.
const CPU_SLOW_SAMPLES: u32 = 10;
/// Consecutive samples with headroom before an upgrade.
const CPU_FAST_SAMPLES: u32 = 60;
/// Weight of each new encode-time sample in the running average.
const CPU_SMOOTHING: f64 = 0.1;

/// What the CPU governor asks the adapter to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuVerdict {
    Keep,
    /// The encoder cannot keep up at this rung. Move to a cheaper one.
    Downgrade,
    /// The encoder has plenty of headroom. Try a better rung.
    Upgrade,
    /// The encoder has been nearly idle for a while, as after a CPU spike that ended. Go back to the best rung the
    /// bandwidth allows. A slow encoder there steps down again, one rung at a time.
    Recover,
}

/// Decides from measured encode times whether the encoder keeps up at the current rung.
///
/// A rung costs roughly in proportion to its pixels times its frame rate, so the cost of the
/// rung above is predicted from the measured cost of the current one. After a verdict the
/// measurements restart, because the encoder is rebuilt at the new rung.
#[derive(Debug, Default)]
pub struct CpuGovernor {
    ewma_ms: Option<f64>,
    slow: u32,
    fast: u32,
}

impl CpuGovernor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one frame's encode time in milliseconds, measured at ladder index `index`.
    pub fn observe(&mut self, encode_ms: f64, ladder: &[Rung], index: usize) -> CpuVerdict {
        let ewma = match self.ewma_ms {
            None => encode_ms,
            Some(prev) => prev + CPU_SMOOTHING * (encode_ms - prev),
        };
        self.ewma_ms = Some(ewma);

        let rung = ladder[index];
        let period = frame_period_ms(rung.fps);
        self.slow = if ewma > period * CPU_SLOW_PERCENT / 100.0 {
            self.slow + 1
        } else {
            0
        };
        if self.slow >= CPU_SLOW_SAMPLES && index + 1 < ladder.len() {
            *self = Self::default();
            return CpuVerdict::Downgrade;
        }

        if index > 0 {
            let up = ladder[index - 1];
            let predicted = ewma * pixel_rate(up) / pixel_rate(rung);
            let headroom = frame_period_ms(up.fps) * CPU_FAST_PERCENT / 100.0;
            self.fast = if predicted < headroom {
                self.fast + 1
            } else {
                0
            };
            if self.fast >= CPU_FAST_SAMPLES {
                *self = Self::default();
                return CpuVerdict::Upgrade;
            }
        }
        CpuVerdict::Keep
    }
}

/// The encoder's share of the frame interval above which a rung is too slow.
pub const CPU_SLOW_SHARE: f64 = 0.8;
/// The share below which the encoder has headroom to climb a rung again.
pub const CPU_HEADROOM_SHARE: f64 = 0.5;
/// The headroom a climb needs first. A climb that fails doubles the wait, up to `CLIMB_WAIT_MAX`.
const CLIMB_WAIT: Duration = Duration::from_secs(30);
const CLIMB_WAIT_MAX: Duration = Duration::from_secs(300);
/// A downgrade is not repeated within this time, so one slow spell steps down one rung at a time.
const STEP_COOLDOWN: Duration = Duration::from_secs(5);
/// The share of the frame interval under which the encoder counts as nearly idle at its stepped-down rung.
pub const CPU_IDLE_SHARE: f64 = 0.25;
/// How long the encoder must be nearly idle before the stream goes back to its best rung in one step. Climbing one rung
/// per `CLIMB_WAIT` took two minutes and more after a spike, and the frame rate stayed down for all of it.
const IDLE_RECOVER: Duration = Duration::from_secs(10);

/// Decides when a stream that stepped down may climb back. It works on the encoder's share of the
/// frame interval, measured now, and on the time, so tests can drive it with fixed instants.
///
/// A climb needs `wait` of headroom in a row. A slow spell within `CLIMB_WAIT` of a climb counts the
/// climb as failed, which doubles `wait`, up to five minutes. A climb that holds for `CLIMB_WAIT`
/// resets `wait`.
#[derive(Debug)]
pub struct CpuRecovery {
    wait: Duration,
    headroom_since: Option<Instant>,
    idle_since: Option<Instant>,
    last_climb: Option<Instant>,
    last_step: Option<Instant>,
}

impl Default for CpuRecovery {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuRecovery {
    pub fn new() -> Self {
        Self {
            wait: CLIMB_WAIT,
            headroom_since: None,
            idle_since: None,
            last_climb: None,
            last_step: None,
        }
    }

    /// Takes one measurement and returns the verdict for the adapter. `stepped_down` says whether
    /// the stream is below its best rung, since a climb is only possible from there.
    pub fn observe(&mut self, now: Instant, share: f64, stepped_down: bool) -> CpuVerdict {
        if self
            .last_climb
            .is_some_and(|climbed| now.duration_since(climbed) >= CLIMB_WAIT)
        {
            self.last_climb = None;
            self.wait = CLIMB_WAIT;
        }
        if share < CPU_IDLE_SHARE && stepped_down {
            let since = *self.idle_since.get_or_insert(now);
            if now.duration_since(since) >= IDLE_RECOVER {
                // Counts as a climb, so a slow spell soon after still doubles the wait.
                self.idle_since = None;
                self.headroom_since = None;
                self.last_climb = Some(now);
                return CpuVerdict::Recover;
            }
        } else {
            self.idle_since = None;
        }
        if share > CPU_SLOW_SHARE {
            self.headroom_since = None;
            let cooling = self
                .last_step
                .is_some_and(|step| now.duration_since(step) < STEP_COOLDOWN);
            if cooling {
                return CpuVerdict::Keep;
            }
            if self.last_climb.take().is_some() {
                // The climb did not hold, so the next one waits longer.
                self.wait = (self.wait * 2).min(CLIMB_WAIT_MAX);
            }
            self.last_step = Some(now);
            return CpuVerdict::Downgrade;
        }
        if share < CPU_HEADROOM_SHARE && stepped_down {
            let since = *self.headroom_since.get_or_insert(now);
            if now.duration_since(since) >= self.wait {
                self.headroom_since = None;
                self.last_climb = Some(now);
                return CpuVerdict::Upgrade;
            }
        } else {
            self.headroom_since = None;
        }
        CpuVerdict::Keep
    }

    /// The headroom a climb needs now. Doubles after each failed climb.
    pub fn wait(&self) -> Duration {
        self.wait
    }
}

fn frame_period_ms(fps: u32) -> f64 {
    1_000.0 / fps as f64
}

fn pixel_rate(rung: Rung) -> f64 {
    (rung.width * rung.height * rung.fps) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ceiling_drops_larger_rungs_and_keeps_the_lowest() {
        let mut a = VideoAdapter::new(Content::Screen);
        a.cap(1280, 720, 30);
        assert!(a.rungs().iter().all(|r| r.height <= 720));
        assert!(!a.rungs().is_empty());
        let mut tiny = VideoAdapter::new(Content::Camera);
        tiny.cap(16, 16, 1);
        assert_eq!(tiny.rungs().len(), 1, "the lowest rung stays");
    }

    const AUDIO: u32 = 32;
    const RESERVE: u32 = 56;
    /// Camera rungs by position: 1080p60, 1080p30, then 720p30 and 720p15, then the lower ones.
    const CAM_720_30: usize = 2;
    const CAM_720_15: usize = 3;

    /// The ladder logic is the same for every codec, so these tests use all of them, AV1 included,
    /// even when this build cannot encode AV1.
    fn adapter(content: Content) -> VideoAdapter {
        VideoAdapter::with_codecs(content, &Codec::ALL).unwrap()
    }

    fn run(adapter: &mut VideoAdapter, available: u32, samples: u32) -> VideoPlan {
        let mut plan = adapter.plan();
        for _ in 0..samples {
            plan = adapter.update(available, AUDIO);
        }
        plan
    }

    #[test]
    fn audio_reserve_has_margin_and_overhead() {
        assert_eq!(audio_reserve_kbps(32), 56);
        assert_eq!(audio_reserve_kbps(0), AUDIO_OVERHEAD_KBPS);
    }

    #[test]
    fn ladders_step_down_in_order() {
        let screen = screen_ladder(ScreenSpec::default(), &Codec::ALL);
        for (content, steps) in [(Content::Camera, &CAMERA[..]), (Content::Screen, &screen[..])] {
            for pair in steps.windows(2) {
                let (hi, lo) = (pair[0], pair[1]);
                assert!(lo.min_kbps < hi.min_kbps, "{content:?}: bitrate must fall");
                assert!(lo.max_kbps <= hi.max_kbps, "{content:?}: ceiling must fall");
                assert!(
                    lo.width * lo.height <= hi.width * hi.height,
                    "{content:?}: size must not grow"
                );
                if lo.width == hi.width {
                    assert!(
                        lo.fps <= hi.fps,
                        "{content:?}: frame rate must not rise at one size"
                    );
                }
            }
            for rung in steps {
                assert!(rung.min_kbps <= rung.max_kbps);
            }
        }
    }

    #[test]
    fn screen_drops_frame_rate_before_resolution() {
        let steps = screen_ladder(ScreenSpec::default(), &Codec::ALL);
        let first_720 = steps.iter().position(|r| r.height == 720).unwrap();
        assert!(steps[..first_720].iter().all(|r| r.height == 1080));
        assert!(steps[..first_720].windows(2).all(|p| p[0].fps > p[1].fps));
    }

    #[test]
    fn budget_below_the_reserve_pauses_video() {
        let mut adapter = adapter(Content::Camera);
        let plan = run(&mut adapter, RESERVE + 100, 20);
        assert!(
            plan.is_paused(),
            "video gets nothing below the audio reserve"
        );
        assert_eq!(plan.target_kbps, 0);
    }

    #[test]
    fn audio_reserve_is_never_given_to_video() {
        let mut adapter = adapter(Content::Camera);
        let mut seed = 7u32;
        for _ in 0..2_000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let available = (seed >> 8) % 4_000;
            let plan = adapter.update(available, AUDIO);
            if !plan.is_paused() {
                assert!(
                    plan.target_kbps + audio_reserve_kbps(AUDIO) <= available,
                    "video {} + audio reserve exceeds link {}",
                    plan.target_kbps,
                    available
                );
            }
        }
    }

    #[test]
    fn stream_resumes_only_after_a_sustained_margin() {
        let mut adapter = adapter(Content::Camera);
        let floor = 150;
        let just_over = RESERVE + floor * 11 / 10;
        assert!(
            run(&mut adapter, just_over, 20).is_paused(),
            "10% over the floor is inside the margin"
        );

        let clear = RESERVE + floor * 13 / 10;
        assert!(run(&mut adapter, clear, UP_SAMPLES - 1).is_paused());
        let plan = adapter.update(clear, AUDIO);
        assert!(!plan.is_paused());
    }

    #[test]
    fn a_single_dip_does_not_drop_a_rung() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        assert_eq!(adapter.plan().rung, Some(CAM_720_30));

        adapter.update(RESERVE + 500, AUDIO);
        assert_eq!(
            adapter.plan().rung,
            Some(CAM_720_30),
            "one low sample must not change rung"
        );
        run(&mut adapter, RESERVE + 2_000, 1);
        assert_eq!(adapter.plan().rung, Some(CAM_720_30));
    }

    #[test]
    fn a_sustained_drop_moves_down_the_ladder() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        let plan = run(&mut adapter, RESERVE + 700, DOWN_SAMPLES);
        assert_eq!(plan.rung, Some(CAM_720_15));
        assert_eq!(plan.fps, 15, "camera drops frame rate before resolution");
        assert_eq!(plan.height, 720);
    }

    #[test]
    fn boundary_noise_does_not_flap() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 1_300, 10);
        let start = adapter.plan().rung;
        assert_eq!(start, Some(CAM_720_30));
        let mut changes = 0;
        let mut last = start;
        for i in 0..200 {
            let budget = if i % 2 == 0 { 990 } else { 1_010 };
            let plan = adapter.update(RESERVE + budget, AUDIO);
            if plan.rung != last {
                changes += 1;
                last = plan.rung;
            }
        }
        assert_eq!(
            changes, 0,
            "alternating around the threshold must not change rung"
        );
    }

    #[test]
    fn small_bitrate_moves_are_held() {
        let mut adapter = adapter(Content::Camera);
        let first = run(&mut adapter, RESERVE + 1_500, 10);
        assert_eq!(first.target_kbps, 1_500);
        let nudged = adapter.update(RESERVE + 1_550, AUDIO);
        assert_eq!(
            nudged.target_kbps, 1_500,
            "a 3% rise is inside the 10% step"
        );
        let raised = adapter.update(RESERVE + 1_800, AUDIO);
        assert_eq!(raised.target_kbps, 1_800);
    }

    #[test]
    fn bitrate_drops_at_once_to_protect_audio() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        let plan = adapter.update(RESERVE + 1_200, AUDIO);
        assert_eq!(
            plan.target_kbps, 1_200,
            "a fall in budget applies on the same sample"
        );
        assert!(plan.target_kbps + RESERVE <= RESERVE + 1_200);
    }

    #[test]
    fn plan_respects_the_rung_ceiling() {
        let mut adapter = adapter(Content::Camera);
        let plan = run(&mut adapter, RESERVE + 10_000, 10);
        assert_eq!(plan.target_kbps, CAMERA[0].max_kbps);
    }

    #[test]
    fn slow_encoder_triggers_a_downgrade_after_a_sustained_run() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for sample in 1..=CPU_SLOW_SAMPLES {
            let verdict = governor.observe(60.0, steps, 0);
            if sample < CPU_SLOW_SAMPLES {
                assert_eq!(
                    verdict,
                    CpuVerdict::Keep,
                    "no downgrade before the run is sustained"
                );
            } else {
                assert_eq!(verdict, CpuVerdict::Downgrade);
            }
        }
    }

    #[test]
    fn a_single_slow_frame_does_not_downgrade() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for _ in 0..200 {
            governor.observe(5.0, steps, 0);
        }
        assert_eq!(governor.observe(90.0, steps, 0), CpuVerdict::Keep);
        for _ in 0..20 {
            assert_eq!(governor.observe(5.0, steps, 0), CpuVerdict::Keep);
        }
    }

    #[test]
    fn the_lowest_rung_cannot_downgrade() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for _ in 0..100 {
            assert_eq!(
                governor.observe(500.0, steps, steps.len() - 1),
                CpuVerdict::Keep
            );
        }
    }

    #[test]
    fn a_fast_encoder_upgrades_after_sustained_headroom() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for sample in 1..=CPU_FAST_SAMPLES {
            let verdict = governor.observe(5.0, steps, CAM_720_30);
            if sample < CPU_FAST_SAMPLES {
                assert_eq!(verdict, CpuVerdict::Keep);
            } else {
                assert_eq!(verdict, CpuVerdict::Upgrade);
            }
        }
    }

    #[test]
    fn a_fast_encoder_at_the_top_rung_never_downgrades() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for _ in 0..1_000 {
            assert_eq!(governor.observe(5.0, steps, 0), CpuVerdict::Keep);
        }
    }

    #[test]
    fn cost_near_the_threshold_does_not_flap() {
        let steps = &CAMERA[..];
        let mut governor = CpuGovernor::new();
        for i in 0..500 {
            let cost = if i % 2 == 0 { 20.0 } else { 30.0 };
            assert_eq!(governor.observe(cost, steps, CAM_720_30), CpuVerdict::Keep);
        }
    }

    #[test]
    fn cpu_downgrade_lowers_the_plan_even_with_bandwidth_to_spare() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        assert_eq!(adapter.plan().rung, Some(CAM_720_30));

        adapter.apply_cpu(CpuVerdict::Downgrade);
        let plan = adapter.update(RESERVE + 2_000, AUDIO);
        assert_eq!(plan.rung, Some(CAM_720_15));
        assert_eq!(plan.fps, 15);

        adapter.apply_cpu(CpuVerdict::Upgrade);
        let plan = adapter.update(RESERVE + 2_000, AUDIO);
        assert_eq!(plan.rung, Some(CAM_720_30));
    }

    /// Each boost level's ceiling keeps the rungs at or below it. The camera ladder has 6 rungs,
    /// the screen ladder 8, and the lowest rung always stays.
    #[test]
    fn a_slow_encoder_steps_down_and_a_headroom_run_climbs_back() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut recovery = CpuRecovery::new();
        assert_eq!(recovery.observe(at(0), 0.9, false), CpuVerdict::Downgrade, "a slow encoder steps down");
        assert_eq!(recovery.observe(at(1), 0.9, true), CpuVerdict::Keep, "no second step inside the cooldown");

        for s in (6..36).step_by(5) {
            assert_eq!(recovery.observe(at(s), 0.3, true), CpuVerdict::Keep, "headroom is not 30 s yet at {s} s");
        }
        assert_eq!(recovery.observe(at(36), 0.3, true), CpuVerdict::Upgrade, "30 s of headroom climbs back");
    }

    #[test]
    fn no_climb_without_headroom_or_without_a_step_down() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // Headroom at the best rung has nothing to climb to.
        let mut recovery = CpuRecovery::new();
        for s in 0..60 {
            assert_eq!(recovery.observe(at(s), 0.3, false), CpuVerdict::Keep);
        }
        // A share between the two limits resets the headroom run.
        let mut recovery = CpuRecovery::new();
        assert_eq!(recovery.observe(at(0), 0.9, false), CpuVerdict::Downgrade);
        assert_eq!(recovery.observe(at(10), 0.3, true), CpuVerdict::Keep);
        assert_eq!(recovery.observe(at(20), 0.6, true), CpuVerdict::Keep);
        assert_eq!(recovery.observe(at(25), 0.3, true), CpuVerdict::Keep, "the run restarted at 25 s");
        assert_eq!(recovery.observe(at(54), 0.3, true), CpuVerdict::Keep);
        assert_eq!(recovery.observe(at(56), 0.3, true), CpuVerdict::Upgrade);
    }

    #[test]
    fn a_failed_climb_doubles_the_wait_up_to_five_minutes() {
        let t0 = Instant::now();
        let mut now = 0u64;
        let mut recovery = CpuRecovery::new();
        let mut waits = Vec::new();
        for _ in 0..6 {
            // Step down, wait out the headroom, climb, then fail straight away.
            assert_eq!(recovery.observe(t0 + Duration::from_secs(now), 0.9, true), CpuVerdict::Downgrade);
            now += 10;
            waits.push(recovery.wait().as_secs());
            let mut climbed = false;
            while !climbed {
                now += 1;
                climbed = recovery.observe(t0 + Duration::from_secs(now), 0.3, true) == CpuVerdict::Upgrade;
                assert!(now < 10_000, "the climb must come");
            }
            now += 5; // A slow spell 5 s after the climb: the climb failed.
        }
        assert_eq!(waits, [30, 60, 120, 240, 300, 300], "the wait doubles, then stops at five minutes");
    }

    #[test]
    fn a_climb_that_holds_resets_the_wait() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut recovery = CpuRecovery::new();
        assert_eq!(recovery.observe(at(0), 0.9, true), CpuVerdict::Downgrade);
        assert_eq!(recovery.observe(at(30), 0.3, true), CpuVerdict::Keep);
        assert_eq!(recovery.observe(at(60), 0.3, true), CpuVerdict::Upgrade);
        assert_eq!(recovery.observe(at(100), 0.3, true), CpuVerdict::Keep, "the climb held for 30 s");
        assert_eq!(recovery.wait(), Duration::from_secs(30));
        assert_eq!(recovery.observe(at(200), 0.9, true), CpuVerdict::Downgrade);
        assert_eq!(recovery.wait(), Duration::from_secs(30), "a slow spell after a held climb is not a failure");
    }

    #[test]
    fn each_boost_level_keeps_the_rungs_it_allows() {
        let cases = [
            (
                Content::Camera,
                [(1280, 720, 30, 4), (1920, 1080, 30, 5), (1920, 1080, 30, 5), (1920, 1080, 60, 6)],
            ),
            (
                Content::Screen,
                [(1920, 1080, 15, 6), (1920, 1080, 30, 7), (1920, 1080, 30, 7), (1920, 1080, 60, 8)],
            ),
        ];
        for (content, levels) in cases {
            for (width, height, fps, kept) in levels {
                let mut adapter = adapter(content);
                adapter.cap(width, height, fps);
                let rungs = adapter.rungs();
                assert_eq!(rungs.len(), kept, "{content:?} at {width}x{height} {fps} fps");
                assert!(rungs
                    .iter()
                    .all(|r| r.width <= width && r.height <= height && r.fps <= fps));
                assert_eq!(rungs.last().unwrap().codec, Codec::H264, "the lowest rung is H.264");
            }
        }
    }

    #[test]
    fn calls_without_av1_can_reach_720p_and_fall_back_to_360p() {
        let adapter = VideoAdapter::with_codecs(Content::Camera, &[Codec::H264]).unwrap();
        let sizes: Vec<_> = adapter.rungs().iter().map(|r| (r.codec, r.width, r.height, r.fps)).collect();
        assert_eq!(
            sizes,
            [(Codec::H264, 1280, 720, 30), (Codec::H264, 640, 360, 15)],
            "H.264 720p30, then the 360p step down"
        );
    }

    #[test]
    fn av1_calls_keep_the_cost_ordered_ladder() {
        let adapter = VideoAdapter::with_codecs(Content::Camera, &Codec::ALL).unwrap();
        let last = adapter.rungs().last().unwrap();
        assert_eq!((last.codec, last.width, last.fps), (Codec::H264, 640, 15), "the AV1 ladder falls back to H.264 360p");
        assert!(
            adapter.rungs().iter().all(|r| !(r.codec == Codec::H264 && r.width == 1280)),
            "no H.264 720p rung sits below an AV1 rung"
        );
    }

    #[test]
    fn a_ceiling_below_every_rung_still_keeps_the_lowest() {
        let mut adapter = adapter(Content::Camera);
        adapter.cap(0, 0, 0);
        assert_eq!(adapter.rungs().len(), 1);
        assert_eq!((adapter.rungs()[0].width, adapter.rungs()[0].fps), (640, 15));
    }

    #[test]
    fn cpu_floor_never_leaves_the_ladder() {
        let mut adapter = adapter(Content::Screen);
        for _ in 0..20 {
            adapter.apply_cpu(CpuVerdict::Downgrade);
        }
        run(&mut adapter, RESERVE + 2_000, 10);
        assert_eq!(adapter.plan().rung, Some(adapter.rungs().len() - 1));
    }

    fn screen(source: (u32, u32), target_kbps: u32) -> Vec<Rung> {
        screen_ladder(ScreenSpec { source, target_kbps, ..ScreenSpec::default() }, &Codec::ALL)
    }

    fn screen_h264(source: (u32, u32), target_kbps: u32) -> Vec<Rung> {
        screen_ladder(ScreenSpec { source, target_kbps, ..ScreenSpec::default() }, &[Codec::H264])
    }

    #[test]
    fn a_screen_streams_at_its_own_size_and_is_never_scaled_up() {
        for (source, expected) in [
            ((1920, 1080), (1920, 1080)),
            ((2560, 1440), (2560, 1440)),
            ((3440, 1440), (3440, 1440)),
            ((3840, 2160), (3840, 2160)),
            ((1280, 800), (1280, 800)),
            ((1366, 768), (1366, 768)),
            ((801, 601), (800, 600)),
            ((5120, 2880), (3840, 2160)),
        ] {
            for rungs in [screen(source, 3_000), screen_h264(source, 3_000)] {
                let top = rungs[0];
                assert_eq!((top.width, top.height), expected, "the top rung of {source:?}");
                assert!(rungs.iter().all(|r| r.width <= expected.0 && r.height <= expected.1 && r.width % 2 == 0 && r.height % 2 == 0));
            }
        }
    }

    #[test]
    fn fit_within_keeps_the_shape_and_even_sides() {
        assert_eq!(fit_within(1920, 1200, (1280, 720)), (1152, 720));
        assert_eq!(fit_within(1000, 3000, (1280, 720)), (240, 720));
        assert_eq!(fit_within(640, 360, (1920, 1080)), (640, 360));
        assert_eq!(fit_within(1, 1, (640, 360)), (2, 2));
    }

    #[test]
    fn every_source_gets_an_ordered_ladder_ending_in_h264() {
        for source in [(1920, 1080), (2560, 1440), (3840, 2160), (900, 722), (720, 1400), (640, 360), (320, 180), (120, 80), (5120, 1440)] {
            for target in [SCREEN_MIN_KBPS, SCREEN_DEFAULT_KBPS, SCREEN_MAX_KBPS] {
                let rungs = screen(source, target);
                assert_eq!(rungs.last().unwrap().codec, Codec::H264, "{source:?} at {target}");
                for pair in rungs.windows(2) {
                    assert!(pair[1].min_kbps < pair[0].min_kbps, "{source:?} at {target}: {rungs:?}");
                    assert!(pair[1].width * pair[1].height <= pair[0].width * pair[0].height);
                }
                assert!(rungs.iter().all(|r| r.max_kbps <= target && r.min_kbps <= r.max_kbps), "{source:?} at {target}: {rungs:?}");
            }
        }
    }

    #[test]
    fn the_chosen_bitrate_limits_what_the_stream_sends_and_its_frame_rate() {
        // 6000 at 1440p reaches 30 fps. 1500 keeps the size and drops to 10 fps.
        let fps = |rungs: &[Rung]| rungs[0].fps;
        assert_eq!(fps(&screen((2560, 1440), 6_000)), 30);
        assert_eq!(fps(&screen((2560, 1440), 3_000)), 30);
        assert_eq!(fps(&screen((2560, 1440), 1_500)), 10);
        assert_eq!(fps(&screen((1920, 1080), 6_000)), 60);
        assert_eq!(fps(&screen((1920, 1080), 1_500)), 15);
        assert_eq!(fps(&screen_h264((1920, 1080), 6_000)), 30, "H.264 stops at 30 fps");

        let mut adapter = VideoAdapter::from_rungs(screen((2560, 1440), 3_000)).unwrap();
        let plan = run(&mut adapter, RESERVE + 50_000, 10);
        assert_eq!((plan.width, plan.height, plan.fps, plan.target_kbps), (2560, 1440, 30, 3_000), "a fat link sends the chosen rate, no more");
    }

    #[test]
    fn a_congested_link_lowers_the_bitrate_then_the_frame_rate_and_keeps_the_size() {
        let mut adapter = VideoAdapter::from_rungs(screen((2560, 1440), 6_000)).unwrap();
        let plan = run(&mut adapter, RESERVE + 50_000, 10);
        assert_eq!((plan.height, plan.fps, plan.target_kbps), (1440, 30, 6_000));

        let plan = run(&mut adapter, RESERVE + 3_000, 10);
        assert_eq!((plan.height, plan.fps, plan.target_kbps), (1440, 30, 3_000), "the bitrate moves first");
        let plan = run(&mut adapter, RESERVE + 1_500, 10);
        assert_eq!((plan.height, plan.fps), (1440, 10), "then the frame rate, at the same size");
        let plan = run(&mut adapter, RESERVE + 900, 10);
        assert_eq!((plan.height, plan.fps), (1440, 5));
        let plan = run(&mut adapter, RESERVE + 600, 10);
        assert_eq!(plan.height, 720, "the size goes only when 5 fps does not fit");
    }

    #[test]
    fn new_rungs_keep_the_stream_running_at_the_new_size_and_rate() {
        let mut adapter = VideoAdapter::from_rungs(screen((2560, 1440), 6_000)).unwrap();
        run(&mut adapter, RESERVE + 50_000, 10);
        adapter.set_rungs(screen((1600, 900), 6_000));
        let plan = adapter.plan();
        assert!(!plan.is_paused(), "a resize does not pause the stream");
        assert_eq!((plan.width, plan.height, plan.target_kbps), (1600, 900, 6_000));

        adapter.set_rungs(screen((1600, 900), 2_000));
        assert_eq!(adapter.plan().target_kbps, 2_000, "a lower choice applies at once");
        adapter.set_rungs(screen((1600, 900), 5_000));
        assert_eq!(adapter.plan().target_kbps, 5_000, "and so does a higher one");
    }

    #[test]
    fn a_cpu_floor_keeps_its_cost_when_the_rungs_change() {
        let mut adapter = VideoAdapter::from_rungs(screen_h264((2560, 1440), 6_000)).unwrap();
        run(&mut adapter, RESERVE + 50_000, 10);
        adapter.apply_cpu(CpuVerdict::Downgrade);
        let plan = adapter.update(RESERVE + 50_000, AUDIO);
        assert_eq!((plan.height, plan.fps), (1440, 15), "a slow H.264 encoder keeps 1440p at 15 fps");
        adapter.set_rungs(screen_h264((2560, 1440), 5_000));
        assert_eq!(adapter.plan().fps, 15, "choosing a bitrate does not lift the floor");
    }

    #[test]
    fn the_lowest_rung_of_a_small_source_is_not_stretched() {
        let rungs = screen((320, 180), SCREEN_DEFAULT_KBPS);
        let bottom = rungs.last().unwrap();
        assert_eq!((bottom.width, bottom.height), (320, 180));
    }

    #[test]
    fn a_share_plans_for_its_rate_until_the_link_pushes_back() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut link = ScreenLink::default();
        assert_eq!(link.available(at(0), 1_000, 3_000), 1_000, "a new share starts on what the link is known to carry");
        assert_eq!(link.available(at(5), 1_000, 3_000), 3_000, "then a low estimate from a quiet stream is not a limit");
        assert_eq!(link.available(at(6), 1_100, 3_000), 3_000);
        assert_eq!(link.available(at(7), 5_000, 3_000), 5_000, "the estimate wins when it is higher");

        assert_eq!(link.available(at(8), 1_500, 3_000), 1_500, "a fall means the link pushed back");
        assert_eq!(link.available(at(10), 1_600, 3_000), 1_600, "and the estimate holds for a while");
        assert_eq!(link.available(at(17), 1_600, 3_000), 1_600);
        assert_eq!(link.available(at(18), 1_600, 3_000), 3_000, "then the share tries again");

        assert_eq!(link.available(at(19), 1_000, 3_000), 1_000, "a second push back");
        assert_eq!(link.available(at(38), 1_000, 3_000), 1_000, "holds twice as long");
        assert_eq!(link.available(at(39), 1_000, 3_000), 3_000);
    }

    #[test]
    fn a_link_that_keeps_pushing_back_is_tried_less_and_less_often() {
        let t0 = Instant::now();
        let mut link = ScreenLink::default();
        link.available(t0, 5_000, 3_000);
        let (mut now, mut holds) = (5u64, Vec::new());
        for _ in 0..6 {
            link.available(t0 + Duration::from_secs(now), 5_000, 3_000);
            now += 1;
            link.available(t0 + Duration::from_secs(now), 1_000, 3_000);
            let start = now;
            while link.available(t0 + Duration::from_secs(now), 1_000, 3_000) == 1_000 {
                now += 1;
            }
            holds.push(now - start);
        }
        assert_eq!(holds, [10, 20, 40, 80, 120, 120]);
    }

    #[test]
    fn a_quiet_spell_forgets_the_push_backs() {
        let t0 = Instant::now();
        let mut link = ScreenLink::default();
        link.available(t0, 5_000, 3_000);
        link.available(t0 + Duration::from_secs(6), 1_000, 3_000);
        link.available(t0 + Duration::from_secs(400), 5_000, 3_000);
        link.available(t0 + Duration::from_secs(401), 1_000, 3_000);
        assert_eq!(link.available(t0 + Duration::from_secs(410), 1_000, 3_000), 1_000);
        assert_eq!(link.available(t0 + Duration::from_secs(411), 1_000, 3_000), 3_000, "back to a 10 s hold");
    }

    #[test]
    fn a_chosen_resolution_and_frame_rate_hold_the_ladder() {
        let spec = ScreenSpec { source: (2560, 1440), target_kbps: 6_000, max_height: Some(720), max_fps: Some(15) };
        let rungs = screen_ladder(spec, &Codec::ALL);
        assert_eq!((rungs[0].width, rungs[0].height, rungs[0].fps), (1280, 720, 15));
        let small = ScreenSpec { source: (1280, 720), target_kbps: 6_000, max_height: Some(1080), ..ScreenSpec::default() };
        assert_eq!(screen_ladder(small, &Codec::ALL)[0].height, 720, "a chosen height never scales up");
    }

    #[test]
    fn a_nearly_idle_encoder_goes_back_to_the_best_rung_in_one_step() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut recovery = CpuRecovery::new();
        assert_eq!(recovery.observe(at(0), 0.9, false), CpuVerdict::Downgrade);
        assert_eq!(recovery.observe(at(6), 0.9, true), CpuVerdict::Downgrade, "a long spike steps down again");
        assert_eq!(recovery.observe(at(12), 0.2, true), CpuVerdict::Keep);
        assert_eq!(recovery.observe(at(21), 0.2, true), CpuVerdict::Keep, "not 10 s yet");
        assert_eq!(recovery.observe(at(22), 0.2, true), CpuVerdict::Recover, "10 s nearly idle");
        let mut adapter = adapter(Content::Screen);
        run(&mut adapter, RESERVE + 50_000, 10);
        for _ in 0..4 {
            adapter.apply_cpu(CpuVerdict::Downgrade);
        }
        assert!(adapter.cpu_stepped_down());
        adapter.apply_cpu(CpuVerdict::Recover);
        assert_eq!(adapter.update(RESERVE + 50_000, AUDIO).rung, Some(0), "the best rung the bandwidth allows");
    }

    #[test]
    fn a_recovery_that_fails_at_once_doubles_the_wait() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut recovery = CpuRecovery::new();
        recovery.observe(at(0), 0.9, false);
        recovery.observe(at(1), 0.2, true);
        assert_eq!(recovery.observe(at(11), 0.2, true), CpuVerdict::Recover);
        assert_eq!(recovery.observe(at(16), 0.9, false), CpuVerdict::Downgrade);
        assert_eq!(recovery.wait(), Duration::from_secs(60));
    }

    #[test]
    fn an_encoder_between_idle_and_slow_does_not_recover_at_once() {
        let t0 = Instant::now();
        let mut recovery = CpuRecovery::new();
        recovery.observe(t0, 0.9, false);
        for s in 1..25 {
            assert_ne!(recovery.observe(t0 + Duration::from_secs(s), 0.4, true), CpuVerdict::Recover);
        }
    }
}

