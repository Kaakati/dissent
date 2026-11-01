//! The server file tracker on the core side: the files key, sealed bodies, the catalog, adding and downloading. Replay
//! rules live in `dissent_events::tracker`. See `docs/design/14-file-tracker.md`.
//!
//! * **Files key.** Each generation is 32 random bytes. Its number is `(secret generation << 32) | random`, so two
//!   admins who make one at the same time make two distinct generations, never two keys under one number. The newest
//!   is the one with the highest number. Persons fetch every generation with `Request::FilesKeys`; storage members are
//!   never answered.
//! * **Content.** A file is encrypted under `dissent_files::tracker::content_key(files key, fingerprint)`, so the same
//!   bytes under the same generation always make the same blob.
//! * **Local book.** What this device keeps about the tracker, sealed with the vault next to the community's other
//!   files: linked folders, auto-import, what it seeds, and the secret its receipt tags are keyed with.

pub(crate) mod bodies;
pub mod adding;
pub mod catalog;
pub mod drops;
pub mod folders;
pub(crate) mod gate;
pub(crate) mod restricted;
pub(crate) mod scan;
pub mod seeding;
pub mod swarm;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use dissent_events::{CategoryId, EventId};
use dissent_identity::{CommunityId, DeviceId};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::membership::CommitRules;
use crate::rpc::{Request, Response};
use crate::store::Paths;

/// How long after one key fetch for a community the next may start.
const FETCH_GAP: Duration = Duration::from_secs(20);
/// How long one device has to answer `FilesKeys`.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// How long a file is seeded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeedLimit {
    #[default]
    Forever,
    /// Until this device's uploads of it reach this many thousandths of its size.
    Ratio(u32),
    /// For this many hours after it was added or downloaded.
    Hours(u32),
    /// Until this wall time, in ms.
    Until(u64),
}

/// One file this device seeds, by blob hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeedRecord {
    pub listing: EventId,
    /// The plaintext on disk this device listed or saved it from, if any.
    pub path: Option<PathBuf>,
    pub limit: SeedLimit,
    pub paused: bool,
    /// Stopped by hand. A seed whose limit is reached is stopped too, without this.
    pub stopped: bool,
    /// Seeded from `path` in place, with no copy in the store. See `dissent_files::Linked`.
    pub linked: bool,
    /// The linked file's modification time when it was linked, in ms. A file changed since is not served.
    pub modified_ms: Option<u64>,
    /// Ciphertext bytes this device served of it.
    pub uploaded: u64,
    pub since_ms: u64,
    /// True for a file this device downloaded, false for one it added.
    pub downloaded: bool,
}

/// What this device keeps about one community's tracker.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct LocalBook {
    /// Keys receipt tags, so only this device can link a receipt to a listing.
    pub receipt_secret: [u8; 32],
    pub folders: BTreeMap<CategoryId, PathBuf>,
    pub auto_import: BTreeSet<CategoryId>,
    pub seeds: BTreeMap<[u8; 32], SeedRecord>,
    /// The roots seen holding each blob whole, for its "downloaded completely" count.
    pub completed: BTreeMap<[u8; 32], BTreeSet<dissent_identity::RootId>>,
}

/// One community's tracker state on this device.
#[derive(Default)]
pub(crate) struct Local {
    /// Every key this device holds, by generation: the server's files key and the restricted categories' keys.
    pub keys: BTreeMap<u64, [u8; 32]>,
    /// The generations that belong to a restricted category's key, and to which. Every other generation is a files key.
    pub scopes: BTreeMap<u64, CategoryId>,
    pub book: LocalBook,
    loaded: bool,
    /// False when `files_keys.bin` is there and could not be read this run, so it must not be replaced.
    keys_writable: bool,
    /// The same for `tracker.bin`.
    book_writable: bool,
    /// `tracker.bin` could not be read, now or on an earlier start. The book may be missing seeds, so the unpin sweep
    /// keeps everything. See `Engine::tracker_book_damaged`.
    book_damaged: bool,
    last_fetch: Option<Instant>,
    /// What each device said it holds lately. In memory only.
    pub(crate) heard: BTreeMap<DeviceId, swarm::HeardHoldings>,
    pub(crate) holdings_asked: Option<Instant>,
    /// Paths a picker or a folder scan offered this run. Only these may be listed, so a page that was tricked into
    /// calling `list_files` cannot publish a file the person never chose. In memory only.
    pub(crate) offered: BTreeSet<PathBuf>,
    /// Paths to seed from where they are, with no copy, as a linked folder's files are. See `folders`. In memory only.
    pub(crate) in_place: BTreeSet<PathBuf>,
    /// This device's last holdings answer and when it was made, so a member asking in a loop costs one build per
    /// `HOLDINGS_CACHE`. In memory only.
    pub(crate) answer: Option<(Instant, Vec<swarm::Holding>)>,
    /// The seeds that are seeding now, for the blob gate. Set by every reconcile.
    pub(crate) serving: BTreeSet<[u8; 32]>,
    /// Upload counters moved since the book was saved.
    pub(crate) dirty: bool,
}

/// A file's modification time, in ms since the epoch.
pub(crate) fn modified_ms(path: &std::path::Path) -> Option<u64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Every community's tracker state, loaded on first use.
#[derive(Default)]
pub(crate) struct TrackerState {
    pub communities: BTreeMap<CommunityId, Local>,
    /// The upload cap's bucket, shared by every server. See `gate`.
    pub(crate) upload: gate::UploadBucket,
    /// The seeding settings as last read or saved.
    pub(crate) settings: Option<seeding::SeedingSettings>,
    /// Paths the person chose this run, through the OS picker or a native drop. See `adding`.
    pub(crate) allowed: BTreeSet<PathBuf>,
    /// The fingerprint checks running, by job, with what stops each.
    pub(crate) checks: BTreeMap<u64, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) next_check: u64,
}

/// One line of `files_keys.bin`: a generation, its key, and the restricted category it belongs to, if any.
type KeyEntry = (u64, [u8; 32], Option<CategoryId>);

fn keys_path(paths: &Paths, id: &CommunityId) -> PathBuf {
    paths.community_dir(id).join("files_keys.bin")
}

fn book_path(paths: &Paths, id: &CommunityId) -> PathBuf {
    paths.community_dir(id).join("tracker.bin")
}

impl Local {
    fn load(paths: &Paths, id: &CommunityId) -> Self {
        let keys = paths.read_guarded::<Vec<KeyEntry>>(&keys_path(paths, id));
        let book = paths.read_guarded::<LocalBook>(&book_path(paths, id));
        let found = keys.value.unwrap_or_default();
        Self {
            scopes: found.iter().filter_map(|(g, _, s)| s.map(|c| (*g, c))).collect(),
            keys: found.into_iter().map(|(g, k, _)| (g, k)).collect(),
            book: book.value.unwrap_or_else(|| LocalBook { receipt_secret: rand::random(), ..LocalBook::default() }),
            keys_writable: keys.may_write,
            book_writable: book.may_write,
            book_damaged: book.damaged,
            loaded: true,
            ..Self::default()
        }
    }

    /// The key file's contents, or `None` while the file could not be read and must not be replaced.
    fn keys_to_save(&self) -> Option<Vec<KeyEntry>> {
        self.keys_writable.then(|| self.keys.iter().map(|(g, k)| (*g, *k, self.scopes.get(g).copied())).collect())
    }

    /// The book to save, or `None` while the file could not be read and must not be replaced.
    fn book_to_save(&self) -> Option<LocalBook> {
        self.book_writable.then(|| self.book.clone())
    }

    /// The newest files-key generation this device holds, and its key.
    pub fn newest(&self) -> Option<(u64, [u8; 32])> {
        self.keys.iter().rev().find(|(g, _)| !self.scopes.contains_key(g)).map(|(g, k)| (*g, *k))
    }

    /// The newest key generation of restricted `category` this device holds, and its key.
    pub fn newest_of(&self, category: CategoryId) -> Option<(u64, [u8; 32])> {
        self.keys.iter().rev().find(|(g, _)| self.scopes.get(g) == Some(&category)).map(|(g, k)| (*g, *k))
    }

    /// The files-key generations, without the restricted categories' keys.
    pub fn files_pairs(&self) -> Vec<(u64, [u8; 32])> {
        self.keys.iter().filter(|(g, _)| !self.scopes.contains_key(g)).map(|(g, k)| (*g, *k)).collect()
    }
}

impl Engine {
    /// Runs `f` on `community`'s tracker state, loading it from disk on first use.
    pub(crate) fn with_tracker<T>(&self, community: CommunityId, f: impl FnOnce(&mut Local) -> T) -> T {
        let paths = self.read().paths.clone();
        let mut state = self.inner.tracker.lock().unwrap_or_else(PoisonError::into_inner);
        let local = state.communities.entry(community).or_default();
        if !local.loaded {
            *local = Local::load(&paths, &community);
        }
        f(local)
    }

    pub(crate) fn save_tracker_keys(&self, community: CommunityId) {
        let paths = self.read().paths.clone();
        let Some(keys) = self.with_tracker(community, |l| l.keys_to_save()) else {
            warn!(community = %community, "the files keys file could not be read, so it is not replaced");
            return;
        };
        if let Err(e) = paths.write_bin(&keys_path(&paths, &community), &keys) {
            warn!(community = %community, error = %e, "could not save the files keys");
        }
    }

    pub(crate) fn save_tracker_book(&self, community: CommunityId) {
        let paths = self.read().paths.clone();
        let Some(book) = self.with_tracker(community, |l| l.book_to_save()) else {
            warn!(community = %community, "the tracker book could not be read, so it is not replaced");
            return;
        };
        if let Err(e) = paths.write_bin(&book_path(&paths, &community), &book) {
            warn!(community = %community, error = %e, "could not save the tracker book");
        }
    }

    /// The highest secret generation a key generation may name: the community's current one. A key that names a later
    /// one is refused, so no member can plant a generation that outranks the one an admin makes at the next rotation.
    pub(crate) fn key_ceiling(&self, community: CommunityId) -> u64 {
        self.read().communities.get(&community).map_or(0, |c| c.state().secret_generation)
    }

    /// Stores files keys another device sent, keeping only generations that name no later secret generation than
    /// this device's replay. Returns whether any was new.
    pub(crate) fn remember_files_keys_from_peer(&self, community: CommunityId, pairs: Vec<(u64, [u8; 32])>) -> bool {
        let ceiling = self.key_ceiling(community);
        self.remember_files_keys(community, pairs.into_iter().filter(|(g, _)| g >> 32 <= ceiling).collect())
    }

    /// Stores files keys this device was given or made. Returns whether any was new.
    pub(crate) fn remember_files_keys(&self, community: CommunityId, pairs: Vec<(u64, [u8; 32])>) -> bool {
        let changed = self.with_tracker(community, |l| {
            let mut changed = false;
            for (generation, key) in pairs {
                if let std::collections::btree_map::Entry::Vacant(slot) = l.keys.entry(generation) {
                    slot.insert(key);
                    changed = true;
                }
            }
            changed
        });
        if changed {
            self.save_tracker_keys(community);
        }
        changed
    }

    /// The key of `generation`, if this device holds it.
    pub(crate) fn files_key(&self, community: CommunityId, generation: u64) -> Option<[u8; 32]> {
        self.with_tracker(community, |l| l.keys.get(&generation).copied())
    }

    /// The newest generation to seal new bodies under. A device that holds none asks its members first; a file
    /// manager that still has none makes one for the current secret generation.
    pub(crate) async fn newest_files_key(&self, community: CommunityId, may_create: bool) -> Result<(u64, [u8; 32])> {
        if let Some(newest) = self.with_tracker(community, |l| l.newest()) {
            return Ok(newest);
        }
        self.fetch_files_keys(community, true).await;
        if let Some(newest) = self.with_tracker(community, |l| l.newest()) {
            return Ok(newest);
        }
        if !may_create {
            return Err(CoreError::Invalid("no member has shared this server's files key with this device yet".into()));
        }
        Ok(self.make_files_key(community))
    }

    /// Makes a new files key generation for the current secret generation and keeps it.
    pub(crate) fn make_files_key(&self, community: CommunityId) -> (u64, [u8; 32]) {
        let secret_generation = self
            .read()
            .communities
            .get(&community)
            .map_or(0, |c| c.state().secret_generation);
        let generation = (secret_generation << 32) | u64::from(rand::random::<u32>());
        let key: [u8; 32] = rand::random();
        self.remember_files_keys(community, vec![(generation, key)]);
        (generation, key)
    }

    /// Asks member devices for the files keys this device lacks. At most once per `FETCH_GAP` unless `now`. Returns
    /// whether any key was new.
    pub(crate) async fn fetch_files_keys(&self, community: CommunityId, now: bool) -> bool {
        let due = self.with_tracker(community, |l| {
            let due = now || l.last_fetch.is_none_or(|at| at.elapsed() >= FETCH_GAP);
            if due {
                l.last_fetch = Some(Instant::now());
            }
            due
        });
        if !due {
            return false;
        }
        let devices: Vec<DeviceId> = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return false,
            }
        };
        let mut changed = false;
        for device in devices {
            let reply = tokio::time::timeout(ASK_WAIT, self.call(device, &Request::FilesKeys { community })).await;
            if let Ok(Ok(Response::FilesKeys(pairs))) = reply {
                changed |= self.remember_files_keys_from_peer(community, pairs);
            }
        }
        if changed {
            self.emit(crate::views::EngineEvent::CommunityUpdated { community_id: community.to_string() });
        }
        changed
    }

    /// Answers `FilesKeys`: every generation this device holds, for an active person only. Storage members, banned
    /// members and members who left get nothing.
    pub(crate) fn serve_files_keys(&self, from: DeviceId, community: CommunityId) -> Response {
        let person = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            w.directory
                .root_of(&from)
                .is_some_and(|root| CommitRules::of(c).is_person(&c.state().canonical(&root)) && !c.state().banned.contains(&root))
        };
        if !person {
            return Response::Error("files keys are for members".into());
        }
        Response::FilesKeys(self.with_tracker(community, |l| l.files_pairs()))
    }

    /// Accepts files keys a member pushed after a rotation. Only from an active person.
    pub(crate) fn accept_files_keys(&self, from: DeviceId, community: CommunityId, pairs: Vec<(u64, [u8; 32])>) -> Response {
        let person = {
            let w = self.read();
            w.communities.get(&community).is_some_and(|c| {
                w.directory.root_of(&from).is_some_and(|root| CommitRules::of(c).is_person(&c.state().canonical(&root)))
            })
        };
        if !person {
            return Response::Error("files keys come from members".into());
        }
        if self.remember_files_keys_from_peer(community, pairs) {
            self.emit(crate::views::EngineEvent::CommunityUpdated { community_id: community.to_string() });
        }
        Response::Ack
    }

    /// After this device rotated the community secret, makes a files key for the new generation and offers it to the
    /// persons who remain, so listings from now on are sealed under a key the departed never had.
    pub(crate) async fn rotate_files_key(&self, community: CommunityId) {
        let has_tracker = self
            .read()
            .communities
            .get(&community)
            .is_some_and(|c| !c.state().files.categories.is_empty());
        if !has_tracker {
            return;
        }
        let (generation, key) = self.make_files_key(community);
        let devices: Vec<DeviceId> = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => {
                    let rules = CommitRules::of(c);
                    w.member_devices(c)
                        .into_iter()
                        .filter(|d| w.directory.root_of(d).is_some_and(|r| rules.is_person(&r)))
                        .collect()
                }
                None => return,
            }
        };
        for device in devices {
            let push = Request::PushFilesKeys { community, pairs: vec![(generation, key)] };
            let _ = tokio::time::timeout(ASK_WAIT, self.call(device, &push)).await;
        }
        self.rotate_category_keys(community).await;
    }
}

#[cfg(test)]
mod sealing_tests {
    use super::*;
    use dissent_identity::Vault;

    fn profile(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-tracker-sealing-{tag}-{}-{}", std::process::id(), crate::engine::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn book() -> LocalBook {
        let folders = BTreeMap::from([(CategoryId([1; 32]), PathBuf::from("/home/me/secret-folder"))]);
        LocalBook { receipt_secret: [7; 32], folders, ..LocalBook::default() }
    }

    fn holds(raw: &[u8], needle: &[u8]) -> bool {
        raw.windows(needle.len()).any(|w| w == needle)
    }

    /// The local book and keys are written as sealed store files, and open again.
    #[test]
    fn the_book_and_the_keys_are_sealed_and_read_back() {
        let dir = profile("write");
        let paths = Paths::for_test(dir);
        let id = CommunityId([2; 32]);
        let keys: Vec<(u64, [u8; 32], Option<CategoryId>)> = vec![(1, [9; 32], None)];
        paths.write_bin(&keys_path(&paths, &id), &keys).unwrap();
        paths.write_bin(&book_path(&paths, &id), &book()).unwrap();
        let raw = std::fs::read(book_path(&paths, &id)).unwrap();
        assert!(raw.starts_with(crate::at_rest::FILE_MAGIC));
        assert!(!holds(&raw, b"secret-folder"));
        let raw = std::fs::read(keys_path(&paths, &id)).unwrap();
        assert!(raw.starts_with(crate::at_rest::FILE_MAGIC));
        assert!(!holds(&raw, &[9; 32]));
        let local = Local::load(&paths, &id);
        assert_eq!(local.keys.get(&1), Some(&[9; 32]));
        assert_eq!(local.book.receipt_secret, [7; 32]);
        assert!(local.book.folders.values().any(|p| p.ends_with("secret-folder")));
    }

    /// A book or key file from an earlier build's sealing is kept, and a good profile starts clean.
    #[test]
    fn files_that_do_not_read_are_kept_and_a_blocked_file_is_not_saved_over() {
        let dir = profile("guard");
        let paths = Paths::for_test(dir);
        let id = CommunityId([2; 32]);
        let fresh = Local::load(&paths, &id);
        assert!(fresh.keys_to_save().is_some() && fresh.book_to_save().is_some() && !fresh.book_damaged);

        let (keys, book) = (keys_path(&paths, &id), book_path(&paths, &id));
        std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
        // XChaCha output, as the earlier builds wrote it: a first byte of 0 would decode as an empty key list.
        let old_seal = [&[0u8][..], &[0x5a; 80]].concat();
        std::fs::write(&keys, &old_seal).unwrap();
        std::fs::write(&book, &old_seal).unwrap();
        let local = Local::load(&paths, &id);
        assert!(local.keys.is_empty() && local.book.seeds.is_empty() && local.book_damaged);
        assert_eq!(std::fs::read(keys.with_file_name("files_keys.bin.bad")).unwrap(), old_seal);
        assert_eq!(std::fs::read(book.with_file_name("tracker.bin.bad")).unwrap(), old_seal);
        assert!(local.keys_to_save().is_some(), "with the old file safe, new state may be saved");
        assert!(Local::load(&paths, &id).book_damaged, "a later start still knows the book was lost");

        let other = CommunityId([3; 32]);
        std::fs::create_dir_all(book_path(&paths, &other)).unwrap();
        std::fs::create_dir_all(keys_path(&paths, &other)).unwrap();
        let blocked = Local::load(&paths, &other);
        assert!(blocked.keys_to_save().is_none() && blocked.book_to_save().is_none() && blocked.book_damaged);
    }

    /// A migration seals a plaintext book, and a profile copy under a new vault carries it over.
    #[test]
    fn the_migration_and_a_profile_copy_cover_the_book() {
        let dir = profile("migrate");
        let id = CommunityId([2; 32]);
        let old = Vault::from_passphrase("old", &[3u8; 16]).unwrap();
        let new = Vault::from_passphrase("new", &[3u8; 16]).unwrap();
        let paths = Paths::sealed_by(dir.clone(), &old);
        let path = book_path(&paths, &id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, postcard::to_allocvec(&book()).unwrap()).unwrap();
        assert_eq!(paths.migrate().unwrap(), 1);
        assert!(!holds(&std::fs::read(&path).unwrap(), b"secret-folder"));
        crate::at_rest::reseal_tree(&dir, &old, &new).unwrap();
        let copied = Paths::sealed_by(dir, &new);
        assert_eq!(copied.read_bin::<LocalBook>(&path).unwrap().unwrap().receipt_secret, [7; 32]);
    }
}
