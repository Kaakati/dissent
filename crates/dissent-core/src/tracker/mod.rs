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
pub mod catalog;
pub(crate) mod restricted;
pub(crate) mod scan;
pub mod seeding;
pub mod swarm;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use dissent_events::{CategoryId, EventId};
use dissent_identity::{CommunityId, DeviceId, Vault};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::membership::CommitRules;
use crate::rpc::{Request, Response};
use crate::store::{write_bytes_atomic, Paths};

const KEYS_PURPOSE: &[u8] = b"dissent/files-keys/v1";
const BOOK_PURPOSE: &[u8] = b"dissent/tracker-book/v1";
const AAD_DOMAIN: &[u8] = b"dissent/tracker-local-aad/v1";
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
    last_fetch: Option<Instant>,
    /// What each device said it holds lately. In memory only.
    pub(crate) heard: BTreeMap<DeviceId, swarm::HeardHoldings>,
    pub(crate) holdings_asked: Option<Instant>,
    /// Paths a picker or a folder scan offered this run. Only these may be listed, so a page that was tricked into
    /// calling `list_files` cannot publish a file the person never chose. In memory only.
    pub(crate) offered: BTreeSet<PathBuf>,
    /// This device's last holdings answer and when it was made, so a member asking in a loop costs one build per
    /// `HOLDINGS_CACHE`. In memory only.
    pub(crate) answer: Option<(Instant, Vec<swarm::Holding>)>,
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
}

fn aad(id: &CommunityId, what: &[u8]) -> Vec<u8> {
    [AAD_DOMAIN, what, id.as_bytes()].concat()
}

fn keys_path(paths: &Paths, id: &CommunityId) -> PathBuf {
    paths.community_dir(id).join("files_keys.bin")
}

fn book_path(paths: &Paths, id: &CommunityId) -> PathBuf {
    paths.community_dir(id).join("tracker.bin")
}

fn read_sealed<T: for<'a> Deserialize<'a>>(vault: &Vault, path: &Path, aad: &[u8]) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(sealed) => {
            let plain = vault.open_sealed(aad, &sealed)?;
            Ok(Some(postcard::from_bytes(&plain).map_err(|e| CoreError::Encoding(e.to_string()))?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_sealed<T: Serialize>(vault: &Vault, path: &Path, aad: &[u8], value: &T) -> Result<()> {
    let plain = postcard::to_allocvec(value).map_err(|e| CoreError::Encoding(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_bytes_atomic(path, &vault.seal(aad, &plain))
}

impl Local {
    fn load(paths: &Paths, vault: &Vault, id: &CommunityId) -> Self {
        let keys: Vec<(u64, [u8; 32], Option<CategoryId>)> =
            read_sealed(&vault.subkey(KEYS_PURPOSE), &keys_path(paths, id), &aad(id, b"keys")).unwrap_or_else(|e| {
                warn!(community = %id, error = %e, "could not read the files keys");
                None
            }).unwrap_or_default();
        let scopes = keys.iter().filter_map(|(g, _, s)| s.map(|c| (*g, c))).collect();
        let book = read_sealed(&vault.subkey(BOOK_PURPOSE), &book_path(paths, id), &aad(id, b"book"))
            .unwrap_or_else(|e| {
                warn!(community = %id, error = %e, "could not read the tracker book");
                None
            })
            .unwrap_or_else(|| LocalBook { receipt_secret: rand::random(), ..LocalBook::default() });
        Self { keys: keys.into_iter().map(|(g, k, _)| (g, k)).collect(), scopes, book, loaded: true, ..Self::default() }
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
            *local = Local::load(&paths, &self.inner.vault, &community);
        }
        f(local)
    }

    pub(crate) fn save_tracker_keys(&self, community: CommunityId) {
        let paths = self.read().paths.clone();
        let keys: Vec<(u64, [u8; 32], Option<CategoryId>)> =
            self.with_tracker(community, |l| l.keys.iter().map(|(g, k)| (*g, *k, l.scopes.get(g).copied())).collect());
        let vault = self.inner.vault.subkey(KEYS_PURPOSE);
        if let Err(e) = write_sealed(&vault, &keys_path(&paths, &community), &aad(&community, b"keys"), &keys) {
            warn!(community = %community, error = %e, "could not save the files keys");
        }
    }

    pub(crate) fn save_tracker_book(&self, community: CommunityId) {
        let paths = self.read().paths.clone();
        let book = self.with_tracker(community, |l| l.book.clone());
        let vault = self.inner.vault.subkey(BOOK_PURPOSE);
        if let Err(e) = write_sealed(&vault, &book_path(&paths, &community), &aad(&community, b"book"), &book) {
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
