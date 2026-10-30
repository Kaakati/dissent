//! Hints to seeders. The member that sends a message's thumbnails and preview pictures tells the community's seeders
//! the hash and size of each blob; a member that fetches one does not, so seeders never learn who fetched what. A seeder then keeps the ciphertext, within its `MirrorPolicy`, so a member
//! who joins later can fetch it while the sender is offline. A hint carries what a cleartext manifest already
//! carries for attachments. A seeder never learns a key.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_identity::{CommunityId, DeviceId, RootId};
use tracing::debug;

use crate::engine::Engine;
use crate::rpc::{Request, Response};

/// Hints one requester device may send a seeder within `HINT_WINDOW`.
const HINTS_PER_WINDOW: usize = 30;
const HINT_WINDOW: Duration = Duration::from_secs(60);
/// Blobs one hint may name. A message has a few thumbnails and previews at most.
pub(crate) const MAX_BLOBS_PER_HINT: usize = 8;
const ASK_WAIT: Duration = Duration::from_secs(10);

/// How often each device has hinted this seeder.
#[derive(Default)]
pub(crate) struct HintLimits {
    asks: Mutex<BTreeMap<DeviceId, VecDeque<Instant>>>,
}

impl HintLimits {
    fn admit(&self, from: DeviceId, now: Instant) -> bool {
        let mut asks = self.asks.lock().unwrap_or_else(PoisonError::into_inner);
        let times = asks.entry(from).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= HINT_WINDOW) {
            times.pop_front();
        }
        if times.len() >= HINTS_PER_WINDOW {
            return false;
        }
        times.push_back(now);
        true
    }
}

impl Engine {
    /// Tells the community's seeders about `blobs`, in the background. A seeder that does not answer is skipped.
    pub(crate) fn hint_seeders(&self, community: CommunityId, blobs: Vec<([u8; 32], u64)>) {
        if blobs.is_empty() {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            engine.send_hints(community, blobs).await;
        });
    }

    async fn send_hints(&self, community: CommunityId, blobs: Vec<([u8; 32], u64)>) {
        let seeders: Vec<DeviceId> = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            c.state()
                .members
                .iter()
                .filter(|(root, m)| m.storage && !m.left && **root != w.me)
                .flat_map(|(root, _)| w.directory.devices_of(root))
                .collect()
        };
        let blobs: Vec<([u8; 32], u64)> = blobs.into_iter().take(MAX_BLOBS_PER_HINT).collect();
        let req = Request::MirrorHint { community, blobs };
        for device in seeders {
            match tokio::time::timeout(ASK_WAIT, self.call(device, &req)).await {
                Ok(Ok(Response::Ack)) => {}
                other => debug!(community = %community, peer = %device, reply = ?other, "a seeder did not take the hint"),
            }
        }
    }

    /// Answers `MirrorHint`. A seeder accepts hints only from members of the community, within its mirror policy and
    /// the per-device limit. It fetches each blob's ciphertext from the members who may hold it, in the background.
    pub(crate) fn serve_mirror_hint(
        &self,
        from: DeviceId,
        community: CommunityId,
        blobs: Vec<([u8; 32], u64)>,
    ) -> Response {
        let (max_file_size, max_total, roots) = {
            let w = self.read();
            let Some(root) = w.directory.root_of(&from) else {
                return Response::Error("unknown device".into());
            };
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            if !c.is_member(&root) {
                return Response::Error("not a member".into());
            }
            let Some((max_file_size, max_total)) = w.attach.mirror else {
                return Response::Ack;
            };
            let roots: Vec<RootId> = c.state().members.keys().copied().collect();
            (max_file_size, max_total, roots)
        };
        if !self.inner.mirror_hints.admit(from, Instant::now()) {
            return Response::Refused("too many hints; try again in a minute".into());
        }
        let providers: Vec<_> = self.providers_for(&roots).iter().filter_map(|p| p.to_iroh().ok()).collect();
        let kept: Vec<[u8; 32]> = blobs.iter().take(MAX_BLOBS_PER_HINT).map(|(hash, _)| *hash).collect();
        // A seeder cannot read the message that names these, so the hint is what tells it who may fetch them.
        self.add_hinted_blobs(community, &kept);
        for (hash, size) in blobs.into_iter().take(MAX_BLOBS_PER_HINT) {
            if size == 0 || size > max_file_size {
                continue;
            }
            let engine = self.clone();
            let providers = providers.clone();
            self.spawn_task(async move {
                let node = engine.node();
                let policy = dissent_files::MirrorPolicy { max_file_size, max_total };
                if let Err(e) = engine
                    .inner
                    .files
                    .mirror(node.endpoint(), &providers, hash, size, policy, |_, _| {})
                    .await
                {
                    debug!(error = %e, "could not keep a hinted blob");
                }
            });
        }
        Response::Ack
    }
}
