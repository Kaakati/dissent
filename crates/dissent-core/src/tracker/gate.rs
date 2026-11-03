//! What the tracker tells the members-only blob gate (`blob_gate`): which category each of its blobs belongs to, which
//! blobs it serves right now, how many bytes it served, how long to wait under the upload cap, and which blobs it keeps
//! pinned. See `docs/design/14-file-tracker.md`, "Downloads" and "Seeding".

use std::collections::{BTreeSet, HashMap};
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use dissent_identity::{CommunityId, DeviceId};

use super::catalog::TRANSFER_PREFIX;
use super::seeding::{caps, SeedState, SeedingSettings};
use super::slots::{RequestKey, Take};
use crate::blob_gate::Scope;
use crate::engine::{now_ms, Engine};

/// A token bucket for the upload cap: when it was last filled and what it holds.
#[derive(Default)]
pub(crate) struct UploadBucket {
    at: Option<Instant>,
    level: f64,
}

/// How long a device's answer to the features probe is trusted.
const FEATURES_KEPT: Duration = Duration::from_secs(600);

impl Engine {
    /// Whether `blob` is a seed in some community's book. Only seeds take upload slots: chat files, pictures and
    /// avatars are served whatever the slots say.
    pub(crate) fn is_tracker_seed(&self, blob: &[u8; 32]) -> bool {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.communities.values().any(|l| l.book.seeds.contains_key(blob))
    }

    /// The most upload transfers of seeds at once.
    fn upload_slots_max(&self) -> usize {
        self.cached_seeding_settings().max_active.max(1) as usize
    }

    /// Upload transfers of seeds running now, and the most at once.
    pub(crate) fn upload_slots(&self) -> (u32, u32) {
        let max = self.upload_slots_max() as u32;
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        (state.slots.active() as u32, max)
    }

    /// Whether `device` takes a busy answer. Until it has said, it is treated as a release that does not, and asked.
    fn knows_busy(&self, device: DeviceId) -> bool {
        let now = Instant::now();
        let ask = {
            let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
            match state.peer_features.get(&device) {
                Some((Some(bits), at)) if now.duration_since(*at) < FEATURES_KEPT => {
                    return bits & crate::custom_roles::FEATURE_SEED_SLOTS != 0;
                }
                Some((None, at)) if now.duration_since(*at) < FEATURES_KEPT => false,
                _ => {
                    state.peer_features.insert(device, (None, now));
                    true
                }
            }
        };
        if ask {
            let engine = self.clone();
            self.spawn_task(async move {
                let reply = tokio::time::timeout(Duration::from_secs(5), engine.call(device, &crate::rpc::Request::Features)).await;
                if let Ok(Ok(crate::rpc::Response::Features(bits))) = reply {
                    let mut state = engine.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
                    state.peer_features.insert(device, (Some(bits), Instant::now()));
                }
            });
        }
        false
    }

    /// Asks for an upload slot for the request `key` of `device`, for the seed `blob`.
    pub(crate) fn take_slot(&self, key: RequestKey, device: DeviceId, blob: &[u8; 32]) -> Take {
        // A file seeded in place is answered outside iroh-blobs' handlers, which cannot send the busy code, so a request for
        // one waits for a slot as an older downloader's does.
        let knows = !self.inner.files.is_linked(*blob) && self.knows_busy(device);
        let max = self.upload_slots_max();
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        let take = state.slots.take(key, device, max, knows, Instant::now());
        match take {
            Take::Busy => *state.slot_refusals.entry(device).or_default() += 1,
            Take::Held(_) => *state.slot_holds.entry(device).or_default() += 1,
            Take::Granted => {}
        }
        take
    }

    /// Ends the request `key`'s upload transfer, and hands its slot on.
    pub(crate) fn release_slot(&self, key: RequestKey) {
        let max = self.upload_slots_max();
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.slots.release(key, max, Instant::now());
    }

    /// Forgets a held request that waited too long. It is refused.
    pub(crate) fn forget_slot(&self, key: RequestKey, device: DeviceId) {
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.slots.forget(key);
        *state.slot_refusals.entry(device).or_default() += 1;
    }

    /// Every blob the tracker keeps pinned for some community: the seeds in its books, whatever their state, since a
    /// paused or stopped seed keeps its copy until its listing is removed.
    pub(crate) fn tracker_kept_blobs(&self) -> Vec<[u8; 32]> {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.communities.values().flat_map(|l| l.book.seeds.keys().copied()).collect()
    }

    /// Whether a loaded tracker book could not be read, now or on an earlier start. Its seeds are not known, so nothing
    /// may be unpinned as unreferenced.
    pub(crate) fn tracker_book_damaged(&self) -> bool {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.communities.values().any(|l| l.book_damaged)
    }

    /// Whether the gate may serve `blob`: false only for a tracker seed that is not seeding now. A blob the tracker
    /// does not seed is not its business.
    pub(crate) fn tracker_serves(&self, blob: [u8; 32]) -> bool {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .communities
            .values()
            .find(|l| l.book.seeds.contains_key(&blob))
            .is_none_or(|l| l.serving.contains(&blob))
    }

    /// Counts `bytes` of `blob` served toward its seed's upload counter. Saved with the next maintenance pass.
    pub(crate) fn tracker_served(&self, blob: [u8; 32], bytes: u64) {
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(local) = state.communities.values_mut().find(|l| l.book.seeds.contains_key(&blob)) {
            if let Some(seed) = local.book.seeds.get_mut(&blob) {
                seed.uploaded = seed.uploaded.saturating_add(bytes);
                local.dirty = true;
            }
        }
    }

    /// How long to wait before sending `bytes` more under the upload cap in force now. Zero without one.
    pub(crate) fn upload_wait(&self, bytes: u64) -> Duration {
        let settings = self.cached_seeding_settings();
        let Some(bps) = caps(&settings, now_ms(), self.utc_offset_min()).0.filter(|b| *b > 0) else {
            return Duration::ZERO;
        };
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        let bucket = &mut state.upload;
        let now = Instant::now();
        let refill = bucket.at.map_or(bps as f64, |at| now.duration_since(at).as_secs_f64() * bps as f64);
        // The bucket holds at most one second of bytes.
        bucket.level = (bucket.level + refill).min(bps as f64) - bytes as f64;
        bucket.at = Some(now);
        if bucket.level >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-bucket.level / bps as f64)
        }
    }

    /// The seeding settings, read from disk once and kept until they change.
    pub(crate) fn cached_seeding_settings(&self) -> SeedingSettings {
        if let Some(s) = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner).settings.clone() {
            return s;
        }
        let s = self.seeding_settings();
        self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner).settings = Some(s.clone());
        s
    }

    /// Records which blobs of `community` this device serves now, and the category each of its seeds and downloads
    /// belongs to, for the gate. Called with every reconcile.
    pub(crate) fn sync_tracker_gate(&self, community: CommunityId, states: &std::collections::BTreeMap<[u8; 32], SeedState>) {
        let serving: BTreeSet<[u8; 32]> = states.iter().filter(|(_, s)| **s == SeedState::Seeding).map(|(b, _)| *b).collect();
        let category_of: HashMap<[u8; 32], dissent_events::CategoryId> =
            self.opened_blobs(community).into_iter().map(|o| (o.body.blob, o.category)).collect();
        let prefix = format!("{TRANSFER_PREFIX}{community}:");
        let downloading: Vec<[u8; 32]> = {
            let w = self.read();
            w.attach.transfers.values().filter(|t| t.message_id.starts_with(&prefix)).map(|t| t.file.hash).collect()
        };
        let mut blobs: Vec<[u8; 32]> = states.keys().copied().collect();
        blobs.extend(downloading);
        self.with_tracker(community, |l| l.serving = serving);
        let mut w = self.write();
        for blob in blobs {
            if let Some(category) = category_of.get(&blob) {
                w.blob_scopes.tracker.insert(blob, Scope::Category(community, *category));
                w.blob_scopes.add(blob, Scope::Category(community, *category));
            }
        }
    }

    /// Saves the books whose upload counters moved. Called from the maintenance pass.
    pub(crate) fn save_dirty_tracker_books(&self) {
        self.prune_meters();
        let dirty: Vec<CommunityId> = {
            let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
            state
                .communities
                .iter_mut()
                .filter(|(_, l)| l.dirty)
                .map(|(id, l)| {
                    l.dirty = false;
                    *id
                })
                .collect()
        };
        for community in dirty {
            self.save_tracker_book(community);
        }
    }
}
