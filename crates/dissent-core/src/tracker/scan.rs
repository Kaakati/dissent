//! Linked folders: finding the files in a category's folder that this device does not seed yet, and listing them on
//! their own when the category's auto-import is on. See `docs/design/14-file-tracker.md`, "Seeding".

use std::path::{Path, PathBuf};
use std::time::Duration;

use dissent_events::CategoryId;
use dissent_identity::CommunityId;
use tracing::{debug, warn};

use super::catalog::{AddItem, CandidateView};
use super::SeedLimit;
use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// How deep a scan goes below the linked folder.
const MAX_DEPTH: usize = 4;
/// Most files one scan reports.
const MAX_FILES: usize = 500;
/// How often auto-import scans, at most.
pub(crate) const AUTO_IMPORT_EVERY: Duration = Duration::from_secs(10 * 60);

/// The regular files under `dir`, `depth` levels deep at most, skipping hidden names.
fn files_under(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_file() => out.push(path),
            Ok(t) if t.is_dir() && depth > 0 => files_under(&path, depth - 1, out),
            _ => {}
        }
    }
}

impl Engine {
    /// The files in `category`'s linked folder that this device does not seed yet, with what adding each would do.
    pub async fn scan_category_folder(&self, community: CommunityId, category: CategoryId) -> Result<Vec<CandidateView>> {
        let (folder, seeded) = self.with_tracker(community, |l| {
            let seeded: Vec<PathBuf> = l.book.seeds.values().filter_map(|s| s.path.clone()).collect();
            (l.book.folders.get(&category).cloned(), seeded)
        });
        let folder = folder.ok_or_else(|| CoreError::Invalid("this category is not linked to a folder here".into()))?;
        let found = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            files_under(&folder, MAX_DEPTH, &mut out);
            out.retain(|p| !seeded.contains(p));
            out.sort();
            out
        })
        .await
        .map_err(|e| CoreError::Invalid(e.to_string()))?;
        self.prepare_listing(community, category, found).await
    }

    /// Lists what each auto-import category's folder holds that is new here, with the category's default tags.
    pub(crate) async fn auto_import(&self) {
        let jobs: Vec<(CommunityId, CategoryId)> = {
            let communities: Vec<CommunityId> = self.read().communities.keys().copied().collect();
            communities
                .into_iter()
                .flat_map(|c| self.with_tracker(c, |l| l.book.auto_import.iter().map(|cat| (c, *cat)).collect::<Vec<_>>()))
                .collect()
        };
        for (community, category) in jobs {
            let found = match self.scan_category_folder(community, category).await {
                Ok(found) => found,
                Err(e) => {
                    debug!(error = %e, "auto-import skipped");
                    continue;
                }
            };
            let tags = self
                .files_view(community)
                .ok()
                .and_then(|v| v.categories.into_iter().find(|c| c.id == category.to_string()))
                .map(|c| c.default_tags)
                .unwrap_or_default();
            let items: Vec<AddItem> = found
                .into_iter()
                .filter(|f| f.state == "ready" || f.state == "already_listed")
                .map(|f| AddItem { path: f.path, tags: tags.clone(), description: String::new() })
                .collect();
            if items.is_empty() {
                continue;
            }
            if let Err(e) = self.list_files(community, category, items, SeedLimit::Forever).await {
                warn!(error = %e, "auto-import could not list a file");
            }
        }
    }
}
