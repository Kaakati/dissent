//! Who may change a community's MLS group, and how the changes reach members.
//! See `docs/design/05-encrypted-servers.md`.
//!
//! Any member may change the group, but only within the rules the DAG sets. An add must bring in
//! a device of a current member. A removal must take out a device of a banned or departed member,
//! or one of the author's own. A welcome may list only current members. Every peer checks these
//! before it merges a commit, so a malicious member gains nothing from breaking them.
//!
//! Rank (owner, then admins, moderators and members by join time) only sets how long a device
//! waits before it acts, so that the highest-ranked online member usually commits first.
//!
//! Commits travel as signed announcements on the community topic. The bytes come over RPC and are
//! checked against the announced hash. Joins and removals live in `membership/join.rs`.

use std::collections::BTreeSet;
use std::sync::PoisonError;
use std::time::Duration;

use dissent_events::{Hlc, Role};
use dissent_identity::{CommunityId, DeviceId, RootId};
use dissent_mls::{CommitInfo, MlsError, WelcomeInfo};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::community::Community;
use crate::engine::Engine;
use crate::rpc::{Request, Response};
use crate::store::{append_records_plain, read_records_plain};
use crate::wire::{encode_gossip, GossipMsg, SignedCommit};

mod join;
#[cfg(test)]
mod policy_tests;

/// Delay before acting, for each higher-ranked member that is online.
pub(crate) const COMMIT_STEP: Duration = Duration::from_secs(3);
/// Refused commits kept per community, oldest dropped first.
const MAX_DEFERRED_COMMITS: usize = 16;
/// Longest a device waits before acting on a membership change.
pub(crate) const COMMIT_DELAY_CAP: Duration = Duration::from_secs(15);

/// One line of `mls_commits.log`.
#[derive(Serialize, Deserialize)]
pub(crate) struct CommitRecord {
    /// Epoch after the commit was applied.
    pub epoch: u64,
    pub bytes: Vec<u8>,
}

/// Owner first, then admins, moderators and members, each group by join time. Banned and
/// departed members are left out.
pub(crate) fn preference_order(c: &Community) -> Vec<RootId> {
    let state = c.state();
    let owner = c.owner();
    let mut ranked: Vec<(u8, Hlc, RootId)> = state
        .members
        .iter()
        .filter(|(root, m)| !m.left && !m.storage && !state.banned.contains(root) && **root != owner)
        .map(|(root, m)| {
            let tier = match state.roles.get(root) {
                Some(roles) if roles.contains(&Role::Admin) => 0,
                Some(roles) if roles.contains(&Role::Moderator) => 1,
                _ => 2,
            };
            (tier, m.joined, *root)
        })
        .collect();
    ranked.sort();
    let mut order = vec![owner];
    order.extend(ranked.into_iter().map(|(_, _, root)| root));
    order
}

/// What a commit may change, as this device's replayed DAG shows it.
#[derive(Default)]
pub(crate) struct CommitRules {
    /// Members who have neither left nor been banned.
    active: BTreeSet<RootId>,
    /// Members who have left, and banned members.
    departed: BTreeSet<RootId>,
    /// Current members who are people, not storage. Only these commit, and only these join the group.
    persons: BTreeSet<RootId>,
}

impl CommitRules {
    pub(crate) fn of(c: &Community) -> Self {
        let state = c.state();
        let active = state
            .members
            .iter()
            .filter(|(root, m)| !m.left && !state.banned.contains(root))
            .map(|(root, _)| *root)
            .collect();
        let persons = state
            .members
            .iter()
            .filter(|(root, m)| !m.left && !m.storage && !state.banned.contains(root))
            .map(|(root, _)| *root)
            .collect();
        let departed = state
            .members
            .iter()
            .filter(|(root, m)| m.left || state.banned.contains(root))
            .map(|(root, _)| *root)
            .chain(state.banned.iter().copied())
            .collect();
        Self { active, departed, persons }
    }

    /// The author must be a current member. Added devices must belong to current members.
    /// Removed devices must belong to departed members, or to the author.
    pub(crate) fn allows_commit(&self, info: &CommitInfo) -> bool {
        self.persons.contains(&info.committer.root)
            && info.adds.iter().all(|d| self.persons.contains(&d.root))
            && info
                .removes
                .iter()
                .all(|d| self.departed.contains(&d.root) || d.root == info.committer.root)
    }

    /// A welcome may list only current members.
    pub(crate) fn allows_welcome(&self, info: &WelcomeInfo) -> bool {
        info.members.iter().all(|m| self.active.contains(&m.root))
    }

    pub(crate) fn is_active(&self, root: &RootId) -> bool {
        self.active.contains(root)
    }

    pub(crate) fn is_person(&self, root: &RootId) -> bool {
        self.persons.contains(root)
    }

    pub(crate) fn is_departed(&self, root: &RootId) -> bool {
        self.departed.contains(root)
    }
}

/// How long a device waits before acting on a membership change: a step for each higher-ranked
/// member that is online, capped. Zero when no higher-ranked member is online.
pub(crate) fn delay_for(ahead: u32) -> Duration {
    COMMIT_STEP.saturating_mul(ahead).min(COMMIT_DELAY_CAP)
}

impl Engine {
    /// Whether this device is a storage member of `community`. Storage devices never join the group
    /// and never commit.
    pub(crate) fn is_storage_here(&self, community: CommunityId) -> bool {
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().members.get(&self.inner.me).is_some_and(|m| m.storage))
    }

    pub(crate) fn commit_rules(&self, community: CommunityId) -> CommitRules {
        let w = self.read();
        w.communities
            .get(&community)
            .map(CommitRules::of)
            .unwrap_or_default()
    }

    /// Higher-ranked members of `community` with a device online in this device's view.
    pub(crate) fn commit_delay(&self, community: CommunityId) -> Duration {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Duration::ZERO;
        };
        let mut online: BTreeSet<DeviceId> = self
            .inner
            .neighbours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&community)
            .cloned()
            .unwrap_or_default();
        online.insert(self.inner.identity.device_id());
        let mut ahead = 0;
        for root in preference_order(c) {
            if root == self.inner.me {
                break;
            }
            if w.directory
                .devices_of(&root)
                .iter()
                .any(|d| online.contains(d))
            {
                ahead += 1;
            }
        }
        delay_for(ahead)
    }

    /// Records an applied commit so others can catch up from us.
    fn log_commit(&self, community: CommunityId, epoch: u64, bytes: Vec<u8>) {
        let path = self.read().paths.commit_log(&community);
        if let Err(e) = append_records_plain(&path, &[CommitRecord { epoch, bytes }]) {
            tracing::warn!(community = %community, error = %e, "could not log MLS commit");
        }
    }

    /// Commits after `after`, in epoch order.
    pub(crate) fn commits_after(&self, community: CommunityId, after: u64) -> Vec<Vec<u8>> {
        self.commit_records(community)
            .into_iter()
            .filter(|r| r.epoch > after)
            .map(|r| r.bytes)
            .collect()
    }

    /// The commit that entered `epoch`, if we logged it.
    pub(crate) fn commit_at(&self, community: CommunityId, epoch: u64) -> Option<Vec<u8>> {
        self.commit_records(community)
            .into_iter()
            .find(|r| r.epoch == epoch)
            .map(|r| r.bytes)
    }

    fn commit_records(&self, community: CommunityId) -> Vec<CommitRecord> {
        let path = self.read().paths.commit_log(&community);
        read_records_plain(&path).unwrap_or_default()
    }

    /// Keeps a refused commit, to retry once the events behind it have arrived.
    fn defer_commit(&self, community: CommunityId, bytes: Vec<u8>) {
        let mut deferred = self
            .inner
            .deferred_commits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let list = deferred.entry(community).or_default();
        if list.len() >= MAX_DEFERRED_COMMITS {
            list.remove(0);
        }
        list.push(bytes);
    }

    /// Retries deferred commits. Applying one can unblock the next, so passes repeat while one
    /// applies. Commits still refused stay deferred, and unusable ones are dropped.
    pub(crate) fn retry_deferred_commits(&self, community: CommunityId) {
        let mut any_applied = false;
        loop {
            let pending = self
                .inner
                .deferred_commits
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&community)
                .unwrap_or_default();
            if pending.is_empty() {
                break;
            }
            let mut applied = false;
            let mut still = Vec::new();
            for bytes in pending {
                match self.apply_commit(community, &bytes) {
                    Ok(true) => applied = true,
                    Err(MlsError::Rejected(_)) => still.push(bytes),
                    Ok(false) | Err(_) => {}
                }
            }
            {
                let mut deferred = self
                    .inner
                    .deferred_commits
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let list = deferred.entry(community).or_default();
                still.append(list);
                *list = still;
            }
            any_applied |= applied;
            if !applied {
                break;
            }
        }
        if any_applied {
            self.unseal_pending(community);
        }
    }

    /// Logs a commit we just made, and announces it to the community topic.
    pub(crate) async fn publish_commit(&self, community: CommunityId, bytes: Vec<u8>) {
        let Some(epoch) = self.mls_epoch(community) else {
            return;
        };
        self.log_commit(community, epoch, bytes.clone());
        self.broadcast_commit(community, epoch, &bytes).await;
    }

    /// Sends a signed announcement of a commit to every neighbour on the community topic.
    async fn broadcast_commit(&self, community: CommunityId, epoch: u64, bytes: &[u8]) {
        let sender = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.sender.clone());
        let Some(sender) = sender else {
            return;
        };
        let signed = match SignedCommit::sign(
            community,
            epoch,
            bytes,
            self.inner.identity.device_id(),
            &self.inner.identity.device,
        ) {
            Ok(signed) => signed,
            Err(e) => {
                tracing::warn!(community = %community, error = %e, "could not sign commit");
                return;
            }
        };
        match encode_gossip(&GossipMsg::Commit(Box::new(signed))) {
            Ok(encoded) => {
                if let Err(e) = sender.broadcast(encoded).await {
                    debug!(community = %community, error = %e, "commit not announced");
                }
            }
            Err(e) => tracing::warn!(community = %community, error = %e, "could not encode commit"),
        }
    }

    /// Applies a commit. Returns `Ok(false)` when it does not apply to us (no group, or we were
    /// removed by it). A commit that breaks the membership rules is refused with an error and
    /// logged at warn level.
    pub(crate) fn apply_commit(
        &self,
        community: CommunityId,
        bytes: &[u8],
    ) -> std::result::Result<bool, MlsError> {
        let group = community.0;
        let rules = self.commit_rules(community);
        let mut added = Vec::new();
        let processed = self.with_mls(|mls| {
            mls.process_commit_with_policy(&group, bytes, |info| {
                added = info.adds.iter().map(|d| d.device).collect();
                let ok = rules.allows_commit(info);
                if !ok {
                    tracing::warn!(
                        community = %community,
                        committer = %info.committer.device,
                        "rejected an MLS commit that breaks the membership rules"
                    );
                }
                ok
            })
        })?;
        if !matches!(processed, dissent_mls::Processed::Commit) {
            return Ok(false);
        }
        match self.mls_epoch(community) {
            Some(epoch) => {
                self.log_commit(community, epoch, bytes.to_vec());
                self.note_epoch(community, &added);
                Ok(true)
            }
            // The commit removed this device, so there is no group left to follow.
            None => Ok(false),
        }
    }

    /// Handles a signed commit announced by a neighbour. The next epoch is fetched and applied.
    /// A later one triggers a catch-up from its committer.
    pub(crate) async fn receive_commit(
        &self,
        community: CommunityId,
        neighbour: DeviceId,
        announce: SignedCommit,
    ) {
        if let Err(e) = announce.verify() {
            debug!(community = %community, error = %e, "rejected commit announcement");
            return;
        }
        let Some(ours) = self.mls_epoch(community) else {
            return;
        };
        if announce.epoch <= ours {
            return;
        }
        if announce.epoch > ours + 1 {
            self.mls_catch_up(community, announce.committer).await;
            return;
        }
        let Some(bytes) = self
            .fetch_commit(community, &announce, &[announce.committer, neighbour])
            .await
        else {
            self.mls_catch_up(community, neighbour).await;
            return;
        };
        match self.apply_commit(community, &bytes) {
            Ok(true) => self.unseal_pending(community),
            Ok(false) => {}
            // Refused for now. The ban or invite behind it may still be on its way, so keep it.
            Err(MlsError::Rejected(_)) => self.defer_commit(community, bytes),
            Err(MlsError::UnknownGroup) => {}
            Err(_) => self.mls_catch_up(community, announce.committer).await,
        }
    }

    /// Fetches the bytes of an announced commit from the first peer that has them and matches
    /// the announced hash.
    async fn fetch_commit(
        &self,
        community: CommunityId,
        announce: &SignedCommit,
        peers: &[DeviceId],
    ) -> Option<Vec<u8>> {
        for peer in peers {
            let request = Request::MlsCommit {
                community,
                epoch: announce.epoch,
            };
            if let Ok(Response::MlsCommit(Some(bytes))) = self.call(*peer, &request).await {
                if announce.matches(&bytes) {
                    return Some(bytes);
                }
            }
        }
        None
    }

    /// Fetches and applies the commits this device missed, from `peer`, then decrypts what they
    /// unlock.
    pub(crate) async fn mls_catch_up(&self, community: CommunityId, peer: DeviceId) {
        self.mls_catch_up_commits(community, peer).await;
        self.unseal_pending(community);
    }

    /// Asks `peer` for commits we missed. Used on a timer, so a lost announcement is repaired
    /// without waiting for a message. Decrypts only when something applied.
    pub(crate) async fn mls_commit_check(&self, community: CommunityId, peer: DeviceId) {
        if self.mls_catch_up_commits(community, peer).await {
            self.unseal_pending(community);
        }
    }

    /// Fetches and applies missed commits from `peer`, in epoch order. Stops at the first one that
    /// does not apply. Returns whether any applied.
    pub(crate) async fn mls_catch_up_commits(&self, community: CommunityId, peer: DeviceId) -> bool {
        let Some(after) = self.mls_epoch(community) else {
            return false;
        };
        let commits = match self
            .call(peer, &Request::MlsCommits { community, after })
            .await
        {
            Ok(Response::MlsCommits(commits)) => commits,
            _ => return false,
        };
        let mut applied = false;
        for bytes in commits {
            if !matches!(self.apply_commit(community, &bytes), Ok(true)) {
                break;
            }
            applied = true;
        }
        applied
    }

    /// Per-pass MLS work: join groups we are missing, and remove departed devices.
    pub(crate) async fn mls_maintenance(&self) {
        let communities: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for community in communities {
            self.mls_join_missing(community).await;
            self.mls_reconcile(community).await;
            self.rotate_if_due(community).await;
            self.refresh_secret(community).await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod storage_tests;
