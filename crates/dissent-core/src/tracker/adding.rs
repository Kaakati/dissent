//! Adding files without waiting on them: what a pick or a drop is before anything is read, and fingerprint checks in the
//! background. See `docs/design/14-file-tracker.md`, "Adding files".
//!
//! Only paths the person chose may go any further. The app's picker and its native drop handler call [`Engine::allow_paths`],
//! so a page that was tricked cannot point these calls at a file nobody chose. A file then reaches `offered` as before,
//! once its check says it can be added.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use dissent_identity::CommunityId;
use serde::Serialize;

use super::bodies::FileType;
use super::catalog::ext_of;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::views::EngineEvent;

/// How many files one check fingerprints at once.
const CHECK_AT_ONCE: usize = 4;
/// How often one file's check reports its progress, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// A chosen path, as far as it can be told without reading the file.
#[derive(Clone, Debug, Serialize)]
pub struct PickedView {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: u64,
    pub is_dir: bool,
}

fn picked(path: &Path) -> Option<PickedView> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    Some(PickedView {
        path: path.to_string_lossy().into_owned(),
        ext: ext_of(&name),
        kind: FileType::of_name(&name).name().to_string(),
        size: if meta.is_file() { meta.len() } else { 0 },
        is_dir: meta.is_dir(),
        name,
    })
}

impl Engine {
    /// Remembers paths the person chose, through the OS picker or a native drop.
    pub fn allow_paths(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        state.allowed.extend(paths);
    }

    pub(super) fn check_allowed(&self, paths: &[PathBuf]) -> Result<()> {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        match paths.iter().find(|p| !state.allowed.contains(*p)) {
            Some(path) => Err(CoreError::Invalid(format!("{} was not picked or dropped", path.display()))),
            None => Ok(()),
        }
    }

    /// What `paths` are, as far as it can be told without reading them. Paths that are gone are left out.
    pub fn describe_paths(&self, paths: &[PathBuf]) -> Vec<PickedView> {
        paths.iter().filter_map(|p| picked(p)).collect()
    }

    /// Fingerprints `paths` in the background and reports each with `EngineEvent::FileCheck`, then
    /// `EngineEvent::FileCheckDone`. Returns the job, for `cancel_file_check`. Files that can be added become listable.
    pub fn start_file_check(&self, community: CommunityId, paths: Vec<PathBuf>) -> Result<u64> {
        self.check_allowed(&paths)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let job = {
            let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
            state.next_check += 1;
            let job = state.next_check;
            state.checks.insert(job, cancel.clone());
            job
        };
        let engine = self.clone();
        tokio::spawn(async move {
            let opened = Arc::new(engine.opened_listings(community));
            let limit = Arc::new(tokio::sync::Semaphore::new(CHECK_AT_ONCE));
            let mut asks = tokio::task::JoinSet::new();
            for path in paths {
                let (engine, opened, limit, cancel) = (engine.clone(), opened.clone(), limit.clone(), cancel.clone());
                asks.spawn(async move {
                    let _turn = limit.acquire_owned().await;
                    if !cancel.load(Ordering::Relaxed) {
                        engine.check_one(community, job, &path, &opened, &cancel).await;
                    }
                });
            }
            while asks.join_next().await.is_some() {}
            engine.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner).checks.remove(&job);
            engine.emit(EngineEvent::FileCheckDone { job });
        });
        Ok(job)
    }

    /// Stops a check at the next read of each file it is on. Nothing more is reported for it but `FileCheckDone`.
    pub fn cancel_file_check(&self, job: u64) {
        let state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cancel) = state.checks.get(&job) {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    async fn check_one(&self, community: CommunityId, job: u64, path: &Path, opened: &[super::catalog::Opened], cancel: &Arc<AtomicBool>) {
        let text = path.to_string_lossy().into_owned();
        let total = std::fs::symlink_metadata(path).map_or(0, |m| m.len());
        let (engine, flag, reported) = (self.clone(), cancel.clone(), text.clone());
        let mut last = Instant::now();
        let progress = move |done: u64| {
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                engine.emit(EngineEvent::FileCheck { job, path: reported.clone(), done, total, result: None });
            }
            !flag.load(Ordering::Relaxed)
        };
        // A category's allowed types are not applied here: the file's category can change while it is checked.
        let view = self.candidate(path, &[], opened, progress).await;
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if view.state == "ready" || view.state == "already_listed" {
            self.with_tracker(community, |l| l.offered.insert(path.to_path_buf()));
            self.remember_candidate(community, path, &view);
        }
        self.emit(EngineEvent::FileCheck { job, path: text, done: total, total, result: Some(view) });
    }
}
