//! Storage grants, phase 1 (`docs/design/05-encrypted-servers.md`, "Storage joins: a known gap").
//!
//! A seeder joins as storage with no invite check, so a departed member could come back as storage
//! under a fresh root. The fix is an admin-signed `Payload::StorageGrant { root, granted }`:
//!
//! - After a storage join, the seeder asks the server's owner and admins for a grant
//!   (`Request::StorageGrantAsk`), and asks again with a growing pause until it has one.
//! - An admin's device keeps the ask, and the member list shows each seeder as granted, asked or
//!   neither. An admin grants it (`Engine::grant_storage`), which publishes the grant as carried
//!   control, so an older client shows it as locked.
//! - Phase 1 only issues, carries and shows grants. A storage join does not need one yet. Phase 2
//!   makes replay refuse an ungranted storage join, once released seeders have asked for grants.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

use dissent_events::{ChannelId, Payload, Role};
use dissent_identity::{CommunityId, DeviceId, RootId};
use tracing::debug;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::views::EngineEvent;

/// The first pause before a seeder asks again for a grant it has not got.
const ASK_FIRST: Duration = Duration::from_secs(5);
/// The longest pause between two asks.
const ASK_MOST: Duration = Duration::from_secs(600);
/// How long one ask waits for an answer.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// The seeders that asked this device for a grant, by community. Kept in memory: a seeder asks again.
#[derive(Default)]
pub(crate) struct StorageAsks(Mutex<BTreeMap<CommunityId, BTreeSet<RootId>>>);

impl StorageAsks {
    fn note(&self, community: CommunityId, root: RootId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(community)
            .or_default()
            .insert(root)
    }

    pub(crate) fn of(&self, community: &CommunityId) -> BTreeSet<RootId> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(community)
            .cloned()
            .unwrap_or_default()
    }
}

impl Engine {
    /// Grants `root` storage in `community`, or withdraws the grant. Admins and the owner only, which
    /// replay enforces.
    pub async fn grant_storage(&self, community: CommunityId, root: RootId, granted: bool) -> Result<()> {
        if !self.read().communities.contains_key(&community) {
            return Err(CoreError::NotFound(format!("community {community}")));
        }
        self.author_publish(community, ChannelId::CONTROL, Payload::StorageGrant { root, granted })
            .await?;
        self.emit(EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

    /// The seeders that asked this device for a grant in `community` and have none yet, as base32
    /// root ids. Only an owner or admin device holds asks.
    pub fn storage_grant_asks(&self, community: CommunityId) -> Vec<String> {
        self.inner
            .storage_asks
            .of(&community)
            .into_iter()
            .filter(|root| !self.storage_granted(community, root))
            .map(|root| root.to_string())
            .collect()
    }

    /// Whether an admin has granted `root` storage in `community`.
    pub fn storage_granted(&self, community: CommunityId, root: &RootId) -> bool {
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().storage_granted(root))
    }

    /// Whether this device's person holds a storage grant in `community`. For a seeder's status.
    pub fn holds_storage_grant(&self, community: CommunityId) -> bool {
        self.storage_granted(community, &self.inner.me)
    }

    /// The devices of `community`'s owner and admins, who can grant storage.
    fn granters(&self, community: CommunityId) -> Vec<DeviceId> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let me = self.inner.identity.device_id();
        let mut roots: Vec<RootId> = vec![c.owner()];
        roots.extend(
            c.state()
                .roles
                .iter()
                .filter(|(_, roles)| roles.contains(&Role::Admin))
                .map(|(root, _)| *root),
        );
        roots
            .iter()
            .flat_map(|root| w.directory.devices_of(root))
            .filter(|device| *device != me)
            .collect()
    }

    /// Whether this device is a storage member of `community` that has no grant yet.
    fn wants_grant(&self, community: CommunityId) -> bool {
        let w = self.read();
        w.communities.get(&community).is_some_and(|c| {
            let me = self.inner.me;
            c.state().members.get(&me).is_some_and(|m| m.storage && !m.left) && !c.state().storage_granted(&me)
        })
    }

    /// On a seeder: asks the owner and admins of `community` for a storage grant until one is
    /// published, with a pause that doubles from `ASK_FIRST` up to `ASK_MOST`. Runs beside the engine.
    pub(crate) fn spawn_grant_asks(&self, community: CommunityId) {
        let engine = self.clone();
        self.spawn_task(async move {
            let mut pause = ASK_FIRST;
            while engine.wants_grant(community) {
                for device in engine.granters(community) {
                    let ask = Request::StorageGrantAsk { community };
                    match tokio::time::timeout(ASK_WAIT, engine.call(device, &ask)).await {
                        Ok(Ok(Response::Ack)) => {}
                        Ok(Ok(other)) => debug!(device = %device, reply = ?other, "storage grant ask refused"),
                        Ok(Err(e)) => debug!(device = %device, error = %e, "storage grant ask not delivered"),
                        Err(_) => debug!(device = %device, "storage grant ask timed out"),
                    }
                }
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(ASK_MOST);
            }
        });
    }

    /// On start: keeps asking for every community where this device is storage without a grant.
    pub(crate) fn resume_grant_asks(&self) {
        let ids: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for id in ids.into_iter().filter(|id| self.wants_grant(*id)) {
            self.spawn_grant_asks(id);
        }
    }

    /// Answers a seeder's `StorageGrantAsk`. It counts only from a current, unbanned storage member
    /// without a grant, and only on a device that can grant, which keeps it for the member list.
    pub(crate) fn answer_storage_ask(&self, from: DeviceId, community: CommunityId) -> Response {
        let noted: Option<(String, String, RootId)> = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            let Some(root) = w.directory.root_of(&from) else {
                return Response::Error("unknown device".into());
            };
            let storage = c.state().members.get(&root).is_some_and(|m| m.storage && !m.left);
            if !storage || c.state().banned.contains(&root) {
                return Response::Error("not a storage member".into());
            }
            let me = self.inner.me;
            let granter = c.owner() == me
                || c.state().roles.get(&me).is_some_and(|roles| roles.contains(&Role::Admin));
            if !granter || c.state().storage_granted(&root) {
                return Response::Ack;
            }
            self.inner
                .storage_asks
                .note(community, root)
                .then(|| (c.name().to_string(), w.display_name(&root), root))
        };
        if let Some((community_name, display_name, root)) = noted {
            self.emit(EngineEvent::CommunityUpdated {
                community_id: community.to_string(),
            });
            self.emit(EngineEvent::StorageGrantAsked {
                community_id: community.to_string(),
                community_name,
                root_id: root.to_string(),
                display_name,
            });
        }
        Response::Ack
    }
}
