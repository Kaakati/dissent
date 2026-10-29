//! What this device seeds, and when: the per-file seed limit, pause and stop, the global speed caps and their weekly
//! alternative, the most files seeded at once, and the metered-network and battery pauses. The app crate reports the
//! network and power state; core holds no OS code. See `docs/design/14-file-tracker.md`, "Seeding".
//!
//! A seed's state, decided from its record, the settings and the time:
//! * **Stopped**: stopped by hand, or its limit reached (ratio, hours since it started, or a date).
//! * **Paused**: paused by hand.
//! * **Scheduled**: everything is held back for now: a metered network, battery power, or an alternative schedule
//!   window whose upload cap is zero.
//! * **Queued**: over the most files seeded at once. The newest seeds go first.
//! * **Seeding**: served, and announced in holdings.

use std::path::PathBuf;

use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use super::{SeedLimit, SeedRecord};
use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, write_bin};
use crate::views::EngineEvent;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;

/// A weekly window with its own caps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AltSchedule {
    /// Days it applies on, bit 0 Monday to bit 6 Sunday.
    pub days: u8,
    /// Minutes after local midnight it starts and ends. An end before the start runs past midnight.
    pub start_min: u16,
    pub end_min: u16,
    /// Bytes per second, `None` for no cap.
    pub upload_bps: Option<u64>,
    pub download_bps: Option<u64>,
}

/// This device's seeding settings, for every server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedingSettings {
    /// Bytes per second, `None` for no cap.
    pub upload_bps: Option<u64>,
    pub download_bps: Option<u64>,
    pub alt: Option<AltSchedule>,
    pub alt_on: bool,
    pub max_active: u32,
    pub unmetered_only: bool,
    pub pause_on_battery: bool,
}

impl Default for SeedingSettings {
    fn default() -> Self {
        Self { upload_bps: None, download_bps: None, alt: None, alt_on: false, max_active: 5, unmetered_only: true, pause_on_battery: false }
    }
}

/// What the app reports of the device: whether the network is metered and whether it runs on battery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceConditions {
    pub metered: bool,
    pub on_battery: bool,
    /// Whether this platform can tell a metered network at all. When it cannot, "only on unmetered networks" cannot
    /// pause anything, and the UI says so.
    #[serde(default)]
    pub metered_known: bool,
}

/// A seed's state. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedState {
    Seeding,
    Paused,
    Scheduled,
    Queued,
    Stopped,
}

/// Whether the alternative schedule applies at `now_ms`, with `offset_min` the local time's offset from UTC.
pub fn in_window(alt: &AltSchedule, now_ms: u64, offset_min: i32) -> bool {
    let local = now_ms as i64 + i64::from(offset_min) * 60_000;
    let days = local.div_euclid(DAY_MS as i64);
    // 1970-01-01 was a Thursday: Monday is day 0 of the week here.
    let weekday = ((days + 3).rem_euclid(7)) as u8;
    let minute = (local.rem_euclid(DAY_MS as i64) / 60_000) as u16;
    let (start, end) = (alt.start_min, alt.end_min);
    let day_on = |d: u8| alt.days & (1 << d) != 0;
    if start <= end {
        day_on(weekday) && minute >= start && minute < end
    } else {
        // Past midnight: the evening of a listed day, or the morning after one.
        (day_on(weekday) && minute >= start) || (day_on((weekday + 6) % 7) && minute < end)
    }
}

/// Whether a seed's own limit is reached. `size` is the listing's ciphertext size.
pub(crate) fn limit_reached(record: &SeedRecord, size: u64, now_ms: u64) -> bool {
    match record.limit {
        SeedLimit::Forever => false,
        SeedLimit::Ratio(milli) => size > 0 && record.uploaded.saturating_mul(1000) >= size.saturating_mul(u64::from(milli)),
        SeedLimit::Hours(h) => now_ms >= record.since_ms.saturating_add(u64::from(h) * HOUR_MS),
        SeedLimit::Until(at) => now_ms >= at,
    }
}

/// The caps in force at `now_ms`: the alternative schedule's inside its window, the main ones outside.
pub fn caps(settings: &SeedingSettings, now_ms: u64, offset_min: i32) -> (Option<u64>, Option<u64>) {
    match settings.alt.filter(|a| settings.alt_on && in_window(a, now_ms, offset_min)) {
        Some(alt) => (alt.upload_bps, alt.download_bps),
        None => (settings.upload_bps, settings.download_bps),
    }
}

/// Whether seeding is held back everywhere right now.
pub fn held_back(settings: &SeedingSettings, conditions: DeviceConditions, now_ms: u64, offset_min: i32) -> bool {
    (settings.unmetered_only && conditions.metered)
        || (settings.pause_on_battery && conditions.on_battery)
        || caps(settings, now_ms, offset_min).0 == Some(0)
}

/// The states of `seeds`, each with its listing's ciphertext size, in the same order. Queued ones are those past
/// `max_active` among the seeds that would otherwise seed, newest start first.
pub(crate) fn states(
    seeds: &[(&SeedRecord, u64)],
    settings: &SeedingSettings,
    conditions: DeviceConditions,
    now_ms: u64,
    offset_min: i32,
) -> Vec<SeedState> {
    let held = held_back(settings, conditions, now_ms, offset_min);
    let mut out: Vec<SeedState> = seeds
        .iter()
        .map(|(r, size)| {
            if r.stopped || limit_reached(r, *size, now_ms) {
                SeedState::Stopped
            } else if r.paused {
                SeedState::Paused
            } else if held {
                SeedState::Scheduled
            } else {
                SeedState::Seeding
            }
        })
        .collect();
    let mut active: Vec<usize> = (0..seeds.len()).filter(|i| out[*i] == SeedState::Seeding).collect();
    active.sort_by_key(|i| std::cmp::Reverse(seeds[*i].0.since_ms));
    for i in active.into_iter().skip(settings.max_active.max(1) as usize) {
        out[i] = SeedState::Queued;
    }
    out
}

/// One row of My seeding.
#[derive(Clone, Debug, Serialize)]
pub struct SeedRowView {
    pub blob: String,
    pub listing: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub category: String,
    pub folder: Option<String>,
    pub path: Option<String>,
    pub state: SeedState,
    /// What the state means here, for the row's note.
    pub note: String,
    pub size: u64,
    pub uploaded: u64,
    pub ratio: f64,
    /// "forever", "ratio:<thousandths>", "hours:<n>" or "until:<ms>".
    pub limit: String,
    pub since_ms: u64,
    pub downloaded: bool,
}

/// My seeding, for one server.
#[derive(Clone, Debug, Serialize)]
pub struct SeedingView {
    pub rows: Vec<SeedRowView>,
    pub seeding: u32,
    pub uploaded: u64,
    pub settings: SeedingSettings,
    pub conditions: DeviceConditions,
    pub caps_now: (Option<u64>, Option<u64>),
    pub held_back: bool,
}

pub fn limit_text(limit: SeedLimit) -> String {
    match limit {
        SeedLimit::Forever => "forever".into(),
        SeedLimit::Ratio(m) => format!("ratio:{m}"),
        SeedLimit::Hours(h) => format!("hours:{h}"),
        SeedLimit::Until(at) => format!("until:{at}"),
    }
}

/// Parses `limit_text`'s forms.
pub fn parse_limit(text: &str) -> Result<SeedLimit> {
    let (kind, value) = text.split_once(':').unwrap_or((text, ""));
    let number = || value.parse::<u64>().map_err(|_| CoreError::Invalid(format!("bad seed limit {text}")));
    Ok(match kind {
        "forever" => SeedLimit::Forever,
        "ratio" => SeedLimit::Ratio(number()?.min(u64::from(u32::MAX)) as u32),
        "hours" => SeedLimit::Hours(number()?.min(u64::from(u32::MAX)) as u32),
        "until" => SeedLimit::Until(number()?),
        _ => return Err(CoreError::Invalid(format!("bad seed limit {text}"))),
    })
}

/// A blob hash from its lowercase base32 form, as rows carry it.
pub fn parse_blob(text: &str) -> Result<[u8; 32]> {
    data_encoding::BASE32_NOPAD
        .decode(text.trim().to_ascii_uppercase().as_bytes())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| CoreError::Invalid("bad file id".into()))
}

fn settings_path(engine: &Engine) -> PathBuf {
    engine.read().paths.local_state().with_file_name("seeding.bin")
}

impl Engine {
    /// This device's seeding settings.
    pub fn seeding_settings(&self) -> SeedingSettings {
        read_bin(&settings_path(self)).ok().flatten().unwrap_or_default()
    }

    pub fn set_seeding_settings(&self, settings: SeedingSettings) -> Result<()> {
        if settings.max_active == 0 || settings.max_active > 100 {
            return Err(CoreError::Invalid("seed between 1 and 100 files at once".into()));
        }
        if let Some(alt) = settings.alt {
            if alt.start_min >= 24 * 60 || alt.end_min > 24 * 60 || alt.days & 0x80 != 0 {
                return Err(CoreError::Invalid("the schedule's times and days are out of range".into()));
            }
        }
        write_bin(&settings_path(self), &settings)?;
        self.inner.files.set_download_cap(caps(&settings, now_ms(), self.utc_offset_min()).1);
        self.emit(EngineEvent::TransfersChanged);
        Ok(())
    }

    /// What the app says of the device. Kept in memory; the app reports it again after a restart.
    pub fn set_device_conditions(&self, conditions: DeviceConditions) {
        *self.inner.device_conditions.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = conditions;
        self.emit(EngineEvent::TransfersChanged);
    }

    pub fn device_conditions(&self) -> DeviceConditions {
        *self.inner.device_conditions.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The local time's offset from UTC, in minutes, as the app reported it with the conditions. 0 until then.
    pub(crate) fn utc_offset_min(&self) -> i32 {
        self.inner.utc_offset_min.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_utc_offset_min(&self, minutes: i32) {
        self.inner.utc_offset_min.store(minutes.clamp(-18 * 60, 18 * 60), std::sync::atomic::Ordering::Relaxed);
    }

    /// The state of each seed of `community`, by blob.
    pub(crate) fn seed_states(&self, community: CommunityId) -> std::collections::BTreeMap<[u8; 32], SeedState> {
        let sizes: std::collections::BTreeMap<[u8; 32], u64> =
            self.opened_listings(community).into_iter().map(|o| (o.body.blob, o.body.cipher_size)).collect();
        let book = self.with_tracker(community, |l| l.book.seeds.clone());
        let seeds: Vec<([u8; 32], SeedRecord)> = book.into_iter().collect();
        let refs: Vec<(&SeedRecord, u64)> = seeds.iter().map(|(b, r)| (r, sizes.get(b).copied().unwrap_or(0))).collect();
        let states = states(&refs, &self.seeding_settings(), self.device_conditions(), now_ms(), self.utc_offset_min());
        seeds.iter().map(|(b, _)| *b).zip(states).collect()
    }

    /// My seeding for `community`.
    pub fn seeding_view(&self, community: CommunityId) -> Result<SeedingView> {
        let opened = self.opened_listings(community);
        let states = self.seed_states(community);
        let book = self.with_tracker(community, |l| l.book.clone());
        let settings = self.seeding_settings();
        let conditions = self.device_conditions();
        let now = now_ms();
        let offset = self.utc_offset_min();
        if !self.read().communities.contains_key(&community) {
            return Err(crate::engine::not_found(&community));
        }
        let mut rows = Vec::new();
        for (blob, record) in &book.seeds {
            let Some(o) = opened.iter().find(|o| o.body.blob == *blob) else { continue };
            let state = states.get(blob).copied().unwrap_or(SeedState::Stopped);
            let note = match state {
                SeedState::Seeding => "Serving now".to_string(),
                SeedState::Paused => "Paused by you".to_string(),
                SeedState::Scheduled if settings.unmetered_only && conditions.metered => "Waits for an unmetered network".to_string(),
                SeedState::Scheduled if settings.pause_on_battery && conditions.on_battery => "Waits for power".to_string(),
                SeedState::Scheduled => "Paused by the schedule".to_string(),
                SeedState::Queued => format!("Waits for a free slot · {} at once", settings.max_active),
                SeedState::Stopped if record.stopped => "Stopped by you".to_string(),
                SeedState::Stopped => "Seed limit reached".to_string(),
            };
            let name = o.body.name.clone();
            rows.push(SeedRowView {
                blob: data_encoding::BASE32_NOPAD.encode(blob).to_lowercase(),
                listing: o.id.to_string(),
                ext: name.rsplit_once('.').map(|(_, e)| e.to_ascii_uppercase()).unwrap_or_default(),
                kind: o.body.kind.name().to_string(),
                name,
                category: o.category.to_string(),
                folder: book.folders.get(&o.category).map(|p| p.to_string_lossy().into_owned()),
                path: record.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                state,
                note,
                size: o.body.cipher_size,
                uploaded: record.uploaded,
                ratio: if o.body.cipher_size == 0 { 0.0 } else { record.uploaded as f64 / o.body.cipher_size as f64 },
                limit: limit_text(record.limit),
                since_ms: record.since_ms,
                downloaded: record.downloaded,
            });
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.since_ms));
        Ok(SeedingView {
            seeding: rows.iter().filter(|r| r.state == SeedState::Seeding).count() as u32,
            uploaded: rows.iter().map(|r| r.uploaded).sum(),
            rows,
            caps_now: caps(&settings, now, offset),
            held_back: held_back(&settings, conditions, now, offset),
            settings,
            conditions,
        })
    }

    /// Serves each linked file of `community` that is seeding, and stops serving the others. A linked file that changed
    /// on disk since it was linked is not served again until it is added anew.
    pub(crate) fn reconcile_linked(&self, community: CommunityId) {
        let states = self.seed_states(community);
        let linked: Vec<([u8; 32], SeedRecord)> =
            self.with_tracker(community, |l| l.book.seeds.iter().filter(|(_, r)| r.linked).map(|(b, r)| (*b, r.clone())).collect());
        if linked.is_empty() {
            return;
        }
        let opened = self.opened_listings(community);
        for (blob, record) in linked {
            let want = states.get(&blob) == Some(&SeedState::Seeding);
            let have = self.inner.files.is_linked(blob);
            if !want && have {
                self.inner.files.unlink(blob);
            } else if want && !have {
                let (Some(path), Some(o)) = (record.path.clone(), opened.iter().find(|o| o.body.blob == blob)) else { continue };
                let Some(files_key) = self.files_key(community, o.body.content_generation) else { continue };
                let linked = dissent_files::Linked {
                    path,
                    key: dissent_files::tracker::content_key(&files_key, &o.body.fingerprint),
                    size: o.body.size,
                    cipher_size: o.body.cipher_size,
                    modified: record.modified_ms.map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
                };
                if let Err(e) = self.inner.files.relink(blob, linked) {
                    tracing::warn!(error = %e, "a linked file is not served");
                }
            }
        }
    }

    /// Serves the linked files of every community. Called on start and from the maintenance tick, since states move
    /// with time, the schedule and the device's conditions.
    pub(crate) fn reconcile_all_linked(&self) {
        let communities: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for community in communities {
            self.reconcile_linked(community);
        }
    }

    fn edit_seed(&self, community: CommunityId, blob: [u8; 32], f: impl FnOnce(&mut SeedRecord)) -> Result<()> {
        let found = self.with_tracker(community, |l| match l.book.seeds.get_mut(&blob) {
            Some(r) => {
                f(r);
                true
            }
            None => false,
        });
        if !found {
            return Err(CoreError::NotFound("that file is not seeded here".into()));
        }
        self.save_tracker_book(community);
        self.reconcile_linked(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(())
    }

    /// Sets how long a file is seeded. A new limit restarts a stopped seed whose limit it lifts.
    pub fn set_seed_limit(&self, community: CommunityId, blob: [u8; 32], limit: SeedLimit) -> Result<()> {
        self.edit_seed(community, blob, |r| {
            r.limit = limit;
            if let SeedLimit::Hours(_) = limit {
                r.since_ms = now_ms();
            }
        })
    }

    /// Pauses (`true`) or resumes a file. Resuming a stopped file restarts it.
    pub fn pause_seed(&self, community: CommunityId, blob: [u8; 32], paused: bool) -> Result<()> {
        self.edit_seed(community, blob, |r| {
            r.paused = paused;
            if !paused {
                r.stopped = false;
            }
        })
    }

    /// Stops seeding a file. The copy is kept until the file is removed.
    pub fn stop_seed(&self, community: CommunityId, blob: [u8; 32]) -> Result<()> {
        self.edit_seed(community, blob, |r| r.stopped = true)
    }

    /// Pauses or resumes every file of `community`.
    pub fn pause_all_seeds(&self, community: CommunityId, paused: bool) -> Result<()> {
        self.with_tracker(community, |l| {
            for r in l.book.seeds.values_mut() {
                r.paused = paused;
            }
        });
        self.save_tracker_book(community);
        self.reconcile_linked(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_events::EventId;

    fn record(since_ms: u64, limit: SeedLimit) -> SeedRecord {
        SeedRecord {
            listing: EventId([0; 32]),
            path: None,
            limit,
            paused: false,
            stopped: false,
            linked: false,
            modified_ms: None,
            uploaded: 0,
            since_ms,
            downloaded: false,
        }
    }

    const MONDAY_NOON: u64 = 1_793_016_000_000; // 2026-10-26 12:00 UTC, a Monday

    #[test]
    fn the_week_starts_on_monday() {
        let weekdays = AltSchedule { days: 0b001_1111, start_min: 9 * 60, end_min: 18 * 60, upload_bps: Some(2), download_bps: Some(10) };
        assert!(in_window(&weekdays, MONDAY_NOON, 0));
        assert!(!in_window(&weekdays, MONDAY_NOON + 7 * HOUR_MS, 0), "19:00 is after the window");
        assert!(!in_window(&weekdays, MONDAY_NOON + 5 * DAY_MS, 0), "Saturday");
        assert!(!in_window(&weekdays, MONDAY_NOON, 8 * 60), "20:00 local in UTC+8");
        let night = AltSchedule { days: 0b000_0001, start_min: 22 * 60, end_min: 6 * 60, upload_bps: None, download_bps: None };
        assert!(in_window(&night, MONDAY_NOON + 11 * HOUR_MS, 0), "Monday 23:00");
        assert!(in_window(&night, MONDAY_NOON + 17 * HOUR_MS, 0), "Tuesday 05:00 counts for Monday night");
        assert!(!in_window(&night, MONDAY_NOON + 7 * HOUR_MS, 0));
    }

    #[test]
    fn limits_end_a_seed() {
        let mut r = record(0, SeedLimit::Ratio(2000));
        r.uploaded = 1999;
        assert!(!limit_reached(&r, 1000, 0));
        r.uploaded = 2000;
        assert!(limit_reached(&r, 1000, 0));
        assert!(!limit_reached(&record(0, SeedLimit::Hours(48)), 1, 47 * HOUR_MS));
        assert!(limit_reached(&record(0, SeedLimit::Hours(48)), 1, 48 * HOUR_MS));
        assert!(limit_reached(&record(0, SeedLimit::Until(5)), 1, 5));
        assert!(!limit_reached(&record(0, SeedLimit::Forever), 1, u64::MAX));
    }

    #[test]
    fn states_follow_hands_limits_conditions_and_slots() {
        let settings = SeedingSettings { max_active: 2, unmetered_only: true, pause_on_battery: true, ..SeedingSettings::default() };
        let mut paused = record(1, SeedLimit::Forever);
        paused.paused = true;
        let mut stopped = record(2, SeedLimit::Forever);
        stopped.stopped = true;
        let (a, b, c) = (record(10, SeedLimit::Forever), record(20, SeedLimit::Forever), record(30, SeedLimit::Forever));
        let ended = record(0, SeedLimit::Until(1));
        let seeds = [(&paused, 1), (&stopped, 1), (&a, 1), (&b, 1), (&c, 1), (&ended, 1)];
        let free = DeviceConditions::default();
        assert_eq!(
            states(&seeds, &settings, free, 100, 0),
            [SeedState::Paused, SeedState::Stopped, SeedState::Queued, SeedState::Seeding, SeedState::Seeding, SeedState::Stopped],
            "the two newest take the two slots"
        );
        let metered = DeviceConditions { metered: true, ..DeviceConditions::default() };
        assert_eq!(states(&seeds, &settings, metered, 100, 0)[3], SeedState::Scheduled);
        let battery = DeviceConditions { on_battery: true, ..DeviceConditions::default() };
        assert_eq!(states(&seeds, &settings, battery, 100, 0)[4], SeedState::Scheduled);
    }

    #[test]
    fn limits_round_trip_as_text() {
        for limit in [SeedLimit::Forever, SeedLimit::Ratio(5000), SeedLimit::Hours(48), SeedLimit::Until(1_800_000_000_000)] {
            assert_eq!(parse_limit(&limit_text(limit)).unwrap(), limit);
        }
        assert!(parse_limit("ratio:x").is_err());
    }
}
