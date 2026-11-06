//! The server file tracker's events: categories, listings, receipts and reputation. Replay sees only the outer layer of
//! each event; names, tags, fingerprints and descriptions are inside a `FilesSealed` under the server's files key,
//! which replay never opens. See `docs/design/14-file-tracker.md`, "Events".

use std::collections::{BTreeMap, BTreeSet};

use dissent_identity::RootId;
use serde::{Deserialize, Serialize};

use crate::event::digest_id;
use crate::roles::{Perms, RoleId};
use crate::state::{Rank, Rejection};
use crate::{ChannelId, CommunityState, Event, EventId, Hlc, Payload};

digest_id!(CategoryId, "A file category: the id of the `FileCategorySet` event that made it.");

/// Bytes sealed under the files key of `generation`. Replay keeps them for the view and never opens them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesSealed {
    pub generation: u64,
    pub ciphertext: Vec<u8>,
}

/// Who may add files to a category. Any one path admits: a rank of at least `min_rank` (0 member, 1 moderator,
/// 2 admin, 3 owner, 4 no rank path), a listed role, being listed, or a reputation level of at least `min_level`
/// (0: no level path). Holders of `MANAGE_FILES` always may.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSpec {
    pub min_rank: u8,
    pub roles: Vec<RoleId>,
    pub members: Vec<RootId>,
    pub min_level: u8,
}

/// Who may see a restricted category, in the same shape as `AddSpec`. `None` on a category means every member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderSpec {
    pub min_rank: u8,
    pub roles: Vec<RoleId>,
    pub members: Vec<RootId>,
}

/// One reputation level: its name, and the credited bytes and distinct members it takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelRule {
    pub name: String,
    pub bytes: u64,
    pub members: u32,
}

/// The server's reputation rules. Level 1 is the first rule, which every member meets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReputationRules {
    pub levels: Vec<LevelRule>,
    /// Receipts from the same member add nothing after the first.
    pub distinct: bool,
    /// Credit halves every `decay_days` without a new receipt. 0 is no decay.
    pub decay_days: u16,
    /// Whether admins may reset a member's reputation.
    pub admins_reset: bool,
}

const GB: u64 = 1_000_000_000;

impl Default for ReputationRules {
    /// The levels the design shows: Newcomer, Sharer, Seeder, Keeper, Archivist.
    fn default() -> Self {
        let level = |name: &str, gb: u64, members: u32| LevelRule { name: name.into(), bytes: gb * GB, members };
        Self {
            levels: vec![
                level("Newcomer", 0, 0),
                level("Sharer", 5, 5),
                level("Seeder", 25, 15),
                level("Keeper", 100, 40),
                level("Archivist", 250, 80),
            ],
            distinct: true,
            decay_days: 30,
            admins_reset: true,
        }
    }
}

/// Most bytes of sealed fields one tracker event may carry.
pub const MAX_SEALED_BYTES: usize = 16 * 1024;
/// Most roles or members an `AddSpec` or `ReaderSpec` may list.
pub const MAX_SPEC_ENTRIES: usize = 256;
/// Most seeders one receipt may credit.
pub const MAX_RECEIPT_CREDITS: usize = 64;
/// Most bytes one receipt may credit in total: the ciphertext of the largest file the format holds (1 TiB of plaintext
/// and one 16-byte tag per 64 KiB chunk), rounded up to 1 TiB and 1 GiB. Releases up to v0.1.35 refuse anything over
/// 11 GiB, so a receipt or a listing past that is accepted only by later ones; those releases cannot fetch such a file.
pub const MAX_RECEIPT_BYTES: u64 = (1 << 40) + (1 << 30);
/// Most ciphertext bytes a listing may declare.
pub const MAX_LISTING_BYTES: u64 = MAX_RECEIPT_BYTES;
/// Most reputation levels.
pub const MAX_LEVELS: usize = 10;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// A category as replay knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryState {
    /// HLC of the spec in force.
    pub hlc: Hlc,
    pub created: Hlc,
    /// Who wrote the spec in force. Its sealed body names them in its associated data.
    pub author: RootId,
    /// How many accepted `FileCategorySet` events changed it after it was made. A restricted category's key
    /// generations carry it, so a key made before a reader was removed always sorts before the one made after.
    pub revision: u32,
    pub add: AddSpec,
    pub visible: Option<ReaderSpec>,
    pub inner: FilesSealed,
}

/// A listing as replay knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingState {
    pub category: CategoryId,
    pub lister: RootId,
    pub hlc: Hlc,
    /// Blob hash and ciphertext size, for an everyone-visible category.
    pub blob: Option<([u8; 32], u64)>,
    pub inner: FilesSealed,
    /// The latest `FileListingUpdate`'s sealed fields, with its HLC.
    pub update: Option<(Hlc, FilesSealed)>,
}

/// One seeder's credit from receipts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub bytes: u64,
    /// Distinct downloaders who credited this seeder.
    pub from: BTreeSet<RootId>,
    /// Wall time of the latest receipt that credited this seeder.
    pub last_ms: u64,
}

/// The tracker's replayed state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesBook {
    pub categories: BTreeMap<CategoryId, CategoryState>,
    pub deleted: BTreeSet<CategoryId>,
    /// Live listings. A removed one is dropped.
    pub listings: BTreeMap<EventId, ListingState>,
    pub removed: BTreeSet<EventId>,
    /// (downloader, tag) pairs already counted.
    pub receipts: BTreeSet<(RootId, [u8; 16])>,
    pub credits: BTreeMap<RootId, Credit>,
    /// The latest `ReputationRules`, with its HLC. `None` means the defaults.
    pub rules: Option<(Hlc, ReputationRules)>,
}

impl FilesBook {
    pub fn rules(&self) -> ReputationRules {
        self.rules.as_ref().map(|(_, r)| r.clone()).unwrap_or_default()
    }

    /// The bytes credited to `root` as of `now_ms`, after decay.
    pub fn credited_bytes(&self, root: &RootId, now_ms: u64) -> u64 {
        let Some(credit) = self.credits.get(root) else {
            return 0;
        };
        let decay = u64::from(self.rules().decay_days) * DAY_MS;
        if decay == 0 {
            return credit.bytes;
        }
        let halvings = now_ms.saturating_sub(credit.last_ms) / decay;
        credit.bytes.checked_shr(halvings.min(64) as u32).unwrap_or(0)
    }

    /// The distinct members who credited `root`.
    pub fn served_members(&self, root: &RootId) -> u32 {
        self.credits.get(root).map_or(0, |c| c.from.len() as u32)
    }

    /// `root`'s reputation level as of `now_ms`, from 1. A server with no levels gives 1.
    pub fn level_of(&self, root: &RootId, now_ms: u64) -> u8 {
        let bytes = self.credited_bytes(root, now_ms);
        let members = self.served_members(root);
        let rules = self.rules();
        let reached = rules.levels.iter().take_while(|l| bytes >= l.bytes && members >= l.members).count();
        reached.max(1) as u8
    }
}

fn rank_number(rank: Rank) -> u8 {
    match rank {
        Rank::User => 0,
        Rank::Moderator => 1,
        Rank::Admin => 2,
        Rank::Owner => 3,
    }
}

/// `min_rank` that no rank meets, so only the other paths admit.
pub const NO_RANK: u8 = 4;

fn spec_ok(roles: &[RoleId], members: &[RootId], min_rank: u8) -> bool {
    roles.len() <= MAX_SPEC_ENTRIES && members.len() <= MAX_SPEC_ENTRIES && min_rank <= NO_RANK
}

fn sealed_ok(inner: &FilesSealed) -> bool {
    !inner.ciphertext.is_empty() && inner.ciphertext.len() <= MAX_SEALED_BYTES
}

impl CommunityState {
    /// Whether `root` may add files to `category` at wall time `now_ms`.
    pub fn may_add_files(&self, root: &RootId, category: &CategoryId, now_ms: u64) -> bool {
        let root = self.canonical(root);
        let Some(cat) = self.files.categories.get(category) else {
            return false;
        };
        let add = &cat.add;
        let held = self.custom.roles_of(&root);
        rank_number(self.rank(&root)) >= add.min_rank
            || add.roles.iter().any(|r| held.contains(r))
            || add.members.contains(&root)
            || (add.min_level > 0 && self.files.level_of(&root, now_ms) >= add.min_level)
            || self.perms_of(&root).contains(Perms::MANAGE_FILES)
    }

    /// Whether `root` may see `category`: every member, unless the category names its readers.
    pub fn may_see_files(&self, root: &RootId, category: &CategoryId) -> bool {
        let root = self.canonical(root);
        let Some(cat) = self.files.categories.get(category) else {
            return false;
        };
        let Some(spec) = &cat.visible else {
            return true;
        };
        let held = self.custom.roles_of(&root);
        rank_number(self.rank(&root)) >= spec.min_rank
            || spec.roles.iter().any(|r| held.contains(r))
            || spec.members.contains(&root)
            || self.perms_of(&root).contains(Perms::MANAGE_FILES)
    }

    /// Checks a tracker payload.
    pub(crate) fn check_files(&self, ev: &Event) -> Result<(), Rejection> {
        let actor = ev.body.author;
        if ev.body.channel != ChannelId::CONTROL {
            return Err(Rejection::Malformed);
        }
        let manage = self.perms_of(&actor).contains(Perms::MANAGE_FILES);
        let listing_owner = |listing: &EventId| match self.files.listings.get(listing) {
            Some(l) if l.lister == self.canonical(&actor) || manage => Ok(()),
            Some(_) => Err(Rejection::NoPermission),
            None => Err(Rejection::UnknownTarget),
        };
        match &ev.body.payload {
            Payload::FileCategorySet { category, add, visible, inner } => {
                if !manage {
                    return Err(Rejection::NoPermission);
                }
                if let Some(id) = category {
                    if !self.files.categories.contains_key(id) {
                        return Err(Rejection::UnknownTarget);
                    }
                }
                let visible_ok = visible.as_ref().is_none_or(|v| spec_ok(&v.roles, &v.members, v.min_rank));
                if !spec_ok(&add.roles, &add.members, add.min_rank) || !visible_ok || !sealed_ok(inner) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::FileCategoryDelete { category } => {
                if !manage {
                    return Err(Rejection::NoPermission);
                }
                if !self.files.categories.contains_key(category) {
                    return Err(Rejection::UnknownTarget);
                }
            }
            Payload::FileListing { category, blob, inner } => {
                let Some(cat) = self.files.categories.get(category) else {
                    return Err(Rejection::UnknownTarget);
                };
                if !self.may_add_files(&actor, category, ev.body.hlc.wall_ms) {
                    return Err(Rejection::NoPermission);
                }
                // A restricted category keeps its blobs out of the outer layer, so storage seeders never mirror them.
                if blob.is_some() == cat.visible.is_some() {
                    return Err(Rejection::Malformed);
                }
                if blob.is_some_and(|(_, size)| size == 0 || size > MAX_LISTING_BYTES) || !sealed_ok(inner) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::FileListingUpdate { listing, inner } => {
                listing_owner(listing)?;
                if !sealed_ok(inner) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::FileListingRemove { listing } => listing_owner(listing)?,
            Payload::FileReceipt { tag, credits } => {
                let total = credits.iter().try_fold(0u64, |sum, (_, b)| sum.checked_add(*b));
                if credits.is_empty()
                    || credits.len() > MAX_RECEIPT_CREDITS
                    || total.is_none_or(|t| t == 0 || t > MAX_RECEIPT_BYTES)
                    || self.files.receipts.contains(&(self.canonical(&actor), *tag))
                {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ReputationRulesSet { rules } => {
                if !manage {
                    return Err(Rejection::NoPermission);
                }
                let names_ok = rules.levels.iter().all(|l| crate::roles::valid_role_name(&l.name));
                if rules.levels.is_empty() || rules.levels.len() > MAX_LEVELS || !names_ok {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ReputationReset { .. } => {
                if !manage || !self.files.rules().admins_reset {
                    return Err(Rejection::NoPermission);
                }
            }
            _ => unreachable!("check_files takes tracker payloads only"),
        }
        Ok(())
    }

    /// Applies a tracker payload that `check_files` accepted.
    pub(crate) fn commit_files(&mut self, ev: &Event) {
        let hlc = ev.body.hlc;
        let actor = self.canonical(&ev.body.author);
        match &ev.body.payload {
            Payload::FileCategorySet { category, add, visible, inner } => match category {
                None => {
                    let state = CategoryState {
                        hlc,
                        created: hlc,
                        author: actor,
                        revision: 0,
                        add: add.clone(),
                        visible: visible.clone(),
                        inner: inner.clone(),
                    };
                    self.files.categories.insert(CategoryId(ev.id.0), state);
                }
                Some(id) => {
                    if let Some(cat) = self.files.categories.get_mut(id) {
                        if hlc > cat.hlc {
                            cat.hlc = hlc;
                            cat.author = actor;
                            cat.revision = cat.revision.saturating_add(1);
                            cat.add = add.clone();
                            cat.visible = visible.clone();
                            cat.inner = inner.clone();
                        }
                    }
                }
            },
            Payload::FileCategoryDelete { category } => {
                self.files.categories.remove(category);
                self.files.deleted.insert(*category);
                self.files.listings.retain(|_, l| l.category != *category);
            }
            Payload::FileListing { category, blob, inner } => {
                let state = ListingState { category: *category, lister: actor, hlc, blob: *blob, inner: inner.clone(), update: None };
                self.files.listings.insert(ev.id, state);
            }
            Payload::FileListingUpdate { listing, inner } => {
                if let Some(l) = self.files.listings.get_mut(listing) {
                    if l.update.as_ref().is_none_or(|(at, _)| hlc > *at) {
                        l.update = Some((hlc, inner.clone()));
                    }
                }
            }
            Payload::FileListingRemove { listing } => {
                self.files.listings.remove(listing);
                self.files.removed.insert(*listing);
            }
            Payload::FileReceipt { tag, credits } => {
                self.files.receipts.insert((actor, *tag));
                let distinct = self.files.rules().distinct;
                for (seeder, bytes) in credits {
                    let seeder = self.canonical(seeder);
                    if seeder == actor {
                        continue;
                    }
                    let credit = self.files.credits.entry(seeder).or_default();
                    // With `distinct`, a downloader's bytes count only on its first receipt for this seeder.
                    if !distinct || credit.from.insert(actor) {
                        credit.bytes = credit.bytes.saturating_add(*bytes);
                    }
                    credit.from.insert(actor);
                    credit.last_ms = credit.last_ms.max(hlc.wall_ms);
                }
            }
            Payload::ReputationRulesSet { rules } => {
                if self.files.rules.as_ref().is_none_or(|(at, _)| hlc > *at) {
                    self.files.rules = Some((hlc, rules.clone()));
                }
            }
            Payload::ReputationReset { root } => {
                self.files.credits.remove(&self.canonical(root));
            }
            _ => unreachable!("commit_files takes tracker payloads only"),
        }
    }

    pub(crate) fn migrate_files(&mut self, old: RootId, new: RootId) {
        if let Some(credit) = self.files.credits.remove(&old) {
            self.files.credits.insert(new, credit);
        }
        for listing in self.files.listings.values_mut() {
            if listing.lister == old {
                listing.lister = new;
            }
        }
    }
}

/// Whether `payload` is one of the tracker payloads.
pub fn is_files_payload(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::FileCategorySet { .. }
            | Payload::FileCategoryDelete { .. }
            | Payload::FileListing { .. }
            | Payload::FileListingUpdate { .. }
            | Payload::FileListingRemove { .. }
            | Payload::FileReceipt { .. }
            | Payload::ReputationRulesSet { .. }
            | Payload::ReputationReset { .. }
    )
}
