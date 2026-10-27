//! Control keys of each MLS epoch. A member-sealed control event is opened with the encryption key
//! of its epoch. Every device exports that key from its MLS group when it enters the epoch, and
//! keeps it, so it can still replay the epochs it was in. Storage members are never in the group,
//! so they hold the write keys only: those check tags, and cannot open anything.
//!
//! Keys are kept sealed under the vault. Members hand the keys of older epochs to a new member,
//! as far as the community's history setting allows. The owner turns member-only control on once
//! every member device says it supports it. See `docs/design/05-encrypted-servers.md` and
//! `docs/design/06-threat-model.md`, finding 2.

use dissent_events::{ChannelId, EpochKey, EpochKeys, HistorySetting, Payload, EXPORT_LABEL};
use dissent_identity::{CommunityId, DeviceId, Vault};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::membership::CommitRules;
use crate::net::{control_effects, ControlEffects};
use crate::rpc::{Request, Response};
use crate::store::{write_bytes_atomic, Paths};
use crate::world::World;

const PURPOSE: &[u8] = b"dissent/epoch-keys/v1";
const AAD_DOMAIN: &[u8] = b"dissent/epoch-keys-aad/v1";
/// Members asked for their epoch keys when joining. The first answers are merged.
const HISTORY_FROM: usize = 3;
/// The wait doubles after each attempt that no member answers, up to `HISTORY_RETRY_CAP`. A member
/// coming online restarts it at once. The first wait is `Timings::history_first_retry`, 1 s in production,
/// as in `history` for invite-card keys.
const HISTORY_RETRY_CAP: std::time::Duration = std::time::Duration::from_secs(60);

/// The wait after a failed attempt that follows `wait`: double it, up to the cap.
fn history_retry_after(wait: std::time::Duration) -> std::time::Duration {
    (wait * 2).min(HISTORY_RETRY_CAP)
}

/// The epoch keys one device holds for one community.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct EpochStore {
    /// Keys of each epoch this device has been in, oldest first.
    pub keys: Vec<(u64, EpochKeys)>,
    /// Write keys of the same epochs, oldest first. Storage members hold these without `keys`.
    pub writes: Vec<EpochKey>,
    /// The epoch each device entered the group at, from the commit that added it.
    pub joins: Vec<(DeviceId, u64)>,
}

impl EpochStore {
    /// Encryption keys, for replay.
    pub(crate) fn enc_ring(&self) -> Vec<EpochKey> {
        self.keys.iter().map(|(e, k)| (*e, k.enc)).collect()
    }

    /// Write keys, for checking tags.
    pub(crate) fn write_ring(&self) -> Vec<EpochKey> {
        self.writes.clone()
    }

    /// The epoch `device` entered the group at, if this device saw the commit that added it.
    pub(crate) fn join_epoch(&self, device: &DeviceId) -> Option<u64> {
        self.joins
            .iter()
            .find(|(d, _)| d == device)
            .map(|(_, epoch)| *epoch)
    }

    /// Records an epoch's keys and the devices its commit added. Returns whether anything is new.
    pub(crate) fn record(&mut self, epoch: u64, keys: EpochKeys, joined: &[DeviceId]) -> bool {
        let mut changed = false;
        if !self.keys.iter().any(|(e, _)| *e == epoch) {
            self.writes.push((epoch, keys.write));
            self.keys.push((epoch, keys));
            self.keys.sort_by_key(|(e, _)| *e);
            self.writes.sort_by_key(|(e, _)| *e);
            changed = true;
        }
        for device in joined {
            if self.join_epoch(device).is_none() {
                self.joins.push((*device, epoch));
                changed = true;
            }
        }
        changed
    }

    /// Adds epoch keys a member sent. An epoch this device already has keeps its keys. Returns
    /// whether anything is new.
    pub(crate) fn merge(&mut self, pairs: &[(u64, [u8; 32], [u8; 32])]) -> bool {
        let mut changed = false;
        for (epoch, enc, write) in pairs {
            if self.keys.iter().any(|(e, _)| e == epoch) {
                continue;
            }
            self.keys.push((
                *epoch,
                EpochKeys {
                    enc: *enc,
                    write: *write,
                },
            ));
            changed = true;
        }
        self.keys.sort_by_key(|(e, _)| *e);
        changed
    }

    /// Adds write keys a member sent. Returns whether anything is new.
    pub(crate) fn merge_writes(&mut self, pairs: &[EpochKey]) -> bool {
        let mut changed = false;
        for (epoch, write) in pairs {
            if !self.writes.iter().any(|(e, _)| e == epoch) {
                self.writes.push((*epoch, *write));
                changed = true;
            }
        }
        self.writes.sort_by_key(|(e, _)| *e);
        changed
    }
}

fn aad(id: &CommunityId) -> Vec<u8> {
    [AAD_DOMAIN, id.as_bytes()].concat()
}

/// Reads a community's epoch keys. A missing file is an empty store.
pub(crate) fn load(paths: &Paths, vault: &Vault, id: &CommunityId) -> Result<EpochStore> {
    match std::fs::read(paths.epoch_keys(id)) {
        Ok(sealed) => {
            let plain = vault.subkey(PURPOSE).open_sealed(&aad(id), &sealed)?;
            postcard::from_bytes(&plain).map_err(|e| CoreError::Encoding(e.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(EpochStore::default()),
        Err(e) => Err(e.into()),
    }
}

/// Writes a community's epoch keys, sealed.
pub(crate) fn save(paths: &Paths, vault: &Vault, id: &CommunityId, store: &EpochStore) -> Result<()> {
    let plain = postcard::to_allocvec(store).map_err(|e| CoreError::Encoding(e.to_string()))?;
    let sealed = vault.subkey(PURPOSE).seal(&aad(id), &plain);
    write_bytes_atomic(&paths.epoch_keys(id), &sealed)
}

/// Member-only control as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberOnlyStatus {
    /// Whether member-only control is on.
    pub on: bool,
    /// Whether it may be turned on: every other member device answered that it supports it.
    pub can_enable: bool,
    /// Member devices that did not answer as supporting it, including offline ones.
    pub waiting: usize,
}

impl World {
    /// Rebuilds the community's replay with its epoch keys. Returns the effects of the events they
    /// unlocked, for the caller to perform after releasing the lock.
    pub(crate) fn install_epochs(&mut self, community: CommunityId) -> ControlEffects {
        let World {
            paths,
            communities,
            dms,
            conv_book,
            identity_changes,
            ..
        } = self;
        let Some(c) = communities.get_mut(&community) else {
            return ControlEffects::default();
        };
        let ring = c.epochs.enc_ring();
        c.set_epoch_ring(ring);
        let unlocked = c.take_unhandled(&[]);
        control_effects(
            community,
            c,
            paths,
            dms,
            conv_book,
            identity_changes,
            crate::engine::now_ms(),
            &unlocked,
        )
    }
}

impl Engine {
    /// Keeps the control key of the community's current MLS epoch, and the devices the commit that
    /// entered the epoch added. Call after every change to the community's group. Each device
    /// keeps every epoch it has been in, so an event sealed in an old epoch still opens here.
    pub(crate) fn note_epoch(&self, community: CommunityId, joined: &[DeviceId]) {
        let group = community.0;
        let (epoch, export) = match self.with_mls_read(|mls| -> dissent_mls::Result<(u64, Vec<u8>)> {
            Ok((
                mls.epoch(&group)?,
                mls.export_secret(&group, EXPORT_LABEL, 32)?,
            ))
        }) {
            Ok(found) => found,
            Err(e) => {
                warn!(community = %community, error = %e, "could not export the control key of the epoch");
                return;
            }
        };
        let keys = EpochKeys::from_export(&export);
        let fx = {
            let mut w = self.write();
            let paths = w.paths.clone();
            let Some(c) = w.communities.get_mut(&community) else {
                return;
            };
            if !c.epochs.record(epoch, keys, joined) {
                return;
            }
            if let Err(e) = save(&paths, &self.inner.vault, &community, &c.epochs) {
                warn!(community = %community, error = %e, "could not save the epoch keys");
            }
            w.install_epochs(community)
        };
        self.perform_effects(fx);
        self.record_history_key(community);
    }

    /// The epoch key a control event is sealed under while member-only control is on, with its
    /// epoch. `None` when it is off, or when this device has no group yet (a joiner or a storage
    /// member). Then the event takes the community-secret form, which is why a joiner blanks its
    /// name until it has a group.
    pub(crate) fn member_seal(&self, community: CommunityId) -> Result<Option<(u64, EpochKeys)>> {
        let on = self
            .read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().member_only);
        if !on {
            return Ok(None);
        }
        let group = community.0;
        let Ok(epoch) = self.with_mls_read(|mls| mls.epoch(&group)) else {
            return Ok(None);
        };
        let find = || {
            self.read().communities.get(&community).and_then(|c| {
                c.epochs
                    .keys
                    .iter()
                    .find(|(e, _)| *e == epoch)
                    .map(|(_, k)| k.clone())
            })
        };
        if let Some(keys) = find() {
            return Ok(Some((epoch, keys)));
        }
        // An epoch entered before this version has no stored key. The key of the current epoch can be
        // exported now, so derive it and try again.
        self.note_epoch(community, &[]);
        match find() {
            Some(keys) => Ok(Some((epoch, keys))),
            // Refuse rather than fall back to the community-secret form, which storage members read.
            None => Err(CoreError::Invalid(
                "member-only control is on, but this device has no key for the current epoch".into(),
            )),
        }
    }

    /// Derives the control key of the current epoch for each community this device has a group in.
    /// Runs at startup, so an epoch entered before this version still opens here.
    pub(crate) fn derive_current_epoch_keys(&self, ids: &[CommunityId]) {
        for id in ids {
            if self.with_mls_read(|mls| mls.has_group(&id.0)) {
                self.note_epoch(*id, &[]);
            }
        }
    }

    /// Answers `EpochHistory`. Only active members get keys, and never a storage member. Under
    /// `SinceJoin` a requester gets the epochs from the one its device entered at, and nothing older.
    /// A responder that did not see the requester's join answers with none.
    pub(crate) fn serve_epoch_history(&self, from: DeviceId, community: CommunityId) -> Response {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Response::Error("unknown community".into());
        };
        let person = w
            .directory
            .root_of(&from)
            .is_some_and(|root| CommitRules::of(c).is_person(&root));
        if !person {
            return Response::Error("not an active member".into());
        }
        let floor = match c.state().history {
            HistorySetting::All => 0,
            HistorySetting::SinceJoin => match c.epochs.join_epoch(&from) {
                Some(epoch) => epoch,
                None => return Response::EpochHistory(Vec::new()),
            },
        };
        let keys = c
            .epochs
            .keys
            .iter()
            .filter(|(e, _)| *e >= floor)
            .map(|(e, k)| (*e, k.enc, k.write))
            .collect();
        Response::EpochHistory(keys)
    }

    /// Answers `WriteKeys`, for an active storage member only. The write keys cannot open anything.
    pub(crate) fn serve_write_keys(&self, from: DeviceId, community: CommunityId) -> Response {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Response::Error("unknown community".into());
        };
        let storage = w.directory.root_of(&from).is_some_and(|root| {
            CommitRules::of(c).is_active(&root)
                && c.state().members.get(&root).is_some_and(|m| m.storage)
        });
        if !storage {
            return Response::Error("not an active storage member".into());
        }
        Response::WriteKeys(c.epochs.writes.clone())
    }

    /// Asks the community's member devices for their epoch keys, from the first `HISTORY_FROM` that
    /// answer. A joiner calls this after it joins. Keys it already holds are kept.
    /// Returns how many members answered.
    pub(crate) async fn fetch_epoch_history(&self, community: CommunityId) -> usize {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return 0,
            }
        };
        let mut answers = 0;
        for device in devices {
            if answers >= HISTORY_FROM {
                break;
            }
            match self
                .call(device, &Request::EpochHistory { community })
                .await
            {
                Ok(Response::EpochHistory(keys)) => {
                    answers += 1;
                    self.remember_epochs(community, &keys);
                }
                _ => debug!(community = %community, peer = %device, "no epoch keys from this member"),
            }
        }
        answers
    }

    /// `fetch_epoch_history`, retried until a member answers, for as long as the community is still
    /// locked. Retries back off up to `HISTORY_RETRY_CAP`. A member coming online retries at once.
    /// Stops when history unlocks, when this device leaves the community, or when the engine shuts
    /// down, since shutdown aborts this task.
    pub(crate) async fn fetch_epoch_history_until_answered(&self, community: CommunityId) {
        let first = self.inner.timings.history_first_retry;
        // The first attempt is a plain timer. A beacon that arrives before it does not start an attempt,
        // so an attempt can only follow a failure that a member's return then answers.
        tokio::time::sleep(first).await;
        let mut wait = first;
        loop {
            let Some(still_locked) = self.read().communities.get(&community).map(|c| {
                c.state()
                    .rejected
                    .iter()
                    .any(|(_, why)| *why == dissent_events::Rejection::Locked)
            }) else {
                return;
            };
            if !still_locked {
                return;
            }
            if self.fetch_epoch_history(community).await > 0 {
                return;
            }
            debug!(community = %community, "no member answered for epoch keys yet; will retry");
            // After a failure, wait for the next attempt, or for a member to be seen, whichever is first.
            let seen = self.inner.member_seen.notified();
            tokio::pin!(seen);
            tokio::select! {
                _ = tokio::time::sleep(wait) => wait = history_retry_after(wait),
                _ = &mut seen => wait = first,
            }
        }
    }

    /// Asks member devices for the write keys, until one answers. A storage member calls this when
    /// it sees an epoch it has no write key for.
    pub(crate) async fn fetch_write_keys(&self, community: CommunityId) {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return,
            }
        };
        for device in devices {
            if let Ok(Response::WriteKeys(pairs)) = self
                .call(device, &Request::WriteKeys { community })
                .await
            {
                self.remember_write_keys(community, &pairs);
                return;
            }
        }
    }

    /// Stores epoch keys a member sent, and rebuilds the replay with them.
    fn remember_epochs(&self, community: CommunityId, pairs: &[(u64, [u8; 32], [u8; 32])]) {
        let fx = {
            let mut w = self.write();
            let paths = w.paths.clone();
            let Some(c) = w.communities.get_mut(&community) else {
                return;
            };
            if !c.epochs.merge(pairs) {
                return;
            }
            if let Err(e) = save(&paths, &self.inner.vault, &community, &c.epochs) {
                warn!(community = %community, error = %e, "could not save the epoch keys");
            }
            w.install_epochs(community)
        };
        self.perform_effects(fx);
    }

    /// Stores write keys a member sent. A storage member checks tags with them.
    fn remember_write_keys(&self, community: CommunityId, pairs: &[EpochKey]) {
        let mut w = self.write();
        let paths = w.paths.clone();
        let Some(c) = w.communities.get_mut(&community) else {
            return;
        };
        if !c.epochs.merge_writes(pairs) {
            return;
        }
        if let Err(e) = save(&paths, &self.inner.vault, &community, &c.epochs) {
            warn!(community = %community, error = %e, "could not save the write keys");
        }
    }

    /// Whether every member device other than this one answers that it supports member-sealed
    /// control. Returns the status the owner sees, counting the devices that did not answer.
    pub async fn member_only_status(&self, community: CommunityId) -> Result<MemberOnlyStatus> {
        let (on, devices) = {
            let w = self.read();
            let c = w
                .communities
                .get(&community)
                .ok_or_else(|| CoreError::NotFound(community.to_string()))?;
            (c.state().member_only, w.member_devices(c))
        };
        let mut waiting = 0;
        for device in devices {
            let supports = matches!(
                self.call(device, &Request::MemberSealedSupport { community }).await,
                Ok(Response::MemberSealedSupport(true))
            );
            if !supports {
                waiting += 1;
            }
        }
        Ok(MemberOnlyStatus {
            on,
            can_enable: waiting == 0,
            waiting,
        })
    }

    /// Turns member-only control on or off. Owner only. Turning it on needs every member device to
    /// support it, so no one is left unable to read control events.
    pub async fn set_member_only(&self, community: CommunityId, on: bool) -> Result<()> {
        let owner = self
            .read()
            .communities
            .get(&community)
            .map(|c| c.owner())
            .ok_or_else(|| CoreError::NotFound(community.to_string()))?;
        if owner != self.inner.me {
            return Err(CoreError::Rejected("NoPermission".into()));
        }
        if on {
            let status = self.member_only_status(community).await?;
            if !status.can_enable {
                return Err(CoreError::Invalid(format!(
                    "{} member device(s) need to update",
                    status.waiting
                )));
            }
        }
        self.author_publish(community, ChannelId::CONTROL, Payload::MemberOnlyControl { on })
            .await?;
        Ok(())
    }

    /// Sets which past control content a new member is given keys for. Owner and admins only. The
    /// default is `All`. A member who joined before the change keeps what it already holds.
    pub async fn set_history_visibility(
        &self,
        community: CommunityId,
        setting: HistorySetting,
    ) -> Result<()> {
        // Gated like the switch: a member device that predates this setting cannot read the events.
        let status = self.member_only_status(community).await?;
        if !status.can_enable {
            return Err(CoreError::Invalid(format!(
                "{} member device(s) need to update",
                status.waiting
            )));
        }
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::HistoryVisibility { setting },
        )
        .await?;
        Ok(())
    }

    /// The name a joiner's Join carries. It is blanked while member-only control is on, and the
    /// name follows as a member-sealed Join once the joiner has a group.
    pub(crate) fn pre_join_name(&self, community: CommunityId, name: String) -> String {
        let on = self
            .read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().member_only);
        if on {
            String::new()
        } else {
            name
        }
    }

    /// After a join, puts the name back on a Join whose name was blanked before the group.
    pub(crate) async fn restore_join_name(&self, community: CommunityId) {
        let blank = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.state().members.get(&self.inner.me).map(|m| m.display_name.is_empty()))
            .unwrap_or(false);
        if !blank {
            return;
        }
        let name = self.read().my_name();
        if let Err(e) = self
            .author_publish(community, ChannelId::CONTROL, Payload::Join { display_name: name })
            .await
        {
            warn!(community = %community, error = %e, "could not put the name back after joining");
        }
    }
}

#[cfg(test)]
mod retry_schedule_tests {
    use super::{history_retry_after, HISTORY_RETRY_CAP};
    use std::time::Duration;

    #[test]
    fn the_wait_doubles_from_one_second_to_the_cap_and_stays_there() {
        let mut wait = Duration::from_secs(1);
        let mut seen = Vec::new();
        for _ in 0..8 {
            seen.push(wait.as_secs());
            wait = history_retry_after(wait);
        }
        assert_eq!(seen, vec![1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(HISTORY_RETRY_CAP, Duration::from_secs(60));
    }
}
