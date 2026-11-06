//! Adding a whole folder: what it holds, what is left out and why, and seeding its files where they are. See
//! `docs/design/14-file-tracker.md`, "Adding files".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dissent_events::CategoryId;
use dissent_identity::CommunityId;
use serde::Serialize;

use super::bodies::{FileType, MAX_TAG_CHARS};
use super::catalog::{ext_of, AddItem};
use super::drops::{choose_category, looks_like_credentials};
use super::SeedLimit;
use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// How deep a folder is walked below the one chosen, as for a linked folder.
pub(crate) const MAX_DEPTH: usize = 4;
/// Most files one folder adds, as for a linked folder.
pub(crate) const MAX_FILES: usize = 500;
/// Most flagged files one folder lists row by row. The rest are counted.
const MAX_FLAGGED: usize = 500;
/// How many entries a walk looks at before it stops counting, so a chosen drive does not walk for an hour.
const MAX_ENTRIES: usize = 100_000;

/// One file found in a folder. `flag` says why it is not ticked to begin with.
#[derive(Clone, Debug, Serialize)]
pub struct FolderFile {
    pub path: String,
    /// The path below the folder, with forward slashes.
    pub relative: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: u64,
    /// "", "hidden", "system", "credentials", "link" or "too_large".
    pub flag: String,
}

/// What a folder holds, and what was left out.
#[derive(Clone, Debug, Serialize)]
pub struct FolderScan {
    pub root: String,
    pub name: String,
    /// The folder's name as a tag.
    pub tag: String,
    pub files: Vec<FolderFile>,
    /// Files past `max_files`, not offered.
    pub over_limit: usize,
    /// Flagged files past the rows shown, not offered.
    pub flagged_more: usize,
    /// Folders more than `max_depth` levels down, not entered.
    pub too_deep: usize,
    /// Hidden and system folders, not entered, such as `.git`.
    pub hidden_dirs: Vec<String>,
    /// The walk gave up before the end of the folder: more may be in there.
    pub stopped_early: bool,
    pub max_files: usize,
    pub max_depth: usize,
    /// The category the most common type would go to, or the filter's if it accepts that.
    pub category: Option<String>,
}

/// Whether a name is one the system keeps for itself.
fn is_system_name(name: &str) -> bool {
    const NAMES: [&str; 7] = ["desktop.ini", "thumbs.db", "ehthumbs.db", ".ds_store", "$recycle.bin", "system volume information", "__macosx"];
    NAMES.contains(&name.to_ascii_lowercase().as_str())
}

/// Whether the operating system marks this entry hidden or system.
#[cfg(windows)]
fn marked_hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

#[cfg(not(windows))]
fn marked_hidden(_meta: &std::fs::Metadata) -> bool {
    false
}

/// A folder as it is walked.
#[derive(Default)]
struct Walk {
    /// The largest file that may be added where the folder goes.
    limit: u64,
    files: Vec<FolderFile>,
    flagged: usize,
    over_limit: usize,
    flagged_more: usize,
    too_deep: usize,
    hidden_dirs: Vec<String>,
    entries: usize,
    stopped_early: bool,
}

impl Walk {
    fn push(&mut self, file: FolderFile) {
        if file.flag.is_empty() {
            if self.files.len() - self.flagged >= MAX_FILES {
                self.over_limit += 1;
                return;
            }
        } else {
            if self.flagged >= MAX_FLAGGED {
                self.flagged_more += 1;
                return;
            }
            self.flagged += 1;
        }
        self.files.push(file);
    }

    fn visit(&mut self, root: &Path, dir: &Path, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                self.stopped_early = true;
                return;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = || {
                let below = path.strip_prefix(root).unwrap_or(&path);
                below.to_string_lossy().replace('\\', "/")
            };
            let Ok(file_type) = entry.file_type() else { continue };
            // `DirEntry::metadata` does not follow links, so a link reads as itself.
            let Ok(meta) = entry.metadata() else { continue };
            let hidden = name.starts_with('.') || marked_hidden(&meta);
            if file_type.is_dir() {
                if hidden || is_system_name(&name) {
                    self.hidden_dirs.push(relative());
                } else if depth == 0 {
                    self.too_deep += 1;
                } else {
                    self.visit(root, &path, depth - 1);
                }
                continue;
            }
            let flag = if file_type.is_symlink() {
                "link"
            } else if looks_like_credentials(&name) {
                "credentials"
            } else if is_system_name(&name) {
                "system"
            } else if hidden {
                "hidden"
            } else if meta.len() > self.limit {
                "too_large"
            } else {
                ""
            };
            self.push(FolderFile {
                path: path.to_string_lossy().into_owned(),
                relative: relative(),
                ext: ext_of(&name),
                kind: FileType::of_name(&name).name().to_string(),
                size: if file_type.is_symlink() { 0 } else { meta.len() },
                flag: flag.to_string(),
                name,
            });
        }
    }
}

/// A folder's name as a tag: one word, within the limit.
fn folder_tag(name: &str) -> String {
    let word: String = name.trim().to_lowercase().chars().map(|c| if c.is_whitespace() { '-' } else { c }).collect();
    word.trim_start_matches('#').chars().take(MAX_TAG_CHARS).collect()
}

impl Engine {
    /// What a chosen folder holds, walked as a linked folder is, with what was left out and why. `filter` is the category
    /// the page shows. Its files become checkable.
    pub async fn scan_folder(&self, community: CommunityId, folder: PathBuf, filter: Option<CategoryId>) -> Result<FolderScan> {
        self.check_allowed(std::slice::from_ref(&folder))?;
        if !std::fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir()) {
            return Err(CoreError::Invalid(format!("{} is not a folder", folder.display())));
        }
        let root = folder.clone();
        let limit = self.upload_limit_in(Some(community));
        let walk = tokio::task::spawn_blocking(move || {
            let mut walk = Walk { limit, ..Walk::default() };
            walk.visit(&root, &root, MAX_DEPTH);
            walk
        })
        .await
        .map_err(|e| CoreError::Invalid(e.to_string()))?;
        self.allow_paths(walk.files.iter().filter(|f| f.flag != "link").map(|f| PathBuf::from(&f.path)));

        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for file in walk.files.iter().filter(|f| f.flag.is_empty()) {
            *counts.entry(file.kind.as_str()).or_default() += 1;
        }
        let common = counts.iter().max_by_key(|(_, n)| **n).and_then(|(kind, _)| FileType::parse(kind));
        let choices = self.category_choices(community);
        let category = common
            .and_then(|kind| choose_category(kind, &choices, filter))
            .or_else(|| filter.and_then(|f| choices.iter().find(|c| c.id == f)))
            .or(choices.first())
            .map(|c| c.id.to_string());
        let name = folder.file_name().map_or_else(|| folder.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        Ok(FolderScan {
            root: folder.to_string_lossy().into_owned(),
            tag: folder_tag(&name),
            name,
            files: walk.files,
            over_limit: walk.over_limit,
            flagged_more: walk.flagged_more,
            too_deep: walk.too_deep,
            hidden_dirs: walk.hidden_dirs,
            stopped_early: walk.stopped_early,
            max_files: MAX_FILES,
            max_depth: MAX_DEPTH,
            category,
        })
    }

    /// Lists files seeded from where they are, with no copy, as a linked folder's are. Each must have been checked.
    pub async fn list_files_in_place(&self, community: CommunityId, category: CategoryId, items: Vec<AddItem>, limit: SeedLimit) -> Result<Vec<String>> {
        self.with_tracker(community, |l| l.in_place.extend(items.iter().map(|i| PathBuf::from(&i.path))));
        self.list_files(community, category, items, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folders_name_becomes_one_tag() {
        assert_eq!(folder_tag("Holiday Photos 2026"), "holiday-photos-2026");
        assert_eq!(folder_tag(&"x".repeat(40)).chars().count(), MAX_TAG_CHARS);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-walk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, relative: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn walked(dir: &Path) -> Walk {
        let mut walk = Walk { limit: dissent_events::DEFAULT_FILE_SIZE_LIMIT, ..Walk::default() };
        walk.visit(dir, dir, MAX_DEPTH);
        walk
    }

    #[test]
    fn a_walk_flags_hidden_system_and_credential_files_and_skips_hidden_folders() {
        let dir = scratch("flags");
        for name in ["a.png", "sub/b.txt", ".hidden", "desktop.ini", "Thumbs.db", ".env", "keys/id_rsa", ".git/config", "c.pem"] {
            touch(&dir, name);
        }
        let walk = walked(&dir);
        let flag = |rel: &str| walk.files.iter().find(|f| f.relative == rel).map(|f| f.flag.as_str());
        assert_eq!(flag("a.png"), Some(""));
        assert_eq!(flag("sub/b.txt"), Some(""));
        assert_eq!(flag(".hidden"), Some("hidden"));
        assert_eq!(flag("desktop.ini"), Some("system"));
        assert_eq!(flag("Thumbs.db"), Some("system"));
        assert_eq!(flag(".env"), Some("credentials"));
        assert_eq!(flag("keys/id_rsa"), Some("credentials"));
        assert_eq!(flag("c.pem"), Some("credentials"));
        assert_eq!(flag(".git/config"), None, "a hidden folder is not entered");
        assert_eq!(walk.hidden_dirs, vec![".git"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_walk_counts_what_it_leaves_out() {
        let dir = scratch("limits");
        touch(&dir, "1/2/3/4/near.txt");
        touch(&dir, "1/2/3/4/5/far.txt");
        for i in 0..(MAX_FILES + 3) {
            touch(&dir, &format!("many/{i:04}.txt"));
        }
        let walk = walked(&dir);
        assert_eq!(walk.too_deep, 1, "the fifth level is not entered");
        assert!(!walk.files.iter().any(|f| f.name == "far.txt"));
        assert_eq!(walk.files.iter().filter(|f| f.flag.is_empty()).count(), MAX_FILES);
        assert_eq!(walk.over_limit, 4, "the files past the limit are counted: 503 + near.txt - 500");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_walk_does_not_follow_a_link() {
        let dir = scratch("links");
        let outside = scratch("links-outside");
        touch(&outside, "secret.txt");
        std::os::unix::fs::symlink(&outside, dir.join("folder-link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("file-link")).unwrap();
        let walk = walked(&dir);
        assert_eq!(walk.files.len(), 2);
        assert!(walk.files.iter().all(|f| f.flag == "link"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
