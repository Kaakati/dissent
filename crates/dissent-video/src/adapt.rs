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
/// 360p, so a slow AV1 encoder can fall back to a codec that is cheaper to encode.
const CAMERA: [Rung; 4] = [
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

/// Screen steps, best first. Frame rate drops first, and resolution holds until the bitrate
/// runs out, so text stays sharp. The lowest rung is H.264 at 360p.
const SCREEN: [Rung; 6] = [
    Rung {
        codec: Codec::Av1,
        width: 1920,
        height: 1080,
        fps: 15,
        min_kbps: 1500,
        max_kbps: 3000,
    },
    Rung {
        codec: Codec::Av1,
        width: 1920,
        height: 1080,
        fps: 10,
        min_kbps: 1000,
        max_kbps: 1500,
    },
    Rung {
        codec: Codec::Av1,
        width: 1920,
        height: 1080,
        fps: 5,
        min_kbps: 700,
        max_kbps: 1000,
    },
    Rung {
        codec: Codec::Av1,
        width: 1280,
        height: 720,
        fps: 10,
        min_kbps: 400,
        max_kbps: 700,
    },
    Rung {
        codec: Codec::Av1,
        width: 1280,
        height: 720,
        fps: 5,
        min_kbps: 250,
        max_kbps: 400,
    },
    Rung {
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 10,
        min_kbps: 150,
        max_kbps: 250,
    },
];

pub fn ladder(content: Content) -> &'static [Rung] {
    match content {
        Content::Camera => &CAMERA,
        Content::Screen => &SCREEN,
    }
}

/// The decision for one sample. A paused plan has no rung and no bitrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoPlan {
    /// Codec for the encoder, or `None` while paused.
    pub codec: Option<Codec>,
    /// Index into `ladder(content)`, or `None` while paused.
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
        let ladder: Vec<Rung> = ladder(content)
            .iter()
            .copied()
            .filter(|rung| codecs.contains(&rung.codec))
            .collect();
        if ladder.is_empty() {
            return None;
        }
        Some(Self {
            ladder,
            rung: None,
            cpu_floor: 0,
            target_kbps: 0,
            down_streak: 0,
            up_streak: 0,
        })
    }

    /// The rungs this adapter moves between, best first.
    pub fn rungs(&self) -> &[Rung] {
        &self.ladder
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
    pub fn apply_cpu(&mut self, verdict: CpuVerdict) {
        let last = self.ladder.len() - 1;
        match verdict {
            CpuVerdict::Keep => {}
            CpuVerdict::Downgrade => self.cpu_floor = (self.cpu_floor + 1).min(last),
            CpuVerdict::Upgrade => self.cpu_floor = self.cpu_floor.saturating_sub(1),
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

fn frame_period_ms(fps: u32) -> f64 {
    1_000.0 / fps as f64
}

fn pixel_rate(rung: Rung) -> f64 {
    (rung.width * rung.height * rung.fps) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIO: u32 = 32;
    const RESERVE: u32 = 56;

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
        for content in [Content::Camera, Content::Screen] {
            let steps = ladder(content);
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
        let steps = ladder(Content::Screen);
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
        assert_eq!(adapter.plan().rung, Some(0));

        adapter.update(RESERVE + 500, AUDIO);
        assert_eq!(
            adapter.plan().rung,
            Some(0),
            "one low sample must not change rung"
        );
        run(&mut adapter, RESERVE + 2_000, 1);
        assert_eq!(adapter.plan().rung, Some(0));
    }

    #[test]
    fn a_sustained_drop_moves_down_the_ladder() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        let plan = run(&mut adapter, RESERVE + 700, DOWN_SAMPLES);
        assert_eq!(plan.rung, Some(1));
        assert_eq!(plan.fps, 15, "camera drops frame rate before resolution");
        assert_eq!(plan.height, 720);
    }

    #[test]
    fn boundary_noise_does_not_flap() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 1_300, 10);
        let start = adapter.plan().rung;
        assert_eq!(start, Some(0));
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
        let steps = ladder(Content::Camera);
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
        let steps = ladder(Content::Camera);
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
        let steps = ladder(Content::Camera);
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
        let steps = ladder(Content::Camera);
        let mut governor = CpuGovernor::new();
        for sample in 1..=CPU_FAST_SAMPLES {
            let verdict = governor.observe(5.0, steps, 1);
            if sample < CPU_FAST_SAMPLES {
                assert_eq!(verdict, CpuVerdict::Keep);
            } else {
                assert_eq!(verdict, CpuVerdict::Upgrade);
            }
        }
    }

    #[test]
    fn a_fast_encoder_at_the_top_rung_never_downgrades() {
        let steps = ladder(Content::Camera);
        let mut governor = CpuGovernor::new();
        for _ in 0..1_000 {
            assert_eq!(governor.observe(5.0, steps, 0), CpuVerdict::Keep);
        }
    }

    #[test]
    fn cost_near_the_threshold_does_not_flap() {
        let steps = ladder(Content::Camera);
        let mut governor = CpuGovernor::new();
        for i in 0..500 {
            let cost = if i % 2 == 0 { 20.0 } else { 30.0 };
            assert_eq!(governor.observe(cost, steps, 0), CpuVerdict::Keep);
        }
    }

    #[test]
    fn cpu_downgrade_lowers_the_plan_even_with_bandwidth_to_spare() {
        let mut adapter = adapter(Content::Camera);
        run(&mut adapter, RESERVE + 2_000, 10);
        assert_eq!(adapter.plan().rung, Some(0));

        adapter.apply_cpu(CpuVerdict::Downgrade);
        let plan = adapter.update(RESERVE + 2_000, AUDIO);
        assert_eq!(plan.rung, Some(1));
        assert_eq!(plan.fps, 15);

        adapter.apply_cpu(CpuVerdict::Upgrade);
        let plan = adapter.update(RESERVE + 2_000, AUDIO);
        assert_eq!(plan.rung, Some(0));
    }

    #[test]
    fn cpu_floor_never_leaves_the_ladder() {
        let mut adapter = adapter(Content::Screen);
        for _ in 0..20 {
            adapter.apply_cpu(CpuVerdict::Downgrade);
        }
        run(&mut adapter, RESERVE + 2_000, 10);
        assert_eq!(adapter.plan().rung, Some(SCREEN.len() - 1));
    }
}
