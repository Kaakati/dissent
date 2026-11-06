//! A folder as one listing: its files, frozen at listing time in a manifest, seeded where they are, and downloaded whole.
//! See `docs/design/14-file-tracker.md`, "Folder listings".
//!
//! A folder listing is a `FileListing` event whose sealed body is a [`FolderBody`] (`BodyKind::Folder`), so a release
//! that predates it fails to open the body and counts the listing as locked. The manifest is too large for a sealed field,
//! so it is a blob of its own, encrypted like a listed file under a key derived from its fingerprint, and named in the
//! body. Members fetch it through the blob gate before they show the contents. Each file in it is a blob of its own too,
//! so a file's holders, gate scope and seeding work as for a single listing (`Engine::opened_blobs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dissent_events::{CategoryId, ChannelId, EventId, Payload, Perms};
use dissent_files::{FileRef, Input, MAX_FILE_SIZE};
use dissent_identity::{CommunityId, RootId};
use serde::{Deserialize, Serialize};

use super::bodies::{clean_description, clean_tags, open, seal, BodyKind, FileType, ListingBody, ListingUpdateBody};
use super::catalog::{ext_of, ListingView, Opened};
use super::{SeedLimit, SeedRecord};
use crate::attachments::Transfer;
use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::views::EngineEvent;

/// The prefix of a folder download's `Transfer::message_id`: `folder:<community>:<listing>`.
pub(crate) const FOLDER_PREFIX: &str = "folder:";
/// Most files one folder listing holds, as for a folder added file by file.
pub(crate) const MAX_FOLDER_FILES: usize = super::folders::MAX_FILES;
/// Most parts in a path below the folder: the levels a folder is walked, and the file.
const MAX_PARTS: usize = super::folders::MAX_DEPTH + 2;
/// Longest path below the folder, so the folder still fits where it lands on Windows.
const MAX_PATH_CHARS: usize = 200;
/// Longest name of one file or folder in it.
const MAX_PART_CHARS: usize = 120;
/// How long a folder download's staged files wait for a resume before they are cleared at startup.
const STAGING_KEPT: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);
/// The start of a staging folder's name, in the download folder.
const STAGING_PREFIX: &str = ".dissent-partial-";
/// The file Dissent writes into a staging folder when it makes one. Only a folder with the prefix and this file, holding
/// exactly `STAGING_MAGIC`, is ever treated as staging: reused, swept or deleted.
const STAGING_MARKER: &str = ".dissent-staging";
const STAGING_MAGIC: &[u8] = b"dissent/folder-staging/v1\n";
/// Room left on the disk beyond what a download needs.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// One file of a folder listing, as it was when the folder was listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Below the folder, with `/` between parts.
    pub path: String,
    pub size: u64,
    pub kind: FileType,
    pub fingerprint: [u8; 32],
    pub blob: [u8; 32],
    pub cipher_size: u64,
}

/// What a folder listing holds. Postcard: its field order is its format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

/// The sealed fields of a folder listing (`BodyKind::Folder`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderBody {
    pub name: String,
    pub files: u32,
    /// Plaintext bytes of every file.
    pub size: u64,
    pub manifest_fingerprint: [u8; 32],
    pub manifest_blob: [u8; 32],
    pub manifest_size: u64,
    pub manifest_cipher_size: u64,
    /// The key generation of the manifest and of every file.
    pub content_generation: u64,
    pub tags: Vec<String>,
    pub description: String,
}

/// A folder listing with its opened body.
#[derive(Clone, Debug)]
pub(crate) struct OpenedFolder {
    pub id: EventId,
    pub category: CategoryId,
    pub lister: RootId,
    pub added_ms: u64,
    pub body: FolderBody,
}

/// The extra a listing view carries for a folder.
#[derive(Clone, Debug, Serialize)]
pub struct FolderInfo {
    pub files: u32,
    /// Whether this device has the manifest, so the contents show without asking anyone.
    pub contents_known: bool,
    /// Files that can run code, once the contents are known.
    pub dangerous: u32,
}

/// One file in a folder's contents, as the contents panel shows it.
#[derive(Clone, Debug, Serialize)]
pub struct FolderEntryView {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: u64,
    pub danger: Option<dissent_files::Danger>,
    /// Members known to hold this file whole.
    pub holders: u32,
}

/// What a folder listing holds.
#[derive(Clone, Debug, Serialize)]
pub struct FolderContentsView {
    pub listing: String,
    pub name: String,
    pub files: u32,
    pub size: u64,
    pub dangerous: u32,
    /// Files no member is known to hold whole right now.
    pub unavailable: u32,
    pub entries: Vec<FolderEntryView>,
}

/// One file of a folder being downloaded.
#[derive(Clone, Debug, Serialize)]
pub struct FolderFileProgress {
    pub path: String,
    pub size: u64,
    /// 0 to 1.
    pub progress: f64,
    pub done: bool,
}

/// Fails unless `part` is a name that is safe to create on every platform this app runs on.
pub(crate) fn check_part(part: &str) -> Result<()> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let bad = |why: &str| Err(CoreError::Invalid(format!("unsafe name {part:?} in a folder: {why}")));
    if part.is_empty() || part == "." || part == ".." {
        return bad("empty or a parent reference");
    }
    if part.chars().count() > MAX_PART_CHARS {
        return bad("too long");
    }
    if part.chars().any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/')) {
        return bad("a character Windows does not allow");
    }
    if part.ends_with('.') || part.ends_with(' ') || part.starts_with(' ') {
        return bad("a leading or trailing space or dot");
    }
    let stem = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return bad("a name Windows reserves");
    }
    Ok(())
}

/// The parts of a path below a folder, or why it is not safe: absolute, a parent reference, a drive, a backslash, too
/// deep or too long.
pub(crate) fn check_relative(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.chars().count() > MAX_PATH_CHARS {
        return Err(CoreError::Invalid(format!("unsafe path {path:?} in a folder")));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_PARTS {
        return Err(CoreError::Invalid(format!("path {path:?} is too deep")));
    }
    for part in &parts {
        check_part(part)?;
    }
    Ok(parts)
}

/// Fails unless every path in `manifest` is safe, no two would land on the same file on a case-insensitive disk, no file
/// is also a folder of another, and the sizes agree.
pub fn check_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.entries.is_empty() || manifest.entries.len() > MAX_FOLDER_FILES {
        return Err(CoreError::Invalid(format!("a folder holds 1 to {MAX_FOLDER_FILES} files")));
    }
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for e in &manifest.entries {
        let parts = check_relative(&e.path)?;
        if e.size > MAX_FILE_SIZE || e.cipher_size != dissent_files::cipher_len(e.size) {
            return Err(CoreError::Invalid(format!("{} has sizes that do not agree", e.path)));
        }
        let lower: Vec<String> = parts.iter().map(|p| p.to_lowercase()).collect();
        for i in 1..lower.len() {
            dirs.insert(lower[..i].join("/"));
        }
        if !files.insert(lower.join("/")) {
            return Err(CoreError::Invalid(format!("{} clashes with another file whose name differs only in case", e.path)));
        }
    }
    if let Some(both) = files.intersection(&dirs).next() {
        return Err(CoreError::Invalid(format!("{both} is both a file and a folder")));
    }
    Ok(())
}

/// The first of `name`, `name (2)`, `name (3)`, ... that is free in `dir`.
fn free_dir(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{name} ({n})")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or(first)
}

/// Whether `path` is a staging folder Dissent made: a real folder, not a link, whose name has the prefix, holding the
/// marker as a real file with exactly the magic bytes.
fn is_staging(path: &Path) -> bool {
    let named = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(STAGING_PREFIX));
    let folder = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    let marker = path.join(STAGING_MARKER);
    let marked = std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() == STAGING_MAGIC.len() as u64)
        && std::fs::read(&marker).is_ok_and(|b| b == STAGING_MAGIC);
    named && folder && marked
}

/// Makes the staging folder at `path`, or reuses one Dissent made there. Anything else at that path is left alone and
/// refused. The marker is written again on reuse, so its age is the time since the download last ran.
fn open_staging(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)?;
        }
        Err(e) => return Err(e.into()),
        Ok(_) if is_staging(path) => {}
        Ok(_) => {
            return Err(CoreError::Invalid(format!("{} is in the way and was not made by Dissent, so it is left alone", path.display())));
        }
    }
    std::fs::write(path.join(STAGING_MARKER), STAGING_MAGIC)?;
    Ok(())
}

/// Deletes a staging folder Dissent made, and nothing else. `remove_dir_all` removes a link inside it as a link, without
/// following it. Returns whether it deleted.
fn remove_staging(path: &Path) -> bool {
    if !is_staging(path) {
        return false;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not clear a folder download's staging");
            false
        }
    }
}

/// Deletes the staging folders in `dir` whose marker is older than `max_age`. Returns how many.
fn sweep_staging(dir: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_staging(&path) {
            continue;
        }
        let age = std::fs::symlink_metadata(path.join(STAGING_MARKER)).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
        if age.is_some_and(|age| age >= max_age) && remove_staging(&path) {
            removed += 1;
        }
    }
    removed
}

/// Fails if anything between `root` and `target` (exclusive of `root`) exists as a link. A resumed download must not write
/// through a link planted in its staging folder.
fn no_links_below(root: &Path, target: &Path) -> Result<()> {
    let Ok(below) = target.strip_prefix(root) else {
        return Err(CoreError::Invalid("a folder file landed outside its folder".into()));
    };
    let mut at = root.to_path_buf();
    for part in below.components() {
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(CoreError::Invalid(format!("{} is a link, so nothing is written through it", at.display())));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8; 32]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn manifests_path(paths: &crate::store::Paths, id: &CommunityId) -> PathBuf {
    paths.community_dir(id).join("folder_manifests.bin")
}

impl Engine {
    /// The manifests this device knows, by manifest blob. Loaded once, sealed like every store file.
    fn known_manifests(&self, community: CommunityId) -> BTreeMap<[u8; 32], Manifest> {
        let paths = self.read().paths.clone();
        self.with_tracker(community, |l| {
            l.manifests
                .get_or_insert_with(|| paths.read_bin::<BTreeMap<[u8; 32], Manifest>>(&manifests_path(&paths, &community)).ok().flatten().unwrap_or_default())
                .clone()
        })
    }

    fn remember_manifest(&self, community: CommunityId, blob: [u8; 32], manifest: Manifest) {
        let mut all = self.known_manifests(community);
        all.insert(blob, manifest);
        let paths = self.read().paths.clone();
        if let Err(e) = paths.write_bin(&manifests_path(&paths, &community), &all) {
            tracing::warn!(community = %community, error = %e, "could not save a folder's contents");
        }
        self.with_tracker(community, |l| l.manifests = Some(all));
    }

    /// Every folder listing of `community` this device can open and its person may see, oldest first.
    pub(crate) fn opened_folders(&self, community: CommunityId) -> Vec<OpenedFolder> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let w = self.read();
        let Some(c) = w.communities.get(&community) else { return Vec::new() };
        let st = c.state();
        let mut out: Vec<OpenedFolder> = st
            .files
            .listings
            .iter()
            .filter(|(_, l)| st.may_see_files(&w.me, &l.category))
            .filter_map(|(id, l)| {
                let key = keys.get(&l.inner.generation)?;
                let mut body: FolderBody = open(key, &community, &l.lister, BodyKind::Folder, &l.inner)?;
                if let Some((_, update)) = &l.update {
                    if let Some(u) = keys
                        .get(&update.generation)
                        .and_then(|k| open::<ListingUpdateBody>(k, &community, &l.lister, BodyKind::ListingUpdate, update))
                    {
                        body.tags = u.tags;
                        body.description = u.description;
                    }
                }
                // As for a file: what replay checked must be what the body says.
                if l.blob.is_some_and(|(hash, size)| hash != body.manifest_blob || size != body.manifest_cipher_size)
                    || body.manifest_cipher_size != dissent_files::cipher_len(body.manifest_size)
                {
                    return None;
                }
                Some(OpenedFolder { id: *id, category: l.category, lister: l.lister, added_ms: l.hlc.wall_ms, body })
            })
            .collect();
        out.sort_by_key(|o| (o.added_ms, o.id));
        out
    }

    pub(crate) fn opened_folders_count(&self, community: CommunityId) -> usize {
        self.opened_folders(community).len()
    }

    /// Every blob a listing of `community` names, as listings: single files, and each file and the manifest of every
    /// folder whose manifest this device knows. What sizes, scopes, holdings and seeding go by.
    pub(crate) fn opened_blobs(&self, community: CommunityId) -> Vec<Opened> {
        let mut out = self.opened_listings(community);
        let manifests = self.known_manifests(community);
        for f in self.opened_folders(community) {
            let as_file = |name: &str, kind, size, fingerprint, blob, cipher_size| Opened {
                id: f.id,
                category: f.category,
                lister: f.lister,
                added_ms: f.added_ms,
                body: ListingBody {
                    name: name.to_string(),
                    mime: mime_guess::from_path(name).first_or_octet_stream().to_string(),
                    kind,
                    size,
                    fingerprint,
                    content_generation: f.body.content_generation,
                    blob,
                    cipher_size,
                    tags: f.body.tags.clone(),
                    description: String::new(),
                },
            };
            out.push(as_file(
                "manifest.bin",
                FileType::Other,
                f.body.manifest_size,
                f.body.manifest_fingerprint,
                f.body.manifest_blob,
                f.body.manifest_cipher_size,
            ));
            if let Some(m) = manifests.get(&f.body.manifest_blob) {
                for e in &m.entries {
                    out.push(as_file(leaf(&e.path), e.kind, e.size, e.fingerprint, e.blob, e.cipher_size));
                }
            }
        }
        out
    }

    /// Lists the folder at `root` as one listing in `category`: `files` (below `root`, `/` between parts) each checked
    /// beforehand, and seeded from where they are. Returns the listing id.
    #[allow(clippy::too_many_arguments)] // the dialog's fields, as it sends them
    pub async fn list_folder(
        &self,
        community: CommunityId,
        category: CategoryId,
        root: PathBuf,
        files: Vec<String>,
        tags: Vec<String>,
        description: String,
        limit: SeedLimit,
    ) -> Result<String> {
        let me = self.inner.me;
        let types = self.category_types(community, category)?;
        let restricted = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
            let st = c.state();
            if !st.may_add_files(&me, &category, now_ms()) {
                return Err(CoreError::Rejected("NoPermission".into()));
            }
            st.files.categories.get(&category).is_some_and(|s| s.visible.is_some())
        };
        let tags = clean_tags(&tags)?;
        let description = clean_description(&description)?;
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| check_part(n).is_ok()).unwrap_or_else(|| "Folder".into());
        if files.is_empty() || files.len() > MAX_FOLDER_FILES {
            return Err(CoreError::Invalid(format!("a folder holds 1 to {MAX_FOLDER_FILES} files")));
        }
        let (generation, files_key) = if restricted {
            self.newest_category_key(community, category).await?
        } else {
            self.newest_files_key(community, false).await?
        };
        // Links made for this listing and for no seed before it. If listing fails, they are undone, so nothing stays served
        // without a seed record.
        let seeded_before: BTreeSet<[u8; 32]> = self.with_tracker(community, |l| l.book.seeds.keys().copied().collect());
        let mut made: Vec<[u8; 32]> = Vec::new();
        let result = self
            .list_folder_linked(community, category, &root, &files, &types, restricted, (generation, files_key), name, tags, description, limit, &seeded_before, &mut made)
            .await;
        if result.is_err() {
            for blob in made {
                self.inner.files.forget_linked(blob);
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)] // `list_folder`'s own state, so a failure can undo what it made
    async fn list_folder_linked(
        &self,
        community: CommunityId,
        category: CategoryId,
        root: &Path,
        files: &[String],
        types: &[FileType],
        restricted: bool,
        (generation, files_key): (u64, [u8; 32]),
        name: String,
        tags: Vec<String>,
        description: String,
        limit: SeedLimit,
        seeded_before: &BTreeSet<[u8; 32]>,
        made: &mut Vec<[u8; 32]>,
    ) -> Result<String> {
        let me = self.inner.me;
        let mut entries = Vec::new();
        let mut seeds = Vec::new();
        for rel in files {
            let parts = check_relative(rel)?;
            let path = parts.iter().fold(root.to_path_buf(), |p, part| p.join(part));
            let offered = self.with_tracker(community, |l| l.offered.contains(&path));
            if !offered || !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                return Err(CoreError::Invalid(format!("{} was not checked for adding", path.display())));
            }
            let kind = FileType::of_name(leaf(rel));
            if !types.is_empty() && !types.contains(&kind) {
                return Err(CoreError::Invalid(format!("{rel}: {} files are not allowed in this category", kind.name())));
            }
            self.check_upload(Some(community), rel, std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))?;
            let (fingerprint, size) = self.fingerprint_of(community, &path).await?;
            let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
            let key = dissent_files::tracker::content_key(&files_key, &fingerprint);
            let fref = self.inner.files.link_file(&path, &mime, key, fingerprint).await.map_err(|e| CoreError::Invalid(e.to_string()))?;
            if !seeded_before.contains(&fref.hash) {
                made.push(fref.hash);
            }
            entries.push(ManifestEntry { path: rel.clone(), size, kind, fingerprint, blob: fref.hash, cipher_size: fref.cipher_size });
            #[cfg(feature = "test-hooks")]
            if self.inner.fail_folder_after.load(std::sync::atomic::Ordering::Relaxed) == entries.len() as u64 {
                return Err(CoreError::Invalid("failed on purpose, for a test".into()));
            }
            seeds.push((fref.hash, path));
        }
        let manifest = Manifest { entries };
        check_manifest(&manifest)?;
        let bytes = postcard::to_stdvec(&manifest).map_err(|e| CoreError::Encoding(e.to_string()))?;
        let manifest_fingerprint = *blake3::hash(&bytes).as_bytes();
        let manifest_size = bytes.len() as u64;
        let key = dissent_files::tracker::content_key(&files_key, &manifest_fingerprint);
        let input = Input::Bytes { name: "manifest.bin".into(), mime: "application/octet-stream".into(), data: bytes };
        let mref = self.inner.files.add_with_key(input, key, manifest_fingerprint).await.map_err(|e| CoreError::Invalid(e.to_string()))?;
        let body = FolderBody {
            name,
            files: manifest.entries.len() as u32,
            size: manifest.entries.iter().map(|e| e.size).sum(),
            manifest_fingerprint,
            manifest_blob: mref.hash,
            manifest_size,
            manifest_cipher_size: mref.cipher_size,
            content_generation: generation,
            tags,
            description,
        };
        let inner = seal(&files_key, generation, &community, &me, BodyKind::Folder, &body)?;
        // A restricted category's blobs stay sealed, as for a file.
        let blob = (!restricted).then_some((mref.hash, mref.cipher_size));
        let published = self.author_publish(community, ChannelId::CONTROL, Payload::FileListing { category, blob, inner }).await;
        if published.is_err() {
            let _ = self.inner.files.unpin(mref.hash).await;
        }
        let listing = EventId(published?.event.id.0);
        self.remember_manifest(community, mref.hash, manifest);
        let record = |path: Option<PathBuf>, linked: bool| SeedRecord {
            listing,
            modified_ms: path.as_deref().filter(|_| linked).and_then(super::modified_ms),
            path,
            limit,
            paused: false,
            stopped: false,
            linked,
            uploaded: 0,
            since_ms: now_ms(),
            downloaded: false,
        };
        self.with_tracker(community, |l| {
            l.book.seeds.entry(mref.hash).or_insert_with(|| record(None, false));
            for (blob, path) in seeds {
                l.book.seeds.entry(blob).or_insert_with(|| record(Some(path), true));
            }
        });
        self.save_tracker_book(community);
        self.reconcile_linked(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(listing.to_string())
    }

    /// A folder's manifest: from this device, or fetched from a member who holds it, and checked against the fingerprint
    /// its listing names before it is believed.
    pub(crate) async fn folder_manifest(&self, community: CommunityId, f: &OpenedFolder) -> Result<Manifest> {
        if let Some(m) = self.known_manifests(community).remove(&f.body.manifest_blob) {
            return Ok(m);
        }
        let files_key = self
            .files_key(community, f.body.content_generation)
            .ok_or_else(|| CoreError::Invalid("this device lacks the key that folder was listed under".into()))?;
        let key = dissent_files::tracker::content_key(&files_key, &f.body.manifest_fingerprint);
        let fref = FileRef {
            hash: f.body.manifest_blob,
            size: f.body.manifest_size,
            cipher_size: f.body.manifest_cipher_size,
            name: "manifest.bin".into(),
            mime: "application/octet-stream".into(),
            key: key.key,
            nonce_prefix: key.nonce_prefix,
        };
        let from = self.folder_providers(community, f, f.body.manifest_blob);
        let dir = self.read().paths.community_dir(&community).join("folder-contents");
        let node = self.node();
        let done = self
            .inner
            .files
            .fetch_swarm(node.endpoint(), &from, &fref, &dir, |_, _| {}, |_| {})
            .await
            .map_err(|e| CoreError::Invalid(format!("could not get the folder's contents: {e}")))?;
        let bytes = std::fs::read(&done.path);
        let _ = std::fs::remove_file(&done.path);
        let bytes = bytes?;
        if *blake3::hash(&bytes).as_bytes() != f.body.manifest_fingerprint {
            return Err(CoreError::Invalid("the folder's contents do not match its listing".into()));
        }
        let manifest: Manifest = postcard::from_bytes(&bytes).map_err(|e| CoreError::Encoding(e.to_string()))?;
        check_manifest(&manifest)?;
        if manifest.entries.len() as u32 != f.body.files || manifest.entries.iter().map(|e| e.size).sum::<u64>() != f.body.size {
            return Err(CoreError::Invalid("the folder's contents do not match its listing".into()));
        }
        self.remember_manifest(community, f.body.manifest_blob, manifest.clone());
        // The gate now knows the folder's files, so their pieces can pass through this device later.
        self.reconcile_linked(community);
        Ok(manifest)
    }

    /// Devices to fetch `blob` of a folder from: whoever said they hold it, then the lister's devices.
    fn folder_providers(&self, community: CommunityId, f: &OpenedFolder, blob: [u8; 32]) -> Vec<iroh::EndpointAddr> {
        self.folder_provider_peers(community, f, blob).into_iter().filter_map(|p| p.to_iroh().ok()).collect()
    }

    /// Who may hold `blob` of folder `f`: whole holders first, then partial ones, then the lister.
    fn folder_provider_peers(&self, community: CommunityId, f: &OpenedFolder, blob: [u8; 32]) -> Vec<dissent_net::PeerAddr> {
        let sizes = BTreeMap::from([(blob, 0u64)]);
        let swarm = self.swarms(community, &sizes).remove(&blob).unwrap_or_default();
        let mut providers: Vec<dissent_net::PeerAddr> = {
            let w = self.read();
            let peers = w.addresses.peers();
            swarm
                .devices
                .iter()
                .chain(&swarm.partial)
                .filter(|d| **d != w.device)
                .filter_map(|d| peers.iter().find(|p| p.id == *d).cloned())
                .collect()
        };
        if f.lister != self.inner.me {
            for p in self.providers_for(&[f.lister]) {
                if !providers.iter().any(|x| x.id == p.id) {
                    providers.push(p);
                }
            }
        }
        providers
    }

    /// What a folder listing holds, with who holds each file and which can run code.
    pub async fn folder_contents(&self, community: CommunityId, listing: EventId) -> Result<FolderContentsView> {
        let f = self
            .opened_folders(community)
            .into_iter()
            .find(|f| f.id == listing)
            .ok_or_else(|| CoreError::NotFound(format!("folder {listing}")))?;
        let manifest = self.folder_manifest(community, &f).await?;
        let sizes: BTreeMap<[u8; 32], u64> = manifest.entries.iter().map(|e| (e.blob, e.cipher_size)).collect();
        let swarms = self.swarms(community, &sizes);
        let heard: BTreeSet<RootId> = self.with_tracker(community, |l| l.heard.values().filter_map(|h| h.root).collect());
        let mine: BTreeSet<[u8; 32]> = self.with_tracker(community, |l| l.book.seeds.keys().copied().collect());
        let entries: Vec<FolderEntryView> = manifest
            .entries
            .iter()
            .map(|e| {
                let mut holders = swarms.get(&e.blob).map(|s| s.seeders.clone()).unwrap_or_default();
                if !heard.contains(&f.lister) {
                    holders.insert(f.lister);
                }
                if mine.contains(&e.blob) {
                    holders.insert(self.inner.me);
                }
                let name = leaf(&e.path).to_string();
                FolderEntryView {
                    path: e.path.clone(),
                    ext: ext_of(&name),
                    kind: e.kind.name().to_string(),
                    danger: dissent_files::danger_of_name(&name, ""),
                    name,
                    size: e.size,
                    holders: holders.len() as u32,
                }
            })
            .collect();
        Ok(FolderContentsView {
            listing: listing.to_string(),
            name: f.body.name.clone(),
            files: f.body.files,
            size: f.body.size,
            dangerous: entries.iter().filter(|e| e.danger.is_some()).count() as u32,
            unavailable: entries.iter().filter(|e| e.holders == 0).count() as u32,
            entries,
        })
    }

    /// The listing views of `community`'s folders, for the Files page. `swarms_of` gives each blob's holders.
    pub(crate) fn folder_views(&self, community: CommunityId) -> Vec<ListingView> {
        let (limits, server) = (self.size_limits(), self.server_file_size_limit(community));
        let folders = self.opened_folders(community);
        if folders.is_empty() {
            return Vec::new();
        }
        let manifests = self.known_manifests(community);
        let sizes: BTreeMap<[u8; 32], u64> = folders
            .iter()
            .flat_map(|f| {
                let files = manifests.get(&f.body.manifest_blob).map(|m| m.entries.iter().map(|e| (e.blob, e.cipher_size)).collect::<Vec<_>>()).unwrap_or_default();
                files.into_iter().chain([(f.body.manifest_blob, f.body.manifest_cipher_size)])
            })
            .collect();
        let swarms = self.swarms(community, &sizes);
        let states = self.seed_states(community);
        let heard: BTreeSet<RootId> = self.with_tracker(community, |l| l.heard.values().filter_map(|h| h.root).collect());
        let transfers: BTreeMap<String, (String, f64, Option<PathBuf>)> = {
            let w = self.read();
            w.attach
                .transfers
                .values()
                .filter(|t| t.message_id.starts_with(FOLDER_PREFIX))
                .map(|t| (t.message_id.clone(), (t.state.clone(), t.progress, t.local_path.clone())))
                .collect()
        };
        let w = self.read();
        let me = w.me;
        let manage = w.communities.get(&community).is_some_and(|c| c.state().perms_of(&me).contains(Perms::MANAGE_FILES));
        folders
            .iter()
            .map(|f| {
                let entries = manifests.get(&f.body.manifest_blob).map(|m| m.entries.as_slice()).unwrap_or(&[]);
                // A holder of the folder holds every file of it whole, or, while its contents are unknown here, its manifest.
                let mut seeders: BTreeSet<RootId> = match entries.split_first() {
                    Some((first, rest)) => {
                        let mut all = swarms.get(&first.blob).map(|s| s.seeders.clone()).unwrap_or_default();
                        for e in rest {
                            let s = swarms.get(&e.blob).map(|s| s.seeders.clone()).unwrap_or_default();
                            all.retain(|r| s.contains(r));
                        }
                        all
                    }
                    None => swarms.get(&f.body.manifest_blob).map(|s| s.seeders.clone()).unwrap_or_default(),
                };
                if !heard.contains(&f.lister) {
                    seeders.insert(f.lister);
                }
                let seeding_here = !entries.is_empty() && entries.iter().all(|e| states.get(&e.blob) == Some(&super::seeding::SeedState::Seeding));
                let held_here = !entries.is_empty() && entries.iter().all(|e| states.contains_key(&e.blob));
                if seeding_here {
                    seeders.insert(me);
                } else {
                    seeders.remove(&me);
                }
                let leechers: BTreeSet<RootId> = entries
                    .iter()
                    .flat_map(|e| swarms.get(&e.blob).map(|s| s.leechers.clone()).unwrap_or_default())
                    .filter(|r| !seeders.contains(r) && *r != me)
                    .collect();
                let wanted = format!("{FOLDER_PREFIX}{community}:{}", f.id);
                let (local, progress, local_path) = match transfers.get(&wanted) {
                    Some((state, progress, path)) if state == "active" || state == "waiting" => ("downloading".to_string(), *progress, path.clone()),
                    Some((state, _, path)) if state == "failed" => ("failed".to_string(), 0.0, path.clone()),
                    _ if seeding_here => ("seeding".to_string(), 1.0, None),
                    _ if held_here => ("paused".to_string(), 1.0, None),
                    Some((_, _, path)) => ("downloaded".to_string(), 1.0, path.clone()),
                    None => ("none".to_string(), 0.0, None),
                };
                let dangerous = entries.iter().filter(|e| dissent_files::danger_of_name(leaf(&e.path), "").is_some()).count() as u32;
                let cipher: u64 = entries.iter().map(|e| e.cipher_size).sum();
                ListingView {
                    // A folder is over a limit when one of its files is: limits are per file.
                    over_limit: crate::limits::over_limit_note(&limits, server, entries.iter().map(|e| e.size).max().unwrap_or(0)),
                    id: f.id.to_string(),
                    category: f.category.to_string(),
                    name: f.body.name.clone(),
                    ext: "DIR".into(),
                    kind: "Folder".into(),
                    mime: "inode/directory".into(),
                    size: f.body.size,
                    cipher_size: cipher,
                    pieces: entries.iter().map(|e| dissent_files::pieces_of(e.cipher_size)).sum(),
                    fingerprint: hex(&f.body.manifest_fingerprint),
                    tags: f.body.tags.clone(),
                    description: f.body.description.clone(),
                    lister: f.lister.to_string(),
                    lister_name: w.display_name(&f.lister),
                    added_ms: f.added_ms,
                    also_listed_by: Vec::new(),
                    seeders: seeders.len() as u32,
                    leechers: leechers.len() as u32,
                    completed: 0,
                    mine: f.lister == me,
                    local,
                    local_path: local_path.map(|p| p.to_string_lossy().into_owned()),
                    danger: entries.iter().find_map(|e| dissent_files::danger_of_name(leaf(&e.path), "")),
                    progress,
                    can_edit: f.lister == me || manage,
                    folder: Some(FolderInfo { files: f.body.files, contents_known: !entries.is_empty(), dangerous }),
                }
            })
            .collect()
    }

    /// The blobs a folder listing names: its files, if known here, and its manifest.
    pub(crate) fn folder_blobs(&self, community: CommunityId, listing: EventId) -> Vec<[u8; 32]> {
        let Some(f) = self.opened_folders(community).into_iter().find(|f| f.id == listing) else { return Vec::new() };
        let mut out: Vec<[u8; 32]> = self
            .known_manifests(community)
            .get(&f.body.manifest_blob)
            .map(|m| m.entries.iter().map(|e| e.blob).collect())
            .unwrap_or_default();
        out.push(f.body.manifest_blob);
        out
    }

    /// Starts downloading a whole folder into the download folder, under its name. It shows in Transfers as one entry.
    pub async fn download_folder(&self, community: CommunityId, listing: EventId) -> Result<crate::attachments::TransferView> {
        self.download_folder_with(community, listing, false).await
    }

    /// `download_folder`, past this device's download limit when `anyway` is set: the person chose to.
    pub async fn download_folder_with(&self, community: CommunityId, listing: EventId, anyway: bool) -> Result<crate::attachments::TransferView> {
        let f = self
            .opened_folders(community)
            .into_iter()
            .find(|f| f.id == listing)
            .ok_or_else(|| CoreError::NotFound(format!("folder {listing}")))?;
        let manifest = self.folder_manifest(community, &f).await?;
        check_manifest(&manifest)?;
        self.check_download(manifest.entries.iter().map(|e| e.size).max().unwrap_or(0), anyway)?;
        let total: u64 = manifest.entries.iter().map(|e| e.size).sum();
        let cipher: u64 = manifest.entries.iter().map(|e| e.cipher_size).sum();
        // The whole folder up front: its plaintext where it lands, and its ciphertext and one file's copy in the store.
        let dest = self.download_dir();
        let largest = manifest.entries.iter().map(|e| e.cipher_size + e.size).max().unwrap_or(0);
        let probe = if dest.exists() { dest.clone() } else { dest.ancestors().find(|p| p.exists()).map(Path::to_path_buf).unwrap_or(dest.clone()) };
        let free = dissent_files::available_space(&probe).map_err(CoreError::Storage)?;
        if free < total + SPACE_MARGIN {
            return Err(CoreError::Invalid(format!("Not enough disk space for this folder: it needs {total} bytes and {free} are free.")));
        }
        self.inner.files.ensure_space(cipher + largest).await.map_err(|e| CoreError::Invalid(e.to_string()))?;
        let id = format!("dir{}", data_encoding::BASE32_NOPAD.encode(&f.body.manifest_blob).to_lowercase());
        {
            let mut w = self.write();
            let busy = w.attach.transfers.get(&id).is_some_and(|t| t.state == "active" || t.state == "waiting");
            if !busy {
                w.attach.transfers.insert(
                    id.clone(),
                    Transfer {
                        id: id.clone(),
                        message_id: format!("{FOLDER_PREFIX}{community}:{listing}"),
                        direction: "download".into(),
                        file: FileRef {
                            hash: f.body.manifest_blob,
                            size: total,
                            cipher_size: cipher,
                            name: f.body.name.clone(),
                            mime: "inode/directory".into(),
                            key: [0; 32],
                            nonce_prefix: [0; 8],
                        },
                        providers: Vec::new(),
                        state: "active".into(),
                        progress: 0.0,
                        started_ms: now_ms(),
                        local_path: None,
                        error: None,
                    },
                );
            }
        }
        self.persist_attach();
        self.start_download(&id);
        self.emit(EngineEvent::TransfersChanged);
        self.transfer_view(&id)
    }

    /// Each file of a folder download, while it runs or after.
    pub fn folder_download_files(&self, id: &str) -> Vec<FolderFileProgress> {
        let state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.folder_progress.get(id).cloned().unwrap_or_default()
    }

    /// `folder_download_files` by the folder listing.
    pub fn folder_download_files_of(&self, community: CommunityId, listing: EventId) -> Vec<FolderFileProgress> {
        let wanted = format!("{FOLDER_PREFIX}{community}:{listing}");
        let id = self.read().attach.transfers.values().find(|t| t.message_id == wanted).map(|t| t.id.clone());
        id.map(|id| self.folder_download_files(&id)).unwrap_or_default()
    }

    fn set_folder_progress(&self, id: &str, files: Vec<FolderFileProgress>) {
        let mut state = self.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.folder_progress.insert(id.to_string(), files);
    }

    /// Runs a folder download: each file into a staging folder beside where the folder lands, checked against its
    /// fingerprint, then the staging folder moved into place. A file already staged and whole is not fetched again, so a
    /// retry resumes. Called from `start_download`.
    pub(crate) async fn run_folder_download(&self, id: &str) {
        if let Err(e) = self.folder_download_inner(id).await {
            self.fail_transfer(id, "failed", e.to_string().trim_start_matches("invalid: ").to_string());
        }
    }

    async fn folder_download_inner(&self, id: &str) -> Result<()> {
        let message_id = self.read().attach.transfers.get(id).map(|t| t.message_id.clone()).unwrap_or_default();
        let (community, listing) = message_id
            .strip_prefix(FOLDER_PREFIX)
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(c, l)| Some((c.parse::<CommunityId>().ok()?, l.parse::<EventId>().ok()?)))
            .ok_or_else(|| CoreError::Invalid("not a folder download".into()))?;
        let f = self
            .opened_folders(community)
            .into_iter()
            .find(|f| f.id == listing)
            .ok_or_else(|| CoreError::NotFound("the folder is no longer listed".into()))?;
        let manifest = self.folder_manifest(community, &f).await?;
        check_manifest(&manifest)?;
        let files_key = self
            .files_key(community, f.body.content_generation)
            .ok_or_else(|| CoreError::Invalid("this device lacks the key that folder was listed under".into()))?;
        let dest = self.download_dir();
        let staging = self.folder_staging(id);
        std::fs::create_dir_all(&dest)?;
        open_staging(&staging)?;
        let mut progress: Vec<FolderFileProgress> =
            manifest.entries.iter().map(|e| FolderFileProgress { path: e.path.clone(), size: e.size, progress: 0.0, done: false }).collect();
        let total = manifest.entries.iter().map(|e| e.size).sum::<u64>().max(1);
        let node = self.node();
        for (i, e) in manifest.entries.iter().enumerate() {
            let parts = check_relative(&e.path)?;
            let target = parts.iter().fold(staging.clone(), |p, part| p.join(part));
            no_links_below(&staging, &target)?;
            let staged = std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_file() && m.len() == e.size) && {
                let owned = target.clone();
                let fp = tokio::task::spawn_blocking(move || dissent_files::tracker::fingerprint_file(&owned, |_| true)).await;
                matches!(fp, Ok(Ok((fp, _))) if fp == e.fingerprint)
            };
            if !staged {
                let _ = std::fs::remove_file(&target);
                let dir = target.parent().unwrap_or(&staging).to_path_buf();
                let key = dissent_files::tracker::content_key(&files_key, &e.fingerprint);
                let fref = FileRef {
                    hash: e.blob,
                    size: e.size,
                    cipher_size: e.cipher_size,
                    name: leaf(&e.path).to_string(),
                    mime: mime_guess::from_path(leaf(&e.path)).first_or_octet_stream().to_string(),
                    key: key.key,
                    nonce_prefix: key.nonce_prefix,
                };
                let peers: Vec<dissent_net::PeerAddr> =
                    self.folder_provider_peers(community, &f, e.blob).into_iter().filter(|p| p.clone().to_iroh().is_ok()).collect();
                let from: Vec<iroh::EndpointAddr> = peers.iter().filter_map(|p| p.clone().to_iroh().ok()).collect();
                // Each file's counts go to the folder's row, so its speed and busy providers show as a file's do.
                self.write().source_counts.insert(
                    id.to_string(),
                    crate::attachments::LiveSources { providers: peers, counts: Vec::new(), cipher: e.cipher_size, metered: None },
                );
                let before: u64 = manifest.entries[..i].iter().map(|x| x.size).sum();
                let engine = self.clone();
                let (tid, msg) = (id.to_string(), message_id.clone());
                let size = e.size;
                let on_progress = move |held: u64, of: u64| {
                    let part = if of == 0 { 0.0 } else { held as f64 / of as f64 };
                    engine.set_progress(&tid, (before as f64 + part * size as f64) / total as f64, &msg, true);
                };
                let done = self
                    .inner
                    .files
                    .fetch_swarm(node.endpoint(), &from, &fref, &dir, on_progress, {
                        let (engine, tid) = (self.clone(), id.to_string());
                        move |counts: &[dissent_files::SourceCount]| engine.record_sources(&tid, counts)
                    })
                    .await
                    .map_err(|err| CoreError::Invalid(format!("{}: {}", e.path, crate::attachments::failure_reason(&err))))?;
                if done.path != target {
                    std::fs::rename(&done.path, &target)?;
                }
                let owned = target.clone();
                let fp = tokio::task::spawn_blocking(move || dissent_files::tracker::fingerprint_file(&owned, |_| true))
                    .await
                    .map_err(|err| CoreError::Invalid(err.to_string()))?
                    .map_err(|err| CoreError::Invalid(err.to_string()))?;
                if fp.0 != e.fingerprint {
                    let _ = std::fs::remove_file(&target);
                    return Err(CoreError::Invalid(format!("{} does not match the folder's contents, so it was not kept", e.path)));
                }
            }
            if let Err(err) = dissent_files::mark_received(&target) {
                tracing::warn!(error = %err, "could not mark a downloaded file as from the internet");
            }
            progress[i] = FolderFileProgress { path: e.path.clone(), size: e.size, progress: 1.0, done: true };
            self.set_folder_progress(id, progress.clone());
        }
        std::fs::remove_file(staging.join(STAGING_MARKER))?;
        let name = if check_part(&f.body.name).is_ok() { f.body.name.clone() } else { "Folder".into() };
        let final_dir = free_dir(&dest, &name);
        std::fs::rename(&staging, &final_dir)?;
        // The files are kept and seeded, as a downloaded file is, and the device holds the folder.
        let hours = self.category_rules(community, f.category).and_then(|r| r.seed_hours);
        let limit = hours.map_or(SeedLimit::Forever, SeedLimit::Hours);
        let mut blobs: Vec<([u8; 32], Option<PathBuf>)> =
            manifest.entries.iter().map(|e| (e.blob, Some(check_relative(&e.path).map(|p| p.iter().fold(final_dir.clone(), |a, b| a.join(b))).unwrap_or_default()))).collect();
        blobs.push((f.body.manifest_blob, None));
        for (blob, _) in &blobs {
            if let Err(err) = self.inner.files.pin(*blob).await {
                tracing::warn!(error = %err, "could not keep a downloaded folder's file for seeding");
            }
        }
        self.with_tracker(community, |l| {
            for (blob, path) in blobs {
                l.book.seeds.entry(blob).or_insert(SeedRecord {
                    listing,
                    path,
                    limit,
                    paused: false,
                    stopped: false,
                    linked: false,
                    modified_ms: None,
                    uploaded: 0,
                    since_ms: now_ms(),
                    downloaded: true,
                });
            }
        });
        self.save_tracker_book(community);
        {
            let mut w = self.write();
            w.attach.tasks.remove(id);
            if let Some(t) = w.attach.transfers.get_mut(id) {
                t.state = "done".into();
                t.progress = 1.0;
                t.local_path = Some(final_dir);
            }
        }
        self.persist_attach();
        self.reconcile_linked(community);
        self.emit(EngineEvent::TransfersChanged);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(())
    }

    /// Where a folder download stages its files: beside where it lands, so moving it into place is a rename.
    fn folder_staging(&self, id: &str) -> PathBuf {
        self.download_dir().join(format!("{STAGING_PREFIX}{}", &id[..id.len().min(16)]))
    }

    /// Clears a folder download's staged files, once it cannot resume. Only a staging folder Dissent made is deleted.
    pub(crate) fn remove_folder_staging(&self, id: &str) {
        remove_staging(&self.folder_staging(id));
    }

    /// Clears staging folders left in the download folder for more than `STAGING_KEPT`. Called at startup.
    pub(crate) fn sweep_folder_staging(&self) {
        sweep_staging(&self.download_dir(), STAGING_KEPT);
    }

    /// The folder a linked folder listing of this device was listed from, so it can be listed again. It may be read: the
    /// person chose it when they listed it.
    pub fn relist_folder(&self, community: CommunityId, listing: EventId) -> Result<PathBuf> {
        let f = self
            .opened_folders(community)
            .into_iter()
            .find(|f| f.id == listing)
            .ok_or_else(|| CoreError::NotFound(format!("folder {listing}")))?;
        let manifest = self.known_manifests(community).remove(&f.body.manifest_blob).ok_or_else(|| CoreError::NotFound("its contents".into()))?;
        let book = self.with_tracker(community, |l| l.book.seeds.clone());
        let root = manifest
            .entries
            .iter()
            .find_map(|e| {
                let record = book.get(&e.blob).filter(|r| r.linked && r.listing == listing)?;
                let path = record.path.as_ref()?;
                let depth = e.path.split('/').count();
                path.ancestors().nth(depth).map(Path::to_path_buf)
            })
            .ok_or_else(|| CoreError::NotFound("this device did not list that folder".into()))?;
        self.allow_paths([root.clone()]);
        Ok(root)
    }

    /// `relist_folder`'s folder, without allowing it.
    pub(crate) fn relist_root(&self, community: CommunityId, listing: EventId) -> Option<String> {
        let f = self.opened_folders(community).into_iter().find(|f| f.id == listing)?;
        let manifest = self.known_manifests(community).remove(&f.body.manifest_blob)?;
        let book = self.with_tracker(community, |l| l.book.seeds.clone());
        manifest.entries.iter().find_map(|e| {
            let record = book.get(&e.blob).filter(|r| r.linked && r.listing == listing)?;
            let depth = e.path.split('/').count();
            record.path.as_ref()?.ancestors().nth(depth).map(|p| p.to_string_lossy().into_owned())
        })
    }

    /// Linked files of `listing` that changed on disk since they were listed, or went away, so they can no longer be served.
    pub(crate) fn changed_since_listed(&self, community: CommunityId, listing: EventId) -> u32 {
        self.with_tracker(community, |l| {
            l.book
                .seeds
                .values()
                .filter(|r| r.listing == listing && r.linked && super::seeding::changed_on_disk(r))
                .count() as u32
        })
    }
}

impl Engine {
    /// For tests: lists `manifest` as a folder, as a lying lister would, with no files behind it. See `test_support`.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn list_raw_folder(&self, community: CommunityId, category: CategoryId, name: &str, manifest: &Manifest) -> Result<String> {
        let (generation, files_key) = self.newest_files_key(community, false).await?;
        let bytes = postcard::to_stdvec(manifest).map_err(|e| CoreError::Encoding(e.to_string()))?;
        let fp = *blake3::hash(&bytes).as_bytes();
        let key = dissent_files::tracker::content_key(&files_key, &fp);
        let size = bytes.len() as u64;
        let input = Input::Bytes { name: "manifest.bin".into(), mime: "application/octet-stream".into(), data: bytes };
        let mref = self.inner.files.add_with_key(input, key, fp).await.map_err(|e| CoreError::Invalid(e.to_string()))?;
        let body = FolderBody {
            name: name.into(),
            files: manifest.entries.len() as u32,
            size: manifest.entries.iter().map(|e| e.size).sum(),
            manifest_fingerprint: fp,
            manifest_blob: mref.hash,
            manifest_size: size,
            manifest_cipher_size: mref.cipher_size,
            content_generation: generation,
            tags: Vec::new(),
            description: String::new(),
        };
        let inner = seal(&files_key, generation, &community, &self.inner.me, BodyKind::Folder, &body)?;
        let se = self.author_publish(community, ChannelId::CONTROL, Payload::FileListing { category, blob: Some((mref.hash, mref.cipher_size)), inner }).await?;
        let listing = EventId(se.event.id.0);
        self.with_tracker(community, |l| {
            l.book.seeds.insert(mref.hash, SeedRecord {
                listing,
                path: None,
                limit: SeedLimit::Forever,
                paused: false,
                stopped: false,
                linked: false,
                modified_ms: None,
                uploaded: 0,
                since_ms: now_ms(),
                downloaded: false,
            });
        });
        self.save_tracker_book(community);
        self.reconcile_linked(community);
        Ok(listing.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> ManifestEntry {
        ManifestEntry { path: path.into(), size: 3, kind: FileType::Other, fingerprint: [1; 32], blob: [2; 32], cipher_size: dissent_files::cipher_len(3) }
    }

    fn manifest(paths: &[&str]) -> Manifest {
        Manifest { entries: paths.iter().map(|p| entry(p)).collect() }
    }

    #[test]
    fn a_manifest_round_trips() {
        let m = manifest(&["a.png", "sub/b.txt", "sub/deeper/c.zip"]);
        let bytes = postcard::to_stdvec(&m).unwrap();
        assert_eq!(postcard::from_bytes::<Manifest>(&bytes).unwrap(), m);
        check_manifest(&m).unwrap();
    }

    #[test]
    fn a_malicious_manifest_is_refused() {
        for bad in [
            "../escape.txt",
            "sub/../../escape.txt",
            "/etc/passwd",
            "C:/Windows/evil.dll",
            "C:evil.dll",
            "sub\\evil.dll",
            "sub//double.txt",
            "./here.txt",
            "trailing.",
            "trailing ",
            " leading",
            "CON",
            "nul.txt",
            "Com1.log",
            "lpt9",
            "a<b",
            "pipe|name",
            "star*",
            "q?",
            "quote\"",
            "tab\tname",
            "",
        ] {
            assert!(check_manifest(&manifest(&[bad])).is_err(), "{bad:?} must be refused");
        }
        let long = "x".repeat(MAX_PART_CHARS + 1);
        assert!(check_manifest(&manifest(&[&long])).is_err(), "a name too long");
        let deep = ["d"; MAX_PARTS + 1].join("/");
        assert!(check_manifest(&manifest(&[&deep])).is_err(), "too deep");
        let total = format!("{}/f.txt", ["dddddddddd"; 19].join("/"));
        assert!(check_manifest(&manifest(&[&total])).is_err(), "a path too long");
    }

    #[test]
    fn names_that_clash_on_a_case_insensitive_disk_are_refused() {
        assert!(check_manifest(&manifest(&["Photo.png", "photo.PNG"])).is_err());
        assert!(check_manifest(&manifest(&["Sub/a.txt", "sub/A.txt"])).is_err());
        assert!(check_manifest(&manifest(&["thing", "Thing/inside.txt"])).is_err(), "a file that is also a folder");
        check_manifest(&manifest(&["Sub/a.txt", "sub/b.txt"])).unwrap();
    }

    #[test]
    fn a_manifest_with_sizes_that_disagree_or_too_many_files_is_refused() {
        let mut m = manifest(&["a.txt"]);
        m.entries[0].cipher_size += 1;
        assert!(check_manifest(&m).is_err());
        assert!(check_manifest(&Manifest { entries: Vec::new() }).is_err());
        let many: Vec<String> = (0..=MAX_FOLDER_FILES).map(|i| format!("f{i}.txt")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(check_manifest(&manifest(&many)).is_err());
    }

    #[test]
    fn a_folder_lands_beside_one_of_the_same_name() {
        let dir = std::env::temp_dir().join(format!("dissent-free-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        std::fs::create_dir_all(dir.join("Album (2)")).unwrap();
        assert_eq!(free_dir(&dir, "Album"), dir.join("Album (3)"));
        assert_eq!(free_dir(&dir, "Other"), dir.join("Other"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-staging-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_user_folder_that_looks_like_staging_is_never_touched() {
        let downloads = scratch("lookalike");
        // A user's folder with the very name, and one with a marker that is not Dissent's.
        let mine = downloads.join(format!("{STAGING_PREFIX}0123456789abcdef"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("holiday.png"), b"keep me").unwrap();
        let fake = downloads.join(format!("{STAGING_PREFIX}fake"));
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join(STAGING_MARKER), b"something else").unwrap();
        // And a marked folder without the prefix.
        let unnamed = downloads.join("Photos");
        std::fs::create_dir_all(&unnamed).unwrap();
        std::fs::write(unnamed.join(STAGING_MARKER), STAGING_MAGIC).unwrap();

        assert_eq!(sweep_staging(&downloads, std::time::Duration::ZERO), 0);
        assert!(!remove_staging(&mine) && !remove_staging(&fake) && !remove_staging(&unnamed));
        assert!(open_staging(&mine).is_err(), "a download never reuses a folder it did not make");
        assert_eq!(std::fs::read(mine.join("holiday.png")).unwrap(), b"keep me");
        assert!(!mine.join(STAGING_MARKER).exists(), "nor writes its marker into it");
        assert!(fake.exists() && unnamed.exists());

        // One Dissent made is swept once old enough, and only then.
        let ours = downloads.join(format!("{STAGING_PREFIX}ours"));
        open_staging(&ours).unwrap();
        std::fs::write(ours.join("part.bin"), b"x").unwrap();
        assert_eq!(sweep_staging(&downloads, std::time::Duration::from_secs(3600)), 0, "too young");
        assert_eq!(sweep_staging(&downloads, std::time::Duration::ZERO), 1);
        assert!(!ours.exists());
        assert!(mine.exists() && fake.exists() && unnamed.exists());
        let _ = std::fs::remove_dir_all(&downloads);
    }

    /// Makes `link` point at the folder `target`: a symlink on unix, a junction on Windows (no privilege needed).
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        }
    }

    #[test]
    fn deleting_staging_does_not_follow_a_link_inside_it() {
        let downloads = scratch("links");
        let outside = scratch("links-outside");
        std::fs::write(outside.join("precious.txt"), b"keep me").unwrap();
        let ours = downloads.join(format!("{STAGING_PREFIX}links"));
        open_staging(&ours).unwrap();
        link_dir(&outside, &ours.join("planted"));
        assert!(remove_staging(&ours));
        assert!(!ours.exists());
        assert_eq!(std::fs::read(outside.join("precious.txt")).unwrap(), b"keep me", "the link's target is untouched");
        // A staging path that is itself a link is not Dissent's, so it is neither used nor deleted.
        let linked = downloads.join(format!("{STAGING_PREFIX}linked"));
        std::fs::write(outside.join(STAGING_MARKER), STAGING_MAGIC).unwrap();
        link_dir(&outside, &linked);
        assert!(!is_staging(&linked));
        assert!(!remove_staging(&linked) && open_staging(&linked).is_err());
        assert_eq!(sweep_staging(&downloads, std::time::Duration::ZERO), 0);
        assert!(outside.join("precious.txt").exists());
        let _ = std::fs::remove_dir(&linked);
        let _ = std::fs::remove_dir_all(&downloads);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link_in_staging() {
        let dir = std::env::temp_dir().join(format!("dissent-staging-link-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("dissent-staging-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("sub")).unwrap();
        assert!(no_links_below(&dir, &dir.join("sub/a.txt")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
