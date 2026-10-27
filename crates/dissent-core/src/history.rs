//! Message history for joiners. A plain message's text is sealed under the history key of its epoch, so a
//! member who joins later can read it when the server's history setting allows. The design is in
//! `docs/design/05-encrypted-servers.md`, "History messages".
//!
//! A device records each epoch's history key when it records the epoch's control keys. It serves those keys to
//! joiners by the rule for `EpochHistory`. It sends the new form only when every other member device says it
//! supports it. Until then, and for older messages, the MLS form is used, as before.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use dissent_events::{ChannelId, HistorySetting};
use dissent_identity::{CommunityId, DeviceId, RootId, Vault};
use tracing::{debug, warn};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::store::{Paths};
use crate::views::EngineEvent;

/// The exporter label of an epoch's history key.
const LABEL: &str = "dissent/history/v1";
/// Domain of the message ciphertext's associated data.
const MSG_DOMAIN: &[u8] = b"dissent/history-msg/v1";
/// Domain of the file that stores the keys.
const STORE_DOMAIN: &[u8] = b"dissent/history-keys/v1";
const STORE_PURPOSE: &[u8] = b"history-keys";
const NONCE_LEN: usize = 24;
const READY_FOR: Duration = Duration::from_secs(300);
/// How long a "no" is kept before it is checked again, so a device that comes back is counted soon.
const NOT_READY_FOR: Duration = Duration::from_secs(60);
const ASK_WAIT: Duration = Duration::from_secs(5);
/// How long a member's presence beacon waits before the readiness check it triggers, so a burst starts one check.
const PRESENCE_DEBOUNCE: Duration = Duration::from_millis(500);
/// Member devices asked for history keys, and how many must answer before a joiner stops asking.
const HISTORY_FROM: usize = 3;
/// A joiner's first retry, then doubling up to `RETRY_CAP`. A member coming online or a new key cuts a wait short.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_CAP: Duration = Duration::from_secs(60);
const RETRY_TRIES: usize = 30;
/// How often a push is tried, and how many times, before the joiner's own fetch takes over.
const PUSH_EVERY: Duration = Duration::from_secs(1);
const PUSH_TRIES: usize = 4;

/// The keys of one community: the history key of each epoch, by epoch number.
pub(crate) type KeyMap = BTreeMap<u64, [u8; 32]>;

/// A readiness answer: when it was checked, the member devices it covers, and whether all supported it.
pub(crate) type ReadyAnswer = (Instant, Vec<DeviceId>, bool);

fn store_aad(id: &CommunityId) -> Vec<u8> {
    [STORE_DOMAIN, id.as_bytes()].concat()
}

/// The text's associated data: the community, the epoch and the domain. A ciphertext cannot be moved to another
/// community or epoch.
fn msg_aad(id: &CommunityId, epoch: u64) -> Vec<u8> {
    [id.as_bytes().as_slice(), &epoch.to_be_bytes(), MSG_DOMAIN].concat()
}

/// Reads a community's history keys, by epoch. A missing file is none.
fn load_store(paths: &Paths, vault: &Vault, id: &CommunityId) -> Result<BTreeMap<u64, [u8; 32]>> {
    match std::fs::read(paths.history_keys(id)) {
        Ok(sealed) => {
            let plain = vault.subkey(STORE_PURPOSE).open_sealed(&store_aad(id), &sealed)?;
            postcard::from_bytes(&plain).map_err(|e| CoreError::Encoding(e.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_store(paths: &Paths, vault: &Vault, id: &CommunityId, keys: &BTreeMap<u64, [u8; 32]>) -> Result<()> {
    let plain = postcard::to_allocvec(keys).map_err(|e| CoreError::Encoding(e.to_string()))?;
    let sealed = vault.subkey(STORE_PURPOSE).seal(&store_aad(id), &plain);
    std::fs::write(paths.history_keys(id), sealed)?;
    Ok(())
}

/// The readiness answers kept on disk, by community: the member devices each answer covers, and the answer.
type ReadyFile = BTreeMap<CommunityId, (Vec<DeviceId>, bool)>;

/// Loads the last readiness answers, so a restart starts from them rather than from "unknown". A missing or unreadable
/// file starts empty, which is the older form until a check runs.
pub(crate) fn load_ready(paths: &Paths) -> BTreeMap<CommunityId, ReadyAnswer> {
    let saved: ReadyFile = crate::store::read_bin(&paths.history_ready()).ok().flatten().unwrap_or_default();
    saved
        .into_iter()
        .map(|(community, (devices, ready))| (community, (Instant::now(), devices, ready)))
        .collect()
}

/// Seals `plain` under `key`, with a random nonce in front.
fn seal_text(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; NONCE_LEN] = rand::random();
    let cipher = XChaCha20Poly1305::new(key.into());
    let body = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| CoreError::Encoding("could not seal the message".into()))?;
    Ok([nonce.as_slice(), body.as_slice()].concat())
}

/// Opens what `seal_text` wrote. `None` when the key is wrong or the data was changed.
fn open_text(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, body) = data.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad }).ok()
}

impl Engine {
    /// Records this device's history key for the current epoch. Called whenever the epoch's control keys are
    /// recorded. A device with no group yet records nothing. The MLS lock is taken on its own, never under the
    /// World lock, which the send and unseal paths take in the other order.
    pub(crate) fn record_history_key(&self, community: CommunityId) {
        let group = community.0;
        let Ok(epoch) = self.with_mls_read(|mls| mls.epoch(&group)) else {
            return;
        };
        let paths = self.read().paths.clone();
        // Export only for an epoch not yet recorded, so a repeated call does no key work.
        match self.with_keys(&paths, community, |keys| keys.contains_key(&epoch)) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => {
                warn!(community = %community, error = %e, "could not read the history keys");
                return;
            }
        }
        let Ok(export) = self.with_mls_read(|mls| mls.export_secret(&group, LABEL, 32)) else {
            return;
        };
        let Ok(key) = <[u8; 32]>::try_from(export.as_slice()) else {
            return;
        };
        self.store_keys(&paths, community, |keys| {
            keys.entry(epoch).or_insert(key);
        });
        // A new epoch's key is news to a joiner that is waiting for keys.
        self.inner.member_seen.notify_waiters();
    }

    /// Sends the keys a device may have, to a device this member has just admitted. Under `All` every epoch; under
    /// `SinceJoin` only the epochs from the device's join. A push is tried a few times, then the device's own fetch
    /// takes over.
    pub(crate) fn push_history_keys(&self, community: CommunityId, device: DeviceId) {
        let engine = self.clone();
        self.spawn_task(async move {
            engine.send_history_keys(community, device).await;
        });
    }

    async fn send_history_keys(&self, community: CommunityId, device: DeviceId) {
        for _ in 0..PUSH_TRIES {
            let floor = {
                let w = self.read();
                let Some(c) = w.communities.get(&community) else {
                    return;
                };
                match c.state().history {
                    HistorySetting::All => 0,
                    HistorySetting::SinceJoin => match c.epochs.join_epoch(&device) {
                        Some(epoch) => epoch,
                        None => return,
                    },
                }
            };
            let paths = self.read().paths.clone();
            let pairs = match self.with_keys(&paths, community, |keys| {
                keys.iter().filter(|(e, _)| **e >= floor).map(|(e, k)| (*e, *k)).collect::<Vec<_>>()
            }) {
                Ok(pairs) if !pairs.is_empty() => pairs,
                _ => return,
            };
            let req = Request::PushHistoryKeys { community, pairs };
            match tokio::time::timeout(ASK_WAIT, self.call(device, &req)).await {
                Ok(Ok(Response::Ack)) => return,
                _ => tokio::time::sleep(PUSH_EVERY).await,
            }
        }
    }

    /// Answers `PushHistoryKeys`. Only an active member may send keys, and what it sends is kept like a fetched key.
    pub(crate) fn accept_history_keys(
        &self,
        from: DeviceId,
        community: CommunityId,
        pairs: Vec<(u64, [u8; 32])>,
    ) -> Response {
        {
            let w = self.read();
            let Some(root) = w.directory.root_of(&from) else {
                return Response::Error("unknown device".into());
            };
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            if !crate::membership::CommitRules::of(c).is_person(&root) {
                return Response::Error("not an active member".into());
            }
        }
        if self.remember_history_keys(community, &pairs) {
            self.unseal_pending(community);
            self.announce_channels(community);
        }
        Response::Ack
    }

    /// The history key of `epoch`, if this device holds it. Read from memory, never from the file.
    fn history_key(&self, community: CommunityId, epoch: u64) -> Option<[u8; 32]> {
        let paths = self.read().paths.clone();
        self.with_keys(&paths, community, |keys| keys.get(&epoch).copied()).ok().flatten()
    }

    /// Opens a history message's ciphertext (without its manifest), as the plaintext encoding of the text.
    pub(crate) fn open_history_text(&self, community: CommunityId, epoch: u64, data: &[u8]) -> Option<Vec<u8>> {
        let key = self.history_key(community, epoch)?;
        open_text(&key, &msg_aad(&community, epoch), data)
    }

    /// The history form of a message sent now: `(epoch, ciphertext)`. `None` when today's form must be used: when
    /// not every other member device supports history messages, or this device has no key for the current epoch.
    /// It never waits on a member. The answer is read from a flag that the background check keeps.
    pub(crate) fn history_seal(&self, community: CommunityId, text: &str) -> Option<(u64, Vec<u8>)> {
        let epoch = self.mls_epoch(community)?;
        let key = self.history_key(community, epoch)?;
        if !self.history_ready_now(community) {
            return None;
        }
        let plain = postcard::to_allocvec(&text.to_string()).ok()?;
        seal_text(&key, &msg_aad(&community, epoch), &plain).ok().map(|ct| (epoch, ct))
    }

    /// Whether the new form may be used now, from the flag alone. With no other member device there is nothing
    /// to ask, so the answer is yes. Otherwise a fresh answer for the current member devices is used. An answer
    /// that is missing, stale or for other devices counts as no, and starts a check in the background.
    pub(crate) fn history_ready_now(&self, community: CommunityId) -> bool {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return false,
            }
        };
        if devices.is_empty() {
            return true;
        }
        let known = self
            .inner
            .history_ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&community)
            .cloned();
        match known {
            Some((at, seen, ready)) if seen == devices => {
                let ttl = if ready { READY_FOR } else { NOT_READY_FOR };
                if at.elapsed() >= ttl {
                    self.start_history_check(community);
                }
                ready && at.elapsed() < ttl
            }
            _ => {
                self.start_history_check(community);
                false
            }
        }
    }

    /// Starts a background check of `community`, unless one is already running.
    fn start_history_check(&self, community: CommunityId) {
        self.start_history_check_after(community, Duration::ZERO);
    }

    /// Starts a background check of `community` after `delay`. A check already waiting or running covers the
    /// request, so a burst of requests starts one check.
    fn start_history_check_after(&self, community: CommunityId, delay: Duration) {
        let fresh = self
            .inner
            .history_checking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(community);
        if fresh {
            let engine = self.clone();
            self.spawn_task(async move {
                tokio::time::sleep(delay).await;
                engine.refresh_history_ready(community).await;
            });
        }
    }

    /// A member's presence beacon arrived. In each community that member belongs to, a readiness answer of no is
    /// checked again soon, so a device that came back is counted without waiting for the next minute. The short
    /// delay lets a burst of beacons start one check.
    pub(crate) fn history_member_seen(&self, root: RootId) {
        let communities: Vec<CommunityId> = {
            let w = self.read();
            w.communities.iter().filter(|(_, c)| c.is_member(&root)).map(|(id, _)| *id).collect()
        };
        for community in communities {
            let not_ready = self
                .inner
                .history_ready
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&community)
                .is_some_and(|(_, _, ready)| !ready);
            if not_ready {
                self.start_history_check_after(community, PRESENCE_DEBOUNCE);
            }
        }
    }

    /// Asks every other member device whether it supports history messages, and keeps the answer. A device that
    /// does not answer, or answers with an error, counts as not supporting it.
    pub(crate) async fn refresh_history_ready(&self, community: CommunityId) -> bool {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => {
                    self.inner.history_checking.lock().unwrap_or_else(PoisonError::into_inner).remove(&community);
                    return false;
                }
            }
        };
        let mut ready = true;
        for device in devices.clone() {
            let reply = tokio::time::timeout(ASK_WAIT, self.call(device, &Request::HistorySupport)).await;
            if !matches!(reply, Ok(Ok(Response::HistorySupport(true)))) {
                ready = false;
                break;
            }
        }
        self.inner
            .history_ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(community, (Instant::now(), devices, ready));
        self.save_ready();
        self.inner.history_checking.lock().unwrap_or_else(PoisonError::into_inner).remove(&community);
        ready
    }

    /// Writes the readiness answers to disk, so a restart starts from them.
    fn save_ready(&self) {
        let snapshot: ReadyFile = self
            .inner
            .history_ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(community, (_, devices, ready))| (*community, (devices.clone(), *ready)))
            .collect();
        let paths = self.read().paths.clone();
        if let Err(e) = crate::store::write_bin(&paths.history_ready(), &snapshot) {
            warn!(error = %e, "could not save the history readiness");
        }
    }

    /// Answers `HistoryKeys`. Only an active person gets keys, never a storage member. Under `All` the requester
    /// gets every epoch this device holds. Under `SinceJoin` it gets the epochs from the requester's join, and
    /// nothing older, the same as `EpochHistory`.
    pub(crate) fn serve_history_keys(&self, from: DeviceId, community: CommunityId) -> Response {
        let (floor, paths) = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            let person = w
                .directory
                .root_of(&from)
                .is_some_and(|root| crate::membership::CommitRules::of(c).is_person(&root));
            if !person {
                return Response::Error("not an active member".into());
            }
            let floor = match c.state().history {
                HistorySetting::All => 0,
                HistorySetting::SinceJoin => match c.epochs.join_epoch(&from) {
                    Some(epoch) => epoch,
                    None => return Response::HistoryKeys(Vec::new()),
                },
            };
            (floor, w.paths.clone())
        };
        match self.with_keys(&paths, community, |keys| {
            keys.iter().filter(|(e, _)| **e >= floor).map(|(e, k)| (*e, *k)).collect::<Vec<_>>()
        }) {
            Ok(pairs) => Response::HistoryKeys(pairs),
            Err(e) => Response::Error(format!("could not read the history keys: {e}")),
        }
    }

    /// Asks the first member devices for their history keys and keeps what they send. Then the stored messages
    /// that were locked are opened again, and the channels are told so their views refresh. Returns how many
    /// devices answered.
    pub(crate) async fn fetch_history_keys(&self, community: CommunityId) -> usize {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return 0,
            }
        };
        let mut answers = 0;
        let mut learned = false;
        for device in devices {
            if answers >= HISTORY_FROM {
                break;
            }
            match self.call(device, &Request::HistoryKeys { community }).await {
                Ok(Response::HistoryKeys(pairs)) => {
                    // An answer with no keys does not end the fetch: the member may simply not hold them yet.
                    if !pairs.is_empty() {
                        answers += 1;
                    }
                    learned |= self.remember_history_keys(community, &pairs);
                }
                _ => debug!(community = %community, peer = %device, "no history keys from this member"),
            }
        }
        if learned {
            self.unseal_pending(community);
            self.announce_channels(community);
        }
        answers
    }

    /// Stores the keys a member sent, keeping the ones already held. Returns whether any was new.
    fn remember_history_keys(&self, community: CommunityId, pairs: &[(u64, [u8; 32])]) -> bool {
        let paths = self.read().paths.clone();
        let mut learned = false;
        self.store_keys(&paths, community, |keys| {
            for (epoch, key) in pairs {
                if !keys.contains_key(epoch) {
                    keys.insert(*epoch, *key);
                    learned = true;
                }
            }
        });
        learned
    }

    /// Loads the community's keys into memory, from the file, the first time they are asked for. Nothing else reads
    /// the file, so a send never does.
    fn with_keys<T>(&self, paths: &Paths, community: CommunityId, f: impl FnOnce(&KeyMap) -> T) -> Result<T> {
        let mut cache = self.inner.history_keys.lock().unwrap_or_else(PoisonError::into_inner);
        let keys = match cache.entry(community) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(slot) => {
                self.inner.history_file_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                slot.insert(load_store(paths, &self.inner.vault, &community)?)
            }
        };
        Ok(f(keys))
    }

    /// Changes the community's keys in memory, then writes the file if anything changed. The file write happens under
    /// the cache lock, so two writers cannot overwrite each other's keys.
    fn store_keys(&self, paths: &Paths, community: CommunityId, change: impl FnOnce(&mut KeyMap)) {
        let mut cache = self.inner.history_keys.lock().unwrap_or_else(PoisonError::into_inner);
        let keys = match cache.entry(community) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(slot) => {
                self.inner.history_file_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match load_store(paths, &self.inner.vault, &community) {
                    Ok(keys) => slot.insert(keys),
                    Err(e) => {
                        warn!(community = %community, error = %e, "could not read the history keys");
                        return;
                    }
                }
            }
        };
        let before = keys.len();
        change(keys);
        if keys.len() == before {
            return;
        }
        if let Err(e) = save_store(paths, &self.inner.vault, &community, keys) {
            warn!(community = %community, error = %e, "could not save the history keys");
        }
    }

    /// Tells every channel of the community that its messages may have changed, so the view reads them again.
    fn announce_channels(&self, community: CommunityId) {
        let channels: Vec<ChannelId> = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => c.state().channels.keys().copied().collect(),
                None => return,
            }
        };
        for channel in channels {
            self.emit(EngineEvent::Message {
                community_id: community.to_string(),
                channel_id: channel.to_string(),
            });
        }
    }

    /// `fetch_history_keys`, retried until a member answers, for up to `RETRY_TRIES` attempts. A joiner runs
    /// this after it joins.
    pub(crate) async fn fetch_history_keys_until_answered(&self, community: CommunityId) {
        let mut wait = RETRY_FIRST;
        for _ in 0..RETRY_TRIES {
            if !self.read().communities.contains_key(&community) {
                return;
            }
            if self.fetch_history_keys(community).await > 0 {
                return;
            }
            // Wait for the backoff, or sooner when a member comes online or a new key is recorded.
            let woken = self.inner.member_seen.notified();
            tokio::pin!(woken);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = &mut woken => {}
            }
            wait = (wait * 2).min(RETRY_CAP);
        }
    }
}

#[cfg(test)]
mod tests;
