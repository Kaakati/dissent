//! The catalog: categories and listings as the UI shows them, and the actions on them: making categories, adding files
//! (with dedupe by fingerprint), editing and removing listings, and downloading. See `docs/design/14-file-tracker.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dissent_events::{AddSpec, CategoryId, ChannelId, EventId, LevelRule, Payload, Perms, ReaderSpec, ReputationRules};
use dissent_files::{FileRef, Input, MAX_FILE_SIZE};
use dissent_identity::{CommunityId, RootId};
use serde::{Deserialize, Serialize};

use super::bodies::{
    clean_description, clean_tags, open, seal, BodyKind, CategoryBody, DownloadRules, FileType, ListingBody,
    ListingUpdateBody, MAX_CATEGORY_NAME_CHARS,
};
use super::{SeedLimit, SeedRecord};
use crate::attachments::{attachment_id, Transfer, TransferView};
use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::views::EngineEvent;

/// The prefix of a tracker download's `Transfer::message_id`: `files:<community>:<listing>`.
pub(crate) const TRANSFER_PREFIX: &str = "files:";

/// Who may add, as the UI edits it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AddSpecView {
    /// "member", "moderator", "admin", "owner" or "none".
    pub min_rank: String,
    pub roles: Vec<String>,
    pub members: Vec<String>,
    pub min_level: u8,
}

/// A category, as the UI shows it.
#[derive(Clone, Debug, Serialize)]
pub struct CategoryView {
    pub id: String,
    /// False while this device lacks the key of the generation it is sealed under. The other fields are empty then.
    pub readable: bool,
    pub name: String,
    pub icon: String,
    /// Allowed type names. Empty allows every type.
    pub types: Vec<String>,
    pub default_tags: Vec<String>,
    pub rules: DownloadRules,
    pub add: AddSpecView,
    pub restricted: bool,
    /// Who may see it, for a restricted category.
    pub visible: Option<ReaderSpecView>,
    pub can_add: bool,
    pub files: u32,
    pub bytes: u64,
    /// This computer's linked folder, never shared.
    pub folder: Option<String>,
    pub auto_import: bool,
}

/// A listing, as the UI shows it.
#[derive(Clone, Debug, Serialize)]
pub struct ListingView {
    pub id: String,
    pub category: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub mime: String,
    pub size: u64,
    pub cipher_size: u64,
    pub pieces: u64,
    /// `blake3(plaintext)`, lowercase hex.
    pub fingerprint: String,
    pub tags: Vec<String>,
    pub description: String,
    pub lister: String,
    pub lister_name: String,
    pub added_ms: u64,
    /// Others who listed the same file, merged into this listing.
    pub also_listed_by: Vec<String>,
    pub seeders: u32,
    pub leechers: u32,
    pub completed: u32,
    pub mine: bool,
    /// "seeding", "paused", "downloading", "downloaded", "failed" or "none".
    pub local: String,
    pub local_path: Option<String>,
    pub progress: f64,
    pub can_edit: bool,
}

/// The next reputation level and how far this member is from it.
#[derive(Clone, Debug, Serialize)]
pub struct NextLevel {
    pub level: u8,
    pub name: String,
    pub bytes_needed: u64,
    pub members_needed: u32,
    /// 0 to 1.
    pub progress: f64,
}

/// This member's standing in the server.
#[derive(Clone, Debug, Serialize)]
pub struct StandingView {
    pub level: u8,
    pub level_name: String,
    pub next: Option<NextLevel>,
    pub credited_bytes: u64,
    pub served_members: u32,
    pub uploaded: u64,
    pub downloaded: u64,
    /// Uploaded over downloaded; `None` before anything was downloaded.
    pub ratio: Option<f64>,
}

/// The whole files area of a server.
#[derive(Clone, Debug, Serialize)]
pub struct FilesView {
    pub can_manage: bool,
    pub categories: Vec<CategoryView>,
    pub listings: Vec<ListingView>,
    /// Listings this device cannot open yet, for want of a key.
    pub locked: u32,
    pub standing: StandingView,
    pub rules: ReputationRules,
    pub all_types: Vec<String>,
}

/// A category as the UI submits it.
#[derive(Clone, Debug, Deserialize)]
pub struct CategoryInput {
    pub name: String,
    pub icon: String,
    pub types: Vec<String>,
    pub default_tags: Vec<String>,
    pub rules: DownloadRules,
    pub add: AddSpecView,
    /// Who may see it, or `None` for every member. Fixed when the category is made.
    #[serde(default)]
    pub visible: Option<ReaderSpecView>,
}

/// Who may see a restricted category, as the UI edits it. `min_rank` as in `AddSpecView`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReaderSpecView {
    pub min_rank: String,
    pub roles: Vec<String>,
    pub members: Vec<String>,
}

/// A file found for adding, and what adding it would do.
#[derive(Clone, Debug, Serialize)]
pub struct CandidateView {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: u64,
    /// "ready", "already_listed", "type_not_allowed", "too_large" or "unreadable".
    pub state: String,
    pub fingerprint: String,
    pub listed_by: Option<String>,
    pub listing: Option<String>,
}

/// One file to add.
#[derive(Clone, Debug, Deserialize)]
pub struct AddItem {
    pub path: String,
    pub tags: Vec<String>,
    pub description: String,
}

const RANKS: [&str; 5] = ["member", "moderator", "admin", "owner", "none"];

fn rank_name(n: u8) -> String {
    RANKS.get(n as usize).copied().unwrap_or("none").to_string()
}

fn rank_number(name: &str) -> Result<u8> {
    RANKS
        .iter()
        .position(|r| *r == name)
        .map(|n| n as u8)
        .ok_or_else(|| CoreError::Invalid(format!("unknown rank {name}")))
}

fn spec_view(spec: &AddSpec) -> AddSpecView {
    AddSpecView {
        min_rank: rank_name(spec.min_rank),
        roles: spec.roles.iter().map(|r| r.to_string()).collect(),
        members: spec.members.iter().map(|r| r.to_string()).collect(),
        min_level: spec.min_level,
    }
}

fn reader_spec_of(view: &ReaderSpecView) -> Result<ReaderSpec> {
    let add = spec_of(&AddSpecView { min_rank: view.min_rank.clone(), roles: view.roles.clone(), members: view.members.clone(), min_level: 0 })?;
    Ok(ReaderSpec { min_rank: add.min_rank, roles: add.roles, members: add.members })
}

fn spec_of(view: &AddSpecView) -> Result<AddSpec> {
    fn parse<T: std::str::FromStr>(what: &str, s: &str) -> Result<T> {
        s.parse().map_err(|_| CoreError::Invalid(format!("bad {what} id")))
    }
    Ok(AddSpec {
        min_rank: rank_number(&view.min_rank)?,
        roles: view.roles.iter().map(|r| parse("role", r)).collect::<Result<_>>()?,
        members: view.members.iter().map(|m| parse("member", m)).collect::<Result<_>>()?,
        min_level: view.min_level,
    })
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_uppercase()).filter(|e| e.len() <= 5).unwrap_or_default()
}

fn files_err(e: dissent_files::FilesError) -> CoreError {
    CoreError::Invalid(e.to_string())
}

fn hex(bytes: &[u8; 32]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

fn category_body(input: &CategoryInput) -> Result<CategoryBody> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_CATEGORY_NAME_CHARS {
        return Err(CoreError::Invalid(format!("a category name is 1 to {MAX_CATEGORY_NAME_CHARS} characters")));
    }
    let types = input
        .types
        .iter()
        .map(|t| FileType::parse(t).ok_or_else(|| CoreError::Invalid(format!("unknown file type {t}"))))
        .collect::<Result<Vec<_>>>()?;
    Ok(CategoryBody {
        name,
        icon: input.icon.trim().chars().take(32).collect(),
        types,
        default_tags: clean_tags(&input.default_tags)?,
        rules: input.rules.clone(),
    })
}

/// A listing with its opened body, for the view and for dedupe.
pub(crate) struct Opened {
    pub id: EventId,
    pub category: CategoryId,
    pub lister: RootId,
    pub added_ms: u64,
    pub body: ListingBody,
}

impl Engine {
    /// Every listing of `community` this device can open and its person may see, oldest first.
    pub(crate) fn opened_listings(&self, community: CommunityId) -> Vec<Opened> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let st = c.state();
        let mut out: Vec<Opened> = st
            .files
            .listings
            .iter()
            .filter(|(_, l)| st.may_see_files(&w.me, &l.category))
            .filter_map(|(id, l)| {
                let key = keys.get(&l.inner.generation)?;
                let mut body: ListingBody = open(key, &community, &l.lister, BodyKind::Listing, &l.inner)?;
                if let Some((_, update)) = &l.update {
                    if let Some(u) = keys
                        .get(&update.generation)
                        .and_then(|k| open::<ListingUpdateBody>(k, &community, &l.lister, BodyKind::ListingUpdate, update))
                    {
                        body.tags = u.tags;
                        body.description = u.description;
                    }
                }
                // A body that disagrees with what replay checked is refused, as a mismatched manifest is.
                if l.blob.is_some_and(|(hash, size)| hash != body.blob || size != body.cipher_size)
                    || body.cipher_size != dissent_files::cipher_len(body.size)
                {
                    return None;
                }
                Some(Opened { id: *id, category: l.category, lister: l.lister, added_ms: l.hlc.wall_ms, body })
            })
            .collect();
        out.sort_by_key(|o| (o.added_ms, o.id));
        out
    }

    /// The files area of `community`.
    pub fn files_view(&self, community: CommunityId) -> Result<FilesView> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let book = self.with_tracker(community, |l| l.book.clone());
        let opened = self.opened_listings(community);
        let sizes: BTreeMap<[u8; 32], u64> = opened.iter().map(|o| (o.body.blob, o.body.cipher_size)).collect();
        let swarms = self.swarms(community, &sizes);
        let seed_states = self.seed_states(community);
        let heard_roots: std::collections::BTreeSet<RootId> =
            self.with_tracker(community, |l| l.heard.values().filter_map(|h| h.root).collect());
        let transfers: BTreeMap<String, (String, f64, Option<PathBuf>)> = {
            let w = self.read();
            w.attach
                .transfers
                .values()
                .filter(|t| t.message_id.starts_with(TRANSFER_PREFIX))
                .map(|t| (t.id.clone(), (t.state.clone(), t.progress, t.local_path.clone())))
                .collect()
        };
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
        let st = c.state();
        let me = w.me;
        let now = now_ms();
        let manage = st.perms_of(&me).contains(Perms::MANAGE_FILES);
        let mut missing = st.files.listings.values().any(|l| !keys.contains_key(&l.inner.generation));

        // Merge listings of the same blob into the earliest, which is the one shown.
        let mut by_blob: BTreeMap<[u8; 32], Vec<&Opened>> = BTreeMap::new();
        for o in &opened {
            by_blob.entry(o.body.blob).or_default().push(o);
        }
        let mut counts: BTreeMap<CategoryId, (u32, u64)> = BTreeMap::new();
        let mut listings = Vec::new();
        for group in by_blob.values() {
            let first = group[0];
            let entry = counts.entry(first.category).or_default();
            entry.0 += 1;
            entry.1 += first.body.size;
            let mut holders: Vec<RootId> = group.iter().map(|o| o.lister).collect();
            holders.sort_unstable();
            holders.dedup();
            let seed = book.seeds.get(&first.body.blob);
            let transfer_id = data_encoding::BASE32_NOPAD.encode(&first.body.blob).to_lowercase();
            let (local, progress, local_path) = match (seed, transfers.get(&transfer_id)) {
                (Some(s), _) if seed_states.get(&first.body.blob) != Some(&super::seeding::SeedState::Seeding) => {
                    ("paused".to_string(), 1.0, s.path.clone())
                }
                (Some(s), _) => ("seeding".to_string(), 1.0, s.path.clone()),
                (None, Some((state, progress, path))) => {
                    let local = match state.as_str() {
                        "done" => "downloaded",
                        "failed" => "failed",
                        _ => "downloading",
                    };
                    (local.to_string(), *progress, path.clone())
                }
                (None, None) => ("none".to_string(), 0.0, None),
            };
            // Seeders: whoever said they hold it whole lately, listers not heard from yet, and this person if it seeds.
            let swarm = swarms.get(&first.body.blob).cloned().unwrap_or_default();
            let mut seeding_roots = swarm.seeders.clone();
            seeding_roots.extend(holders.iter().filter(|r| !heard_roots.contains(r)).copied());
            if seed_states.get(&first.body.blob) == Some(&super::seeding::SeedState::Seeding) {
                seeding_roots.insert(me);
            } else {
                seeding_roots.remove(&me);
            }
            let seeders = seeding_roots.len() as u32;
            let leechers = swarm.leechers.iter().filter(|r| **r != me).count() as u32 + u32::from(local == "downloading");
            let completed = book.completed.get(&first.body.blob).map_or(0, |c| c.iter().filter(|r| !holders.contains(r)).count()) as u32;
            listings.push(ListingView {
                id: first.id.to_string(),
                category: first.category.to_string(),
                name: first.body.name.clone(),
                ext: ext_of(&first.body.name),
                kind: first.body.kind.name().to_string(),
                mime: first.body.mime.clone(),
                size: first.body.size,
                cipher_size: first.body.cipher_size,
                pieces: dissent_files::pieces_of(first.body.cipher_size),
                fingerprint: hex(&first.body.fingerprint),
                tags: first.body.tags.clone(),
                description: first.body.description.clone(),
                lister: first.lister.to_string(),
                lister_name: w.display_name(&first.lister),
                added_ms: first.added_ms,
                also_listed_by: group[1..].iter().map(|o| w.display_name(&o.lister)).collect(),
                seeders,
                leechers,
                completed,
                mine: group.iter().any(|o| o.lister == me),
                local,
                local_path: local_path.map(|p| p.to_string_lossy().into_owned()),
                progress,
                can_edit: first.lister == me || manage,
            });
        }

        let mut categories: Vec<CategoryView> = Vec::new();
        for (id, cat) in &st.files.categories {
            if !st.may_see_files(&me, id) {
                continue;
            }
            let body = keys.get(&cat.inner.generation).and_then(|k| {
                open::<CategoryBody>(k, &community, &category_author(st, id), BodyKind::Category, &cat.inner)
            });
            missing |= body.is_none();
            let (files, bytes) = counts.get(id).copied().unwrap_or_default();
            let b = body.clone();
            categories.push(CategoryView {
                id: id.to_string(),
                readable: b.is_some(),
                name: b.as_ref().map(|b| b.name.clone()).unwrap_or_default(),
                icon: b.as_ref().map(|b| b.icon.clone()).unwrap_or_default(),
                types: b.as_ref().map(|b| b.types.iter().map(|t| t.name().to_string()).collect()).unwrap_or_default(),
                default_tags: b.as_ref().map(|b| b.default_tags.clone()).unwrap_or_default(),
                rules: b.map(|b| b.rules).unwrap_or_default(),
                add: spec_view(&cat.add),
                restricted: cat.visible.is_some(),
                visible: cat.visible.as_ref().map(|v| ReaderSpecView {
                    min_rank: rank_name(v.min_rank),
                    roles: v.roles.iter().map(|r| r.to_string()).collect(),
                    members: v.members.iter().map(|r| r.to_string()).collect(),
                }),
                can_add: st.may_add_files(&me, id, now),
                files,
                bytes,
                folder: book.folders.get(id).map(|p| p.to_string_lossy().into_owned()),
                auto_import: book.auto_import.contains(id),
            });
        }
        categories.sort_by_key(|c| st.files.categories.get(&c.id.parse().unwrap_or(CategoryId([0; 32]))).map(|s| s.created));

        let rules = st.files.rules();
        let level = st.files.level_of(&me, now);
        let credited = st.files.credited_bytes(&me, now);
        let served = st.files.served_members(&me);
        let next = rules.levels.get(level as usize).map(|l: &LevelRule| {
            let by_bytes = if l.bytes == 0 { 1.0 } else { (credited as f64 / l.bytes as f64).min(1.0) };
            let by_members = if l.members == 0 { 1.0 } else { (f64::from(served) / f64::from(l.members)).min(1.0) };
            NextLevel {
                level: level + 1,
                name: l.name.clone(),
                bytes_needed: l.bytes.saturating_sub(credited),
                members_needed: l.members.saturating_sub(served),
                progress: by_bytes.min(by_members),
            }
        });
        let uploaded: u64 = book.seeds.values().map(|s| s.uploaded).sum();
        let downloaded: u64 = book
            .seeds
            .iter()
            .filter(|(_, s)| s.downloaded)
            .filter_map(|(blob, _)| opened.iter().find(|o| o.body.blob == *blob).map(|o| o.body.cipher_size))
            .sum();
        let standing = StandingView {
            level,
            level_name: rules.levels.get(level.saturating_sub(1) as usize).map(|l| l.name.clone()).unwrap_or_default(),
            next,
            credited_bytes: credited,
            served_members: served,
            uploaded,
            downloaded,
            ratio: (downloaded > 0).then(|| uploaded as f64 / downloaded as f64),
        };
        let locked = st.files.listings.len().saturating_sub(opened.len()) as u32;
        drop(w);
        let locked_restricted: Vec<CategoryId> = categories
            .iter()
            .filter(|c| c.restricted && !c.readable)
            .filter_map(|c| c.id.parse().ok())
            .collect();
        let engine = self.clone();
        tokio::spawn(async move {
            if missing {
                engine.fetch_files_keys(community, false).await;
            }
            for category in locked_restricted {
                engine.fetch_category_keys(community, category).await;
            }
            engine.refresh_holdings(community, false).await;
        });
        Ok(FilesView {
            can_manage: manage,
            categories,
            listings,
            locked,
            standing,
            rules,
            all_types: FileType::ALL.iter().map(|t| t.name().to_string()).collect(),
        })
    }

    /// Makes a category. Restricted categories, with their own keys, are not built yet.
    pub async fn create_category(&self, community: CommunityId, input: CategoryInput) -> Result<CategoryId> {
        let body = category_body(&input)?;
        // The first category turns the Files area on for everyone, so it waits until every member's app has it.
        let first = self.read().communities.get(&community).is_some_and(|c| c.state().files.categories.is_empty());
        if first {
            let waiting = self.devices_without(community, crate::custom_roles::FEATURE_TRACKER).await;
            if waiting > 0 {
                return Err(CoreError::Invalid(format!("{waiting} member device(s) need to update before this server can share files")));
            }
        }
        let add = spec_of(&input.add)?;
        let visible = input.visible.as_ref().map(reader_spec_of).transpose()?;
        // A restricted category is sealed under a key of its own from the start; an open one under the files key.
        let (inner, pending) = match &visible {
            Some(_) => {
                let (generation, key) = self.make_category_key(community, super::restricted::PENDING);
                (seal(&key, generation, &community, &self.inner.me, BodyKind::Category, &body)?, Some(generation))
            }
            None => (self.seal_files_body(community, BodyKind::Category, &body, true).await?, None),
        };
        let payload = Payload::FileCategorySet { category: None, add, visible, inner };
        let se = self.author_publish(community, ChannelId::CONTROL, payload).await?;
        let id = CategoryId(se.event.id.0);
        if let Some(generation) = pending {
            self.assign_scope(community, generation, id);
        }
        Ok(id)
    }

    /// Replaces a category's spec. Its readers stay as they are.
    pub async fn update_category(&self, community: CommunityId, category: CategoryId, input: CategoryInput) -> Result<()> {
        let body = category_body(&input)?;
        let add = spec_of(&input.add)?;
        let visible: Option<ReaderSpec> = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.state().files.categories.get(&category).map(|s| s.visible.clone()))
            .ok_or_else(|| CoreError::NotFound(format!("category {category}")))?;
        if input.visible.is_some() != visible.is_some() {
            return Err(CoreError::Invalid("a category stays open or restricted as it was made".into()));
        }
        let visible = match &input.visible {
            Some(v) => Some(reader_spec_of(v)?),
            None => None,
        };
        // A restricted category's readers may change, so its spec goes under a new generation, offered to the readers
        // of the new spec once it is in.
        let inner = match &visible {
            Some(_) => {
                let (generation, key) = self.make_category_key(community, category);
                seal(&key, generation, &community, &self.inner.me, BodyKind::Category, &body)?
            }
            None => self.seal_files_body(community, BodyKind::Category, &body, true).await?,
        };
        let generation = inner.generation;
        let restricted = visible.is_some();
        let payload = Payload::FileCategorySet { category: Some(category), add, visible, inner };
        self.author_publish(community, ChannelId::CONTROL, payload).await?;
        if restricted {
            let key = self.files_key(community, generation).unwrap_or_default();
            for device in self.reader_devices_of(community, category) {
                let push = crate::rpc::Request::PushCategoryKeys { community, category, pairs: vec![(generation, key)] };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.call(device, &push)).await;
            }
        }
        Ok(())
    }

    pub async fn delete_category(&self, community: CommunityId, category: CategoryId) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::FileCategoryDelete { category }).await?;
        self.with_tracker(community, |l| {
            l.book.folders.remove(&category);
            l.book.auto_import.remove(&category);
        });
        self.save_tracker_book(community);
        Ok(())
    }

    /// Links `category` to a folder on this computer, or unlinks it. The path never leaves this device.
    pub fn set_category_folder(&self, community: CommunityId, category: CategoryId, folder: Option<PathBuf>) -> Result<()> {
        if let Some(dir) = &folder {
            if !dir.is_dir() {
                return Err(CoreError::Invalid(format!("{} is not a folder", dir.display())));
            }
        }
        self.with_tracker(community, |l| match folder {
            Some(dir) => {
                l.book.folders.insert(category, dir);
            }
            None => {
                l.book.folders.remove(&category);
                l.book.auto_import.remove(&category);
            }
        });
        self.save_tracker_book(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(())
    }

    pub fn set_category_auto_import(&self, community: CommunityId, category: CategoryId, on: bool) -> Result<()> {
        self.with_tracker(community, |l| {
            if on && l.book.folders.contains_key(&category) {
                l.book.auto_import.insert(category);
            } else {
                l.book.auto_import.remove(&category);
            }
        });
        self.save_tracker_book(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(())
    }

    /// Seals a body under the newest files key. A file manager may make the first key.
    async fn seal_files_body<T: Serialize>(&self, community: CommunityId, kind: BodyKind, body: &T, may_create: bool) -> Result<dissent_events::FilesSealed> {
        let me = self.inner.me;
        let manage = may_create
            && self
                .read()
                .communities
                .get(&community)
                .is_some_and(|c| c.state().perms_of(&me).contains(Perms::MANAGE_FILES));
        let (generation, key) = self.newest_files_key(community, manage).await?;
        seal(&key, generation, &community, &me, kind, body)
    }

    /// What adding `paths` to `category` would do: each file's type, fingerprint, and whether it is already listed.
    pub async fn prepare_listing(&self, community: CommunityId, category: CategoryId, paths: Vec<PathBuf>) -> Result<Vec<CandidateView>> {
        let types = self.category_types(community, category)?;
        let opened = self.opened_listings(community);
        let mut out = Vec::new();
        for path in paths {
            let view = self.candidate(&path, &types, &opened).await;
            if view.state == "ready" || view.state == "already_listed" {
                self.with_tracker(community, |l| l.offered.insert(path.clone()));
            }
            out.push(view);
        }
        Ok(out)
    }

    async fn candidate(&self, path: &Path, types: &[FileType], opened: &[Opened]) -> CandidateView {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let kind = FileType::of_name(&name);
        let mut view = CandidateView {
            path: path.to_string_lossy().into_owned(),
            ext: ext_of(&name),
            kind: kind.name().to_string(),
            name,
            size: 0,
            state: "ready".into(),
            fingerprint: String::new(),
            listed_by: None,
            listing: None,
        };
        // A link is not followed: its target may be a file the person never meant to share.
        let Some(meta) = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file()) else {
            view.state = "unreadable".into();
            return view;
        };

        view.size = meta.len();
        if !types.is_empty() && !types.contains(&kind) {
            view.state = "type_not_allowed".into();
            return view;
        }
        if meta.len() > MAX_FILE_SIZE {
            view.state = "too_large".into();
            return view;
        }
        let owned = path.to_path_buf();
        let fingerprint = tokio::task::spawn_blocking(move || dissent_files::tracker::fingerprint_file(&owned, |_| true)).await;
        let Ok(Ok((fp, _))) = fingerprint else {
            view.state = "unreadable".into();
            return view;
        };
        view.fingerprint = hex(&fp);
        if let Some(existing) = opened.iter().find(|o| o.body.fingerprint == fp) {
            view.state = "already_listed".into();
            view.listed_by = Some(self.read().display_name(&existing.lister));
            view.listing = Some(existing.id.to_string());
        }
        view
    }

    fn category_types(&self, community: CommunityId, category: CategoryId) -> Result<Vec<FileType>> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
        let st = c.state();
        let cat = st.files.categories.get(&category).ok_or_else(|| CoreError::NotFound(format!("category {category}")))?;
        let key = keys.get(&cat.inner.generation).ok_or_else(|| CoreError::Invalid("this device lacks the key of that category yet".into()))?;
        let body: CategoryBody = open(key, &community, &category_author(st, &category), BodyKind::Category, &cat.inner)
            .ok_or_else(|| CoreError::Invalid("the category does not open".into()))?;
        Ok(body.types)
    }

    /// Adds files to `category`. A file already listed with the same fingerprint is not listed again: this device
    /// seeds that listing. Returns the listing id of each file, in order.
    pub async fn list_files(&self, community: CommunityId, category: CategoryId, items: Vec<AddItem>, limit: SeedLimit) -> Result<Vec<String>> {
        let types = self.category_types(community, category)?;
        let me = self.inner.me;
        let restricted = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.state().files.categories.get(&category).map(|s| s.visible.is_some()))
            .unwrap_or(false);
        if !self.read().communities.get(&community).is_some_and(|c| c.state().may_add_files(&me, &category, now_ms())) {
            return Err(CoreError::Rejected("NoPermission".into()));
        }
        let mut out = Vec::new();
        for item in items {
            let path = PathBuf::from(&item.path);
            let offered = self.with_tracker(community, |l| l.offered.contains(&path));
            if !offered || std::fs::symlink_metadata(&path).map(|m| !m.is_file()).unwrap_or(true) {
                return Err(CoreError::Invalid(format!("{} was not picked or found in a linked folder", path.display())));
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let kind = FileType::of_name(&name);
            if !types.is_empty() && !types.contains(&kind) {
                return Err(CoreError::Invalid(format!("{name}: {} files are not allowed in this category", kind.name())));
            }
            let tags = clean_tags(&item.tags)?;
            let description = clean_description(&item.description)?;
            let owned = path.clone();
            let (fp, size) = tokio::task::spawn_blocking(move || dissent_files::tracker::fingerprint_file(&owned, |_| true))
                .await
                .map_err(|e| CoreError::Invalid(e.to_string()))?
                .map_err(files_err)?;
            // Dedupe within the same keys: an open category's file against open listings, a restricted one's against its
            // own category. A listing whose blob is not what these bytes make under its key was made of other bytes:
            // this file is listed anew rather than refused, so nobody can squat a fingerprint.
            let scope_of = |o: &Opened| -> bool {
                if restricted {
                    o.category == category
                } else {
                    !self
                        .read()
                        .communities
                        .get(&community)
                        .and_then(|c| c.state().files.categories.get(&o.category).map(|s| s.visible.is_some()))
                        .unwrap_or(false)
                }
            };
            let mut existing = self.opened_listings(community).into_iter().find(|o| o.body.fingerprint == fp && scope_of(o));
            let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
            // A file in the category's linked folder is seeded from there; any other is copied into the store.
            let linked = self.with_tracker(community, |l| l.book.folders.get(&category).cloned()).is_some_and(|dir| path.starts_with(dir));
            let (generation, files_key, fref) = loop {
                let generation = match &existing {
                    Some(o) => o.body.content_generation,
                    None if restricted => self.newest_category_key(community, category).await?.0,
                    None => self.newest_files_key(community, false).await?.0,
                };
                let files_key = match self.files_key(community, generation) {
                    Some(k) => k,
                    None => {
                        self.fetch_files_keys(community, true).await;
                        self.files_key(community, generation)
                            .ok_or_else(|| CoreError::Invalid("this device lacks the key that file was listed under".into()))?
                    }
                };
                let key = dissent_files::tracker::content_key(&files_key, &fp);
                let fref = if linked {
                    self.inner.files.link_file(&path, &mime, key, fp).await.map_err(files_err)?
                } else {
                    self.inner
                        .files
                        .add_with_key(Input::Path { path: path.clone(), mime: mime.clone() }, key, fp)
                        .await
                        .map_err(files_err)?
                };
                match &existing {
                    Some(o) if fref.hash != o.body.blob => {
                        if linked {
                            self.inner.files.forget_linked(fref.hash);
                        } else {
                            let _ = self.inner.files.unpin(fref.hash).await;
                        }
                        tracing::warn!(listing = %o.id, "a listing's blob does not match its fingerprint; listing this file anew");
                        existing = None;
                    }
                    _ => break (generation, files_key, fref),
                }
            };
            let listing = match existing {
                Some(o) => o.id,
                None => {
                    let body = ListingBody {
                        name: fref.name.clone(),
                        mime: fref.mime.clone(),
                        kind,
                        size,
                        fingerprint: fp,
                        content_generation: generation,
                        blob: fref.hash,
                        cipher_size: fref.cipher_size,
                        tags,
                        description,
                    };
                    let inner = seal(&files_key, generation, &community, &me, BodyKind::Listing, &body)?;
                    // A restricted category's blob stays sealed, so storage seeders never learn or mirror it.
                    let blob = (!restricted).then_some((fref.hash, fref.cipher_size));
                    let payload = Payload::FileListing { category, blob, inner };
                    EventId(self.author_publish(community, ChannelId::CONTROL, payload).await?.event.id.0)
                }
            };
            self.with_tracker(community, |l| {
                l.book.seeds.insert(
                    fref.hash,
                    SeedRecord {
                        listing,
                        modified_ms: linked.then(|| super::modified_ms(&path)).flatten(),
                        linked,
                        path: Some(path),
                        limit,
                        paused: false,
                        stopped: false,
                        uploaded: 0,
                        since_ms: now_ms(),
                        downloaded: false,
                    },
                );
            });
            out.push(listing.to_string());
        }
        self.save_tracker_book(community);
        // The gate serves a seed only once it knows its state and category.
        self.reconcile_linked(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
        Ok(out)
    }

    /// Replaces a listing's tags and description.
    pub async fn update_listing(&self, community: CommunityId, listing: EventId, tags: Vec<String>, description: String) -> Result<()> {
        let body = ListingUpdateBody { tags: clean_tags(&tags)?, description: clean_description(&description)? };
        // The update opens with the lister's author in its associated data, so only the lister's own app writes it.
        let lister = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.state().files.listings.get(&listing).map(|l| l.lister))
            .ok_or_else(|| CoreError::NotFound(format!("listing {listing}")))?;
        if lister != self.inner.me {
            return Err(CoreError::Invalid("only the lister edits a listing; file managers may remove it".into()));
        }
        let category = self.read().communities.get(&community).and_then(|c| {
            let st = c.state();
            let cat = st.files.listings.get(&listing)?.category;
            st.files.categories.get(&cat).filter(|s| s.visible.is_some()).map(|_| cat)
        });
        let inner = match category {
            Some(cat) => {
                let (generation, key) = self.newest_category_key(community, cat).await?;
                seal(&key, generation, &community, &self.inner.me, BodyKind::ListingUpdate, &body)?
            }
            None => self.seal_files_body(community, BodyKind::ListingUpdate, &body, false).await?,
        };
        self.author_publish(community, ChannelId::CONTROL, Payload::FileListingUpdate { listing, inner }).await?;
        Ok(())
    }

    /// Removes a listing. This device stops seeding it, unless another listing names the same file.
    pub async fn remove_listing(&self, community: CommunityId, listing: EventId) -> Result<()> {
        let blob = self.opened_listings(community).into_iter().find(|o| o.id == listing).map(|o| o.body.blob);
        self.author_publish(community, ChannelId::CONTROL, Payload::FileListingRemove { listing }).await?;
        if let Some(blob) = blob {
            self.drop_seed_if_unlisted(community, blob).await;
        }
        Ok(())
    }

    /// Stops seeding `blob` and lets its copy go when no listing names it any more.
    pub(crate) async fn drop_seed_if_unlisted(&self, community: CommunityId, blob: [u8; 32]) {
        if self.opened_listings(community).iter().any(|o| o.body.blob == blob) {
            return;
        }
        let Some(record) = self.with_tracker(community, |l| l.book.seeds.remove(&blob)) else { return };
        self.save_tracker_book(community);
        if record.linked {
            self.inner.files.forget_linked(blob);
        } else {
            // Through the release path, so a blob an attachment also uses keeps its pin.
            self.release_blobs(vec![blob]);
        }
    }

    /// Starts downloading a listing into the download folder. It shows in Transfers like an attachment.
    pub async fn download_listing(&self, community: CommunityId, listing: EventId) -> Result<TransferView> {
        let opened = self.opened_listings(community);
        let o = opened
            .iter()
            .find(|o| o.id == listing)
            .ok_or_else(|| CoreError::NotFound(format!("listing {listing}")))?;
        let files_key = self
            .files_key(community, o.body.content_generation)
            .ok_or_else(|| CoreError::Invalid("this device lacks the key that file was listed under".into()))?;
        let key = dissent_files::tracker::content_key(&files_key, &o.body.fingerprint);
        let file = FileRef {
            hash: o.body.blob,
            size: o.body.size,
            cipher_size: o.body.cipher_size,
            name: o.body.name.clone(),
            mime: o.body.mime.clone(),
            key: key.key,
            nonce_prefix: key.nonce_prefix,
        };
        file.check(self.inner.files.size_limit()).map_err(files_err)?;
        // Whole holders first, freshest first, then partial ones, then the listers' devices.
        let mut roots: Vec<RootId> = opened.iter().filter(|x| x.body.blob == o.body.blob).map(|x| x.lister).collect();
        roots.dedup();
        roots.retain(|r| *r != self.inner.me);
        let sizes = BTreeMap::from([(o.body.blob, o.body.cipher_size)]);
        let swarm = self.swarms(community, &sizes).remove(&o.body.blob).unwrap_or_default();
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
        for p in self.providers_for(&roots) {
            if !providers.iter().any(|x| x.id == p.id) {
                providers.push(p);
            }
        }
        let id = attachment_id(&file);
        {
            let mut w = self.write();
            let busy = w.attach.transfers.get(&id).is_some_and(|t| t.state == "active" || t.state == "waiting");
            if !busy {
                w.attach.transfers.insert(
                    id.clone(),
                    Transfer {
                        id: id.clone(),
                        message_id: format!("{TRANSFER_PREFIX}{community}:{listing}"),
                        direction: "download".into(),
                        file,
                        providers: providers.iter().map(|p| p.to_string()).collect(),
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
        // The gate learns the blob's category, so this device can pass on the pieces it gets.
        self.reconcile_linked(community);
        self.start_download(&id);
        self.emit(EngineEvent::TransfersChanged);
        self.transfer_view(&id)
    }

    /// Keeps a finished tracker download pinned and seeds it. `message_id` is the transfer's `files:<community>:<listing>`.
    /// The seed lasts as the category's rules ask (`seed_hours`), or for good.
    pub(crate) async fn seed_downloaded(&self, message_id: &str, blob: [u8; 32], path: PathBuf) {
        let Some((community, listing)) = message_id
            .strip_prefix(TRANSFER_PREFIX)
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(c, l)| Some((c.parse::<CommunityId>().ok()?, l.parse::<EventId>().ok()?)))
        else {
            return;
        };
        if let Err(e) = self.inner.files.pin(blob).await {
            tracing::warn!(error = %e, "could not keep a downloaded listing for seeding");
            return;
        }
        let hours = self
            .opened_listings(community)
            .into_iter()
            .find(|o| o.id == listing)
            .and_then(|o| self.category_rules(community, o.category))
            .and_then(|r| r.seed_hours);
        let limit = hours.map_or(SeedLimit::Forever, SeedLimit::Hours);
        self.with_tracker(community, |l| {
            l.book.seeds.entry(blob).or_insert(SeedRecord {
                listing,
                path: Some(path),
                limit,
                paused: false,
                stopped: false,
                linked: false,
                modified_ms: None,
                uploaded: 0,
                since_ms: now_ms(),
                downloaded: true,
            });
        });
        self.save_tracker_book(community);
        self.reconcile_linked(community);
        self.emit(EngineEvent::CommunityUpdated { community_id: community.to_string() });
    }

    fn category_rules(&self, community: CommunityId, category: CategoryId) -> Option<DownloadRules> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let w = self.read();
        let st = w.communities.get(&community)?.state();
        let cat = st.files.categories.get(&category)?;
        let body: CategoryBody = open(keys.get(&cat.inner.generation)?, &community, &cat.author, BodyKind::Category, &cat.inner)?;
        Some(body.rules)
    }

    /// For tests: lists `bytes` under a fingerprint they do not have, as a lying lister would. See `test_support`.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn list_with_false_fingerprint(
        &self,
        community: CommunityId,
        category: CategoryId,
        name: &str,
        bytes: Vec<u8>,
        claimed: [u8; 32],
    ) -> Result<String> {
        let actual = *blake3::hash(&bytes).as_bytes();
        let size = bytes.len() as u64;
        let (generation, files_key) = self.newest_files_key(community, false).await?;
        let key = dissent_files::tracker::content_key(&files_key, &claimed);
        let input = Input::Bytes { name: name.into(), mime: "application/octet-stream".into(), data: bytes };
        let fref = self.inner.files.add_with_key(input, key, actual).await.map_err(files_err)?;
        let body = ListingBody {
            name: name.into(),
            mime: fref.mime.clone(),
            kind: FileType::of_name(name),
            size,
            fingerprint: claimed,
            content_generation: generation,
            blob: fref.hash,
            cipher_size: fref.cipher_size,
            tags: Vec::new(),
            description: String::new(),
        };
        let inner = seal(&files_key, generation, &community, &self.inner.me, BodyKind::Listing, &body)?;
        let payload = Payload::FileListing { category, blob: Some((fref.hash, fref.cipher_size)), inner };
        Ok(self.author_publish(community, ChannelId::CONTROL, payload).await?.event.id.to_string())
    }

    /// Checks that a finished listing download hashes to the fingerprint its listing claims.
    pub(crate) async fn check_listing_download(&self, message_id: &str, path: &std::path::Path) -> std::result::Result<(), dissent_files::FilesError> {
        let Some((community, listing)) = message_id
            .strip_prefix(TRANSFER_PREFIX)
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(c, l)| Some((c.parse::<CommunityId>().ok()?, l.parse::<EventId>().ok()?)))
        else {
            return Ok(());
        };
        let expected = self.opened_listings(community).into_iter().find(|o| o.id == listing).map(|o| o.body.fingerprint);
        let Some(expected) = expected else {
            return Err(dissent_files::FilesError::BadRef("the listing is gone".into()));
        };
        let owned = path.to_path_buf();
        let (actual, _) = tokio::task::spawn_blocking(move || dissent_files::tracker::fingerprint_file(&owned, |_| true))
            .await
            .map_err(|e| dissent_files::FilesError::Store(e.to_string()))??;
        if actual == expected {
            Ok(())
        } else {
            Err(dissent_files::FilesError::BadRef("the file does not match its listing's fingerprint".into()))
        }
    }

    /// Sets the server's reputation rules.
    pub async fn set_reputation_rules(&self, community: CommunityId, rules: ReputationRules) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::ReputationRulesSet { rules }).await?;
        Ok(())
    }

    pub async fn reset_reputation(&self, community: CommunityId, root: RootId) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::ReputationReset { root }).await?;
        Ok(())
    }
}

/// The author of a category's spec in force: the bodies are sealed with their author in the associated data.
fn category_author(st: &dissent_events::CommunityState, id: &CategoryId) -> RootId {
    st.files.categories.get(id).map_or(st.owner, |c| c.author)
}
