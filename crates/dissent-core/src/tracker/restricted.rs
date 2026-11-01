//! Restricted categories: a category visible to some members only has its own key per generation. Its spec, its
//! listings' fields and its files are sealed and encrypted under that key, and its listings carry no blob in clear, so
//! a member who is not a reader learns only that a restricted category exists, and storage seeders never mirror it.
//! See `docs/design/14-file-tracker.md`, "Keys".
//!
//! Its keys live in the same ring as the files key, by generation; `Local::scopes` says which category each belongs
//! to. A generation is made before the category's create event, whose id names the category, so it is filed under a
//! placeholder until that event is published.

use dissent_events::CategoryId;
use dissent_identity::{CommunityId, DeviceId};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::membership::CommitRules;
use crate::rpc::{Request, Response};

/// The scope a category key is filed under until its category's create event is published.
pub(crate) const PENDING: CategoryId = CategoryId([0; 32]);

impl Engine {
    /// Stores keys of `category`. A generation this device already holds is kept as it is. Returns whether any was new.
    pub(crate) fn remember_category_keys(&self, community: CommunityId, category: CategoryId, pairs: Vec<(u64, [u8; 32])>) -> bool {
        let changed = self.with_tracker(community, |l| {
            let mut changed = false;
            for (generation, key) in pairs {
                if let std::collections::btree_map::Entry::Vacant(slot) = l.keys.entry(generation) {
                    slot.insert(key);
                    l.scopes.insert(generation, category);
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

    /// The spec revision a key generation of `category` names: its middle 16 bits.
    fn revision_of(generation: u64) -> u64 {
        (generation >> 16) & 0xffff
    }

    /// Stores category keys another device sent, keeping only generations that name no later secret generation and no
    /// later spec revision than this device's replay. A reader who is about to be removed therefore cannot plant a
    /// generation that outranks the one made when the spec drops them. Returns whether any was new.
    pub(crate) fn remember_category_keys_from_peer(&self, community: CommunityId, category: CategoryId, pairs: Vec<(u64, [u8; 32])>) -> bool {
        let ceiling = self.key_ceiling(community);
        let revision = self.read().communities.get(&community).and_then(|c| c.state().files.categories.get(&category).map(|s| u64::from(s.revision)));
        let Some(revision) = revision else {
            return false;
        };
        let kept = pairs.into_iter().filter(|(g, _)| g >> 32 <= ceiling && Self::revision_of(*g) <= revision.min(0xffff)).collect();
        self.remember_category_keys(community, category, kept)
    }

    /// Makes a key generation for a restricted category, filed under `category` (or `PENDING` before its id exists).
    /// It names the current secret generation and the spec revision it will be used from: the next one for an existing
    /// category, whose update is about to be published, and 0 for a new one.
    pub(crate) fn make_category_key(&self, community: CommunityId, category: CategoryId) -> (u64, [u8; 32]) {
        let (secret_generation, revision) = self.read().communities.get(&community).map_or((0, 0), |c| {
            let next = c.state().files.categories.get(&category).map_or(0, |s| u64::from(s.revision) + 1);
            (c.state().secret_generation, next.min(0xffff))
        });
        let generation = (secret_generation << 32) | (revision << 16) | u64::from(rand::random::<u16>());
        let key: [u8; 32] = rand::random();
        self.remember_category_keys(community, category, vec![(generation, key)]);
        (generation, key)
    }

    /// Files a pending generation under the category its create event made.
    pub(crate) fn assign_scope(&self, community: CommunityId, generation: u64, category: CategoryId) {
        self.with_tracker(community, |l| {
            l.scopes.insert(generation, category);
        });
        self.save_tracker_keys(community);
    }

    /// The newest key of `category`, asking its readers when this device holds none.
    pub(crate) async fn newest_category_key(&self, community: CommunityId, category: CategoryId) -> Result<(u64, [u8; 32])> {
        if let Some(newest) = self.with_tracker(community, |l| l.newest_of(category)) {
            return Ok(newest);
        }
        self.fetch_category_keys(community, category).await;
        self.with_tracker(community, |l| l.newest_of(category))
            .ok_or_else(|| CoreError::Invalid("no reader has shared this category's key with this device yet".into()))
    }

    /// The member devices whose person may see `category`, other than this device.
    pub(crate) fn reader_devices_of(&self, community: CommunityId, category: CategoryId) -> Vec<DeviceId> {
        self.reader_devices(community, category)
    }

    fn reader_devices(&self, community: CommunityId, category: CategoryId) -> Vec<DeviceId> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let rules = CommitRules::of(c);
        w.member_devices(c)
            .into_iter()
            .filter(|d| {
                w.directory
                    .root_of(d)
                    .is_some_and(|r| rules.is_person(&c.state().canonical(&r)) && c.state().may_see_files(&r, &category))
            })
            .collect()
    }

    /// Asks the category's readers for its keys. Returns whether any was new.
    pub(crate) async fn fetch_category_keys(&self, community: CommunityId, category: CategoryId) -> bool {
        let mut changed = false;
        for device in self.reader_devices(community, category) {
            let ask = Request::CategoryKeys { community, category };
            if let Ok(Ok(Response::CategoryKeys(pairs))) = tokio::time::timeout(std::time::Duration::from_secs(5), self.call(device, &ask)).await {
                changed |= self.remember_category_keys_from_peer(community, category, pairs);
            }
        }
        if changed {
            self.emit(crate::views::EngineEvent::CommunityUpdated { community_id: community.to_string() });
        }
        changed
    }

    /// Answers `CategoryKeys`: the category's generations, for a person who may see it in this device's replay.
    pub(crate) fn serve_category_keys(&self, from: DeviceId, community: CommunityId, category: CategoryId) -> Response {
        let reader = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            w.directory.root_of(&from).is_some_and(|root| {
                let root = c.state().canonical(&root);
                CommitRules::of(c).is_person(&root) && c.state().may_see_files(&root, &category)
            })
        };
        if !reader {
            return Response::Error("not a reader of this category".into());
        }
        Response::CategoryKeys(self.with_tracker(community, |l| {
            l.keys.iter().filter(|(g, _)| l.scopes.get(g) == Some(&category)).map(|(g, k)| (*g, *k)).collect()
        }))
    }

    /// Accepts category keys a reader pushed after making a new generation. Only from a person who may see it.
    pub(crate) fn accept_category_keys(&self, from: DeviceId, community: CommunityId, category: CategoryId, pairs: Vec<(u64, [u8; 32])>) -> Response {
        let reader = {
            let w = self.read();
            w.communities.get(&community).is_some_and(|c| {
                w.directory.root_of(&from).is_some_and(|root| {
                    let root = c.state().canonical(&root);
                    CommitRules::of(c).is_person(&root) && c.state().may_see_files(&root, &category)
                })
            })
        };
        if !reader {
            return Response::Error("category keys come from its readers".into());
        }
        if self.remember_category_keys_from_peer(community, category, pairs) {
            self.emit(crate::views::EngineEvent::CommunityUpdated { community_id: community.to_string() });
        }
        Response::Ack
    }

    /// Makes a new generation of `category` and offers it to its readers, so what is listed from now on is sealed under
    /// a key a removed reader never held.
    pub(crate) async fn rotate_category_key(&self, community: CommunityId, category: CategoryId) -> (u64, [u8; 32]) {
        let (generation, key) = self.make_category_key(community, category);
        for device in self.reader_devices(community, category) {
            let push = Request::PushCategoryKeys { community, category, pairs: vec![(generation, key)] };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.call(device, &push)).await;
        }
        (generation, key)
    }

    /// Rotates every restricted category's key this device may see. Called after this device rotated the secret.
    pub(crate) async fn rotate_category_keys(&self, community: CommunityId) {
        let me = self.inner.me;
        let categories: Vec<CategoryId> = self
            .read()
            .communities
            .get(&community)
            .map(|c| {
                c.state()
                    .files
                    .categories
                    .iter()
                    .filter(|(id, cat)| cat.visible.is_some() && c.state().may_see_files(&me, id))
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        for category in categories {
            self.rotate_category_key(community, category).await;
        }
    }
}
