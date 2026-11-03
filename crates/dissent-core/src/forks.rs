//! Noticing that a community's MLS group forked: two members committed at one epoch, and the group
//! split into two branches with the same epoch number and different secrets. Nothing here changes
//! keys or refuses commits. A device that notices logs it at warn level once per epoch and reports
//! it in the community view (`CommunityView::mls_fork`). See `05-encrypted-servers.md`, "Fork
//! detection: build plan".
//!
//! Two signals:
//! - A: a commit announcement for the epoch this device is at, naming a commit other than the one
//!   this device entered it by (`Engine::check_commit_at_our_epoch`). It needs no new wire.
//! - B: a member's signed epoch check (`SignedEpochCheck`), sent after each commit, on a slow
//!   timer, and in answer to a check that disagrees. It catches a fork a device was offline for.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::PoisonError;
use std::time::Instant;

use dissent_identity::{CommunityId, DeviceId};
use tracing::{debug, warn};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::views::{EngineEvent, ForkView};
use crate::wire::{encode_gossip, GossipMsg, SignedEpochCheck};

/// The MLS exporter label of the epoch check. Exporter outputs under different labels are independent
/// (RFC 9420, section 8.5), so this reveals nothing about the call, history or control keys, which use
/// their own labels; `exporter_labels_are_distinct` pins that no two uses share one.
pub(crate) const EPOCH_CHECK_LABEL: &str = "dissent/epoch-check/v1";
/// Bytes of exporter output in a check. Two branches agree on all 16 only by chance, at 2^-128.
const CHECK_LEN: usize = 16;

/// What this device knows about one community's epoch checks.
#[derive(Default)]
pub(crate) struct ForkWatch {
    /// The newest check heard from each member device that was ahead of this device, to compare once
    /// this device reaches that epoch. At most one per member device.
    ahead: BTreeMap<DeviceId, SignedEpochCheck>,
    fork: Option<Fork>,
    /// Epochs a fork was already logged at, so each is logged once.
    warned: BTreeSet<u64>,
    /// When this device last sent a check here, and whether a later one is waiting for the gap.
    last_sent: Option<Instant>,
    pending: bool,
    /// Checks this device has sent here, for tests of the rate.
    sent: u64,
}

/// A fork this device noticed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fork {
    /// The epoch both branches claim.
    pub epoch: u64,
    /// Devices heard on the other branch.
    pub other_side: BTreeSet<DeviceId>,
    pub since_ms: u64,
}

impl Engine {
    fn lock_forks(&self) -> std::sync::MutexGuard<'_, BTreeMap<CommunityId, ForkWatch>> {
        self.inner.forks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This device's epoch in `community` and its check value, if it is in the group.
    pub(crate) fn own_epoch_check(&self, community: CommunityId) -> Option<(u64, [u8; CHECK_LEN])> {
        self.with_mls_read(|mls| -> dissent_mls::Result<(u64, Vec<u8>)> {
            Ok((mls.epoch(&community.0)?, mls.export_secret(&community.0, EPOCH_CHECK_LABEL, CHECK_LEN)?))
        })
        .ok()
        .and_then(|(epoch, check)| Some((epoch, check.try_into().ok()?)))
    }

    /// Signal A. `announced` is the hash of a commit that a member says entered `epoch`, the epoch this
    /// device is at. If this device entered it by another commit, the announcer is on another branch.
    /// Without a logged commit for the epoch (this device joined at it), it says nothing.
    pub(crate) fn check_commit_at_our_epoch(&self, community: CommunityId, epoch: u64, announced: &[u8; 32], committer: DeviceId) {
        let Some(ours) = self.commit_at(community, epoch) else {
            return;
        };
        if blake3::hash(&ours).as_bytes() != announced {
            self.note_fork(community, epoch, committer, "a commit announcement for our epoch names another commit");
        }
    }

    /// Signal B: a member's check, from the community topic.
    pub(crate) async fn accept_epoch_check(&self, via: CommunityId, check: SignedEpochCheck) -> Result<()> {
        check.verify()?;
        if check.community != via {
            return Err(CoreError::Invalid("an epoch check for another community".into()));
        }
        let community = check.community;
        if check.device == self.inner.identity.device_id() {
            return Ok(());
        }
        // A device outside the group, a removed one included, says nothing about it.
        let member = self
            .with_mls_read(|mls| mls.members(&community.0))
            .is_ok_and(|members| members.iter().any(|c| c.device == check.device));
        if !member {
            debug!(community = %community, device = %check.device, "epoch check from a device outside the group");
            return Ok(());
        }
        let Some((epoch, ours)) = self.own_epoch_check(community) else {
            return Ok(());
        };
        if check.epoch > epoch {
            let mut forks = self.lock_forks();
            let ahead = &mut forks.entry(community).or_default().ahead;
            if ahead.get(&check.device).is_none_or(|old| old.ts_ms < check.ts_ms) {
                ahead.insert(check.device, check);
            }
            return Ok(());
        }
        if check.epoch < epoch {
            // That device is catching up.
            return Ok(());
        }
        if check.check == ours {
            self.note_agreement(community, check.device);
        } else {
            self.note_fork(community, epoch, check.device, "a member's epoch check disagrees with ours");
            // Tell that device too: it learns of the fork from our check.
            self.send_epoch_check(community).await;
        }
        Ok(())
    }

    /// After this device's epoch changed: compare the checks that were waiting for this epoch, and send
    /// its own.
    pub(crate) async fn after_epoch_change(&self, community: CommunityId) {
        if let Some((epoch, ours)) = self.own_epoch_check(community) {
            let due: Vec<SignedEpochCheck> = {
                let mut forks = self.lock_forks();
                let watch = forks.entry(community).or_default();
                let due = watch.ahead.values().filter(|c| c.epoch == epoch).cloned().collect();
                watch.ahead.retain(|_, c| c.epoch > epoch);
                due
            };
            for check in due {
                if check.check == ours {
                    self.note_agreement(community, check.device);
                } else {
                    self.note_fork(community, epoch, check.device, "a member's epoch check disagrees with ours");
                }
            }
        }
        self.send_epoch_check(community).await;
    }

    /// The maintenance pass: a check every `Timings::epoch_check_every`, so a device that was away hears
    /// one from each branch even when nobody commits.
    pub(crate) async fn epoch_check_if_due(&self, community: CommunityId) {
        let due = {
            let forks = self.lock_forks();
            forks
                .get(&community)
                .and_then(|w| w.last_sent)
                .is_none_or(|at| at.elapsed() >= self.inner.timings.epoch_check_every)
        };
        if due {
            self.send_epoch_check(community).await;
        }
    }

    /// Sends this device's check on the community topic, at most once per `Timings::epoch_check_gap`. A
    /// check asked for sooner goes out when the gap ends, with the epoch then current.
    pub(crate) async fn send_epoch_check(&self, community: CommunityId) {
        let gap = self.inner.timings.epoch_check_gap;
        let wait = {
            let mut forks = self.lock_forks();
            let watch = forks.entry(community).or_default();
            match watch.last_sent.map(|at| at.elapsed()) {
                Some(since) if since < gap => {
                    if watch.pending {
                        return;
                    }
                    watch.pending = true;
                    Some(gap - since)
                }
                _ => {
                    watch.last_sent = Some(Instant::now());
                    None
                }
            }
        };
        if let Some(wait) = wait {
            let engine = self.clone();
            self.spawn_task(async move {
                tokio::time::sleep(wait).await;
                {
                    let mut forks = engine.lock_forks();
                    let watch = forks.entry(community).or_default();
                    watch.pending = false;
                    watch.last_sent = Some(Instant::now());
                }
                engine.broadcast_epoch_check(community).await;
            });
            return;
        }
        self.broadcast_epoch_check(community).await;
    }

    async fn broadcast_epoch_check(&self, community: CommunityId) {
        let Some((epoch, check)) = self.own_epoch_check(community) else {
            return;
        };
        let sender = self.read().communities.get(&community).and_then(|c| c.sender.clone());
        let Some(sender) = sender else {
            return;
        };
        let signed = match SignedEpochCheck::sign(community, epoch, check, crate::engine::now_ms(), &self.inner.identity.device) {
            Ok(signed) => signed,
            Err(e) => {
                warn!(community = %community, error = %e, "could not sign an epoch check");
                return;
            }
        };
        match encode_gossip(&GossipMsg::EpochCheck(Box::new(signed))) {
            Ok(encoded) => {
                if let Err(e) = sender.broadcast(encoded).await {
                    debug!(community = %community, error = %e, "epoch check not sent");
                }
                if let Some(watch) = self.lock_forks().get_mut(&community) {
                    watch.sent += 1;
                }
            }
            Err(e) => warn!(community = %community, error = %e, "could not encode an epoch check"),
        }
    }

    fn note_fork(&self, community: CommunityId, epoch: u64, device: DeviceId, why: &str) {
        let first_at_epoch = {
            let mut forks = self.lock_forks();
            let watch = forks.entry(community).or_default();
            match watch.fork.as_mut() {
                Some(fork) if fork.epoch == epoch => {
                    fork.other_side.insert(device);
                }
                _ => {
                    watch.fork = Some(Fork {
                        epoch,
                        other_side: BTreeSet::from([device]),
                        since_ms: crate::engine::now_ms(),
                    })
                }
            }
            watch.warned.insert(epoch)
        };
        if first_at_epoch {
            warn!(community = %community, epoch, device = %device, why, "the MLS group forked");
        }
        self.emit(EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
    }

    /// A device on the other side of a fork now agrees with this one, after a recovery or after it
    /// caught up. The fork is over once every device heard on the other side agrees.
    fn note_agreement(&self, community: CommunityId, device: DeviceId) {
        let ended = {
            let mut forks = self.lock_forks();
            let Some(watch) = forks.get_mut(&community) else {
                return;
            };
            let Some(fork) = watch.fork.as_mut() else {
                return;
            };
            if !fork.other_side.remove(&device) || !fork.other_side.is_empty() {
                return;
            }
            watch.fork = None;
            true
        };
        if ended {
            self.emit(EngineEvent::CommunityUpdated {
                community_id: community.to_string(),
            });
        }
    }

    /// The fork this device noticed in `community`, for the community view, with the other side as root ids.
    pub(crate) fn fork_view(&self, community: CommunityId) -> Option<ForkView> {
        let fork = self.lock_forks().get(&community)?.fork.clone()?;
        let members = self.with_mls_read(|mls| mls.members(&community.0)).unwrap_or_default();
        let roots: BTreeSet<String> = fork
            .other_side
            .iter()
            .filter_map(|d| members.iter().find(|c| c.device == *d).map(|c| c.root.to_string()))
            .collect();
        Some(ForkView {
            epoch: fork.epoch,
            roots: roots.into_iter().collect(),
        })
    }

    /// Checks this device has sent in `community`, for tests.
    pub(crate) fn epoch_checks_sent(&self, community: CommunityId) -> u64 {
        self.lock_forks().get(&community).map_or(0, |w| w.sent)
    }

    /// The fork this device noticed, with the other side's devices, for tests.
    pub(crate) fn fork_state(&self, community: CommunityId) -> Option<(u64, BTreeSet<DeviceId>)> {
        self.lock_forks().get(&community)?.fork.as_ref().map(|f| (f.epoch, f.other_side.clone()))
    }
}

#[cfg(test)]
mod net_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every MLS exporter label this workspace derives a secret under, with its owner. Two uses under one
    /// label would derive the same secret, so a check would leak a call or history key. A new
    /// `export_secret` call must add its label here; `every_export_secret_call_uses_a_listed_label` fails
    /// until it does.
    const LABELS: [(&str, &str); 4] = [
        ("SFrame 1.0 Base Key", "calls::KEY_LABEL"),
        (dissent_events::control::EXPORT_LABEL, "epoch_keys, control"),
        ("dissent/history/v1", "history::LABEL"),
        (EPOCH_CHECK_LABEL, "forks::EPOCH_CHECK_LABEL"),
    ];

    #[test]
    fn exporter_labels_are_distinct() {
        let labels: BTreeSet<&str> = LABELS.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels.len(), LABELS.len(), "two exporter uses share a label: {LABELS:?}");
        assert_eq!(dissent_events::control::EXPORT_LABEL, "dissent/control/v1");
    }

    /// Reads this crate's sources: every `export_secret(` call outside tests names one of the label constants
    /// above, so a new exporter use cannot pick a label silently.
    #[test]
    fn every_export_secret_call_uses_a_listed_label() {
        let known = ["KEY_LABEL", "EXPORT_LABEL", "LABEL", "EPOCH_CHECK_LABEL"];
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut calls = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if !name.ends_with(".rs") || name.contains("tests") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read file");
                // Only the code before a file's own test module.
                let code = text.split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
                for (i, _) in code.match_indices(".export_secret(") {
                    let args = &code[i..code[i..].find(')').map_or(code.len(), |e| i + e)];
                    let label = args.split(',').nth(1).map(str::trim).unwrap_or_default();
                    assert!(known.contains(&label), "{}: export_secret with label {label:?}", path.display());
                    calls += 1;
                }
            }
        }
        assert!(calls >= 4, "found the exporter calls: {calls}");
    }
}
