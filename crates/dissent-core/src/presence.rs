//! Presence: the status this device shows, how long a timed status lasts, and the signed beacons
//! that tell the people who can see us what it is.
//!
//! A beacon goes out every `beacon_every` to each community topic we are on, and to the devices of
//! each contact. Invisible sends none. Others read a person as online only while a fresh beacon
//! from them is held. Traffic alone never makes someone look online.

use crate::engine::{now_ms, Engine, Inner, MAX_CLOCK_SKEW_MS};
use crate::error::{CoreError, Result};
use crate::rpc::Request;
use crate::store::{read_bin, write_bin};
use crate::timings::millis;
use crate::views::EngineEvent;
use crate::wire::{encode_gossip, GossipMsg};
use crate::world::World;
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use dissent_net::TopicSender;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tracing::{debug, warn};

/// Domain prefix for the signature over a presence beacon.
const PRESENCE_DOMAIN: &[u8] = b"dissent/presence/v1";
/// Longest a shutdown waits to send the offline beacon.
pub(crate) const OFFLINE_WAIT: Duration = Duration::from_secs(2);

/// The status a person sets for themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Online,
    /// Away. Set by hand, or by the desktop after a quiet ten minutes while online.
    Idle,
    /// Do not disturb. Desktop notifications and sounds are off. Incoming calls still ring.
    Dnd,
    /// Shown as offline to everyone. Sends no beacons.
    Invisible,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Idle => "idle",
            Status::Dnd => "dnd",
            Status::Invisible => "invisible",
        }
    }
}

impl FromStr for Status {
    type Err = CoreError;

    fn from_str(s: &str) -> std::result::Result<Self, CoreError> {
        match s {
            "online" => Ok(Status::Online),
            "idle" => Ok(Status::Idle),
            "dnd" => Ok(Status::Dnd),
            "invisible" => Ok(Status::Invisible),
            other => Err(CoreError::Invalid(format!("unknown status {other}"))),
        }
    }
}

/// What others see of a person: one of the statuses they can beacon, or offline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Idle,
    Dnd,
    Offline,
}

impl Presence {
    /// Invisible reads as offline.
    fn of(status: Status) -> Self {
        match status {
            Status::Online => Presence::Online,
            Status::Idle => Presence::Idle,
            Status::Dnd => Presence::Dnd,
            Status::Invisible => Presence::Offline,
        }
    }
}

/// This device's own status, as stored in `status.bin`. `until_ms` is set only for a timed status.
/// Kept apart from `local.bin`, because that file's layout is read without a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OwnStatus {
    pub status: Status,
    pub until_ms: Option<u64>,
}

impl Default for OwnStatus {
    fn default() -> Self {
        Self {
            status: Status::Online,
            until_ms: None,
        }
    }
}

impl OwnStatus {
    /// The status in force at `now`. A timed status that has run out reads as online.
    fn at(self, now: u64) -> Self {
        match self.until_ms {
            Some(until) if until <= now => Self::default(),
            _ => self,
        }
    }
}

/// Loads the stored status. A missing or unreadable file means online.
pub(crate) fn load_own(paths: &crate::store::Paths) -> OwnStatus {
    match read_bin(&paths.own_status()) {
        Ok(own) => own.unwrap_or_default(),
        Err(e) => {
            warn!(error = %e, "could not read the stored status; starting online");
            OwnStatus::default()
        }
    }
}

/// A signed status, sent by one device. Anyone may relay it, so receivers check the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedPresence {
    pub cert: DeviceCert,
    pub status: Status,
    pub ts_ms: u64,
    pub sig: Signature,
}

impl SignedPresence {
    /// Signs `status` with the device key. `cert` must certify that key.
    pub fn sign(cert: DeviceCert, status: Status, ts_ms: u64, key: &DeviceKey) -> Result<Self> {
        let sig = key.sign(&signing_bytes(cert.device, status, ts_ms)?);
        Ok(Self {
            cert,
            status,
            ts_ms,
            sig,
        })
    }

    /// Checks the cert and the device signature. `Invisible` is valid only as the offline beacon
    /// sent when going invisible or shutting down.
    pub fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        let msg = signing_bytes(self.cert.device, self.status, self.ts_ms)?;
        self.cert.device.verify(&msg, &self.sig)?;
        Ok(())
    }
}

fn signing_bytes(device: DeviceId, status: Status, ts_ms: u64) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(device, status, ts_ms))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(PRESENCE_DOMAIN.len() + body.len());
    out.extend_from_slice(PRESENCE_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

/// The newest beacon held for one person.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Heard {
    pub status: Status,
    pub ts_ms: u64,
    /// Our clock when it arrived. Freshness is judged from this, so a skewed sender clock does not
    /// make a live person look offline.
    pub received_ms: u64,
}

impl World {
    /// The status this device shows at `now`, with auto-idle applied.
    pub(crate) fn own_status(&self, now: u64) -> (Status, Option<u64>) {
        self.shown(self.own.at(now))
    }

    /// `own` with auto-idle applied. Auto-idle turns an online status into idle, and nothing else.
    pub(crate) fn shown(&self, own: OwnStatus) -> (Status, Option<u64>) {
        let status = match own.status {
            Status::Online if self.auto_idle => Status::Idle,
            status => status,
        };
        (status, own.until_ms)
    }

    /// What others see of `user` at `now`.
    pub(crate) fn presence_of(&self, user: &RootId, now: u64) -> Presence {
        if *user == self.me {
            return Presence::of(self.own_status(now).0);
        }
        match self.presence.get(user) {
            Some(heard) if now.saturating_sub(heard.received_ms) < millis(self.timings.presence_window) => {
                Presence::of(heard.status)
            }
            _ => Presence::Offline,
        }
    }
}

impl Engine {
    /// Sets our status. `until_ms` ends it, and then the status reads as online. `None` lasts until
    /// it is changed. A status set to online never has an end time.
    pub fn set_status(&self, status: Status, until_ms: Option<u64>) -> Result<()> {
        if until_ms.is_some_and(|until| until <= now_ms()) {
            return Err(CoreError::Invalid("a status cannot end in the past".into()));
        }
        let own = OwnStatus {
            status,
            until_ms: if status == Status::Online { None } else { until_ms },
        };
        self.change_status(|w| {
            write_bin(&w.paths.own_status(), &own)?;
            w.own = own;
            Ok(())
        })
    }

    /// Our status now, as the desktop shows it. Auto-idle reads as idle, and a timed status
    /// reads as online once its end time has passed.
    pub fn status(&self) -> (Status, Option<u64>) {
        self.read().own_status(now_ms())
    }

    /// Turns auto-idle on or off. The desktop turns it on after a quiet ten minutes and off on the
    /// next input. It shows only while the status is online, and it is never stored, so a restart
    /// cannot leave it set.
    pub fn set_auto_idle(&self, idle: bool) {
        // Only the in-memory flag changes, so the change cannot fail.
        let _ = self.change_status(|w| {
            w.auto_idle = idle;
            Ok(())
        });
    }

    /// Applies a change to our status. If what others see changes, it emits `StatusChanged` and
    /// sends a beacon.
    fn change_status(&self, change: impl FnOnce(&mut World) -> Result<()>) -> Result<()> {
        let (before, after) = {
            let mut guard = self.write();
            let before = guard.shown(guard.own);
            change(&mut guard)?;
            (before, guard.shown(guard.own))
        };
        if before != after {
            self.emit(EngineEvent::StatusChanged {
                status: after.0,
                until_ms: after.1,
            });
            let engine = self.clone();
            if after.0 != Status::Invisible {
                self.spawn_task(async move { engine.send_presence().await });
            } else if before.0 != Status::Invisible {
                // Going invisible: one offline beacon, so peers drop us now, not when the window ends.
                self.spawn_task(async move { engine.send_offline().await });
            }
        }
        Ok(())
    }

    /// Reverts a timed status whose end time has passed. Checked again under the lock, so a status
    /// set in the meantime is kept.
    fn expire_status(&self) {
        let result = self.change_status(|w| {
            if w.own.until_ms.is_some_and(|until| until <= now_ms()) {
                let own = OwnStatus::default();
                write_bin(&w.paths.own_status(), &own)?;
                w.own = own;
            }
            Ok(())
        });
        if let Err(e) = result {
            warn!(error = %e, "could not revert an expired status");
        }
    }

    /// A beacon for `status`, signed now by this device.
    fn sign_beacon(&self, status: Status) -> Option<SignedPresence> {
        match SignedPresence::sign(
            self.inner.identity.cert.clone(),
            status,
            now_ms(),
            &self.inner.identity.device,
        ) {
            Ok(sp) => Some(sp),
            Err(e) => {
                warn!(error = %e, "could not sign our presence");
                None
            }
        }
    }

    /// Our beacon for the current status, or `None` when invisible.
    fn beacon(&self) -> Option<SignedPresence> {
        match self.status().0 {
            Status::Invisible => None,
            status => self.sign_beacon(status),
        }
    }

    fn topic_senders(&self) -> Vec<Arc<TopicSender>> {
        self.read()
            .communities
            .values()
            .filter_map(|c| c.sender.clone())
            .collect()
    }

    /// Sends our beacon to every community topic we are on and to each contact's devices. Best
    /// effort.
    pub(crate) async fn send_presence(&self) {
        let Some(sp) = self.beacon() else {
            return;
        };
        gossip_beacon(&self.topic_senders(), &sp).await;
        self.send_direct_beacon(&sp);
    }

    /// Sends the offline beacon: an `Invisible` status, which peers read as offline at once. Sent
    /// on going invisible and on shutdown. Direct sends are awaited, so the caller bounds the time.
    pub(crate) async fn send_offline(&self) {
        let Some(sp) = self.sign_beacon(Status::Invisible) else {
            return;
        };
        gossip_beacon(&self.topic_senders(), &sp).await;
        let bytes = match self.request_bytes(&Request::Presence(Box::new(sp))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our offline beacon");
                return;
            }
        };
        // Gossip is fire-and-forget and the node closes right after, so also deliver it directly,
        // acknowledged, to contacts and to the community members we have heard from recently.
        let devices = offline_devices(&self.read(), now_ms());
        let node = self.node();
        let mut sends = tokio::task::JoinSet::new();
        for device in devices {
            let node = node.clone();
            let bytes = bytes.clone();
            sends.spawn(async move {
                if let Err(e) = node.request(device, bytes).await {
                    debug!(device = %device, error = %e, "offline beacon not delivered directly");
                }
            });
        }
        while sends.join_next().await.is_some() {}
    }

    /// Sends our beacon on one community's topic, when a neighbour joins it.
    pub(crate) async fn broadcast_presence(&self, community: CommunityId) {
        let sender = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.sender.clone());
        if let (Some(sender), Some(sp)) = (sender, self.beacon()) {
            gossip_beacon(&[sender], &sp).await;
        }
    }

    /// Emits `PresenceChanged` for each peer whose last beacon ran out of its window between
    /// `since` and now, so the UI updates without polling.
    fn sweep_presence(&self, since: u64) {
        let aged = {
            let w = self.read();
            aged_out(&w.presence, millis(w.timings.presence_window), since, now_ms())
        };
        for user in aged {
            self.emit(EngineEvent::PresenceChanged {
                user: user.to_string(),
            });
        }
    }

    /// Sends our beacon to each contact device, over the direct connection.
    fn send_direct_beacon(&self, sp: &SignedPresence) {
        let bytes = match self.request_bytes(&Request::Presence(Box::new(sp.clone()))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our presence");
                return;
            }
        };
        let devices = beacon_devices(&self.read());
        let node = self.node();
        for device in devices {
            let node = node.clone();
            let bytes = bytes.clone();
            self.spawn_task(async move {
                if let Err(e) = node.request(device, bytes).await {
                    debug!(device = %device, error = %e, "presence not delivered directly");
                }
            });
        }
    }

    /// Checks a beacon and records it. `community` is the topic it arrived on, or `None` for a
    /// direct beacon. The sender must be a member of that community, or else a contact who is not
    /// blocked.
    pub(crate) fn accept_presence(&self, community: Option<CommunityId>, sp: SignedPresence) -> Result<()> {
        sp.verify()?;
        let now = now_ms();
        if sp.ts_ms.abs_diff(now) > MAX_CLOCK_SKEW_MS {
            return Err(CoreError::Invalid("presence timestamp is too far from our clock".into()));
        }
        let root = sp.cert.root;
        let (before, after) = {
            let mut guard = self.write();
            let w = &mut *guard;
            if root == w.me {
                return Ok(());
            }
            let allowed = match community {
                Some(id) => w.communities.get(&id).is_some_and(|c| c.is_member(&root)),
                // Sent directly: from a contact, or from someone we share a community with (the
                // same people who see this beacon on a community topic).
                None => {
                    !w.contact_book.blocked.contains(&root)
                        && (w.is_contact(&root) || w.communities.values().any(|c| c.is_member(&root)))
                }
            };
            if !allowed {
                return Err(CoreError::Invalid("sender is neither a member nor a contact".into()));
            }
            if w.presence.get(&root).is_some_and(|held| held.ts_ms >= sp.ts_ms) {
                return Ok(());
            }
            let before = w.presence_of(&root, now);
            w.presence.insert(
                root,
                Heard {
                    status: sp.status,
                    ts_ms: sp.ts_ms,
                    received_ms: now,
                },
            );
            (before, w.presence_of(&root, now))
        };
        if community.is_some() {
            self.inner.member_seen.notify_waiters();
        }
        if before != after {
            self.emit(EngineEvent::PresenceChanged {
                user: root.to_string(),
            });
        }
        Ok(())
    }
}

/// Sends one beacon on each topic. Delivery is best effort.
async fn gossip_beacon(senders: &[Arc<TopicSender>], sp: &SignedPresence) {
    let bytes = match encode_gossip(&GossipMsg::Presence(Box::new(sp.clone()))) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(error = %e, "could not encode our presence");
            return;
        }
    };
    for sender in senders {
        if let Err(e) = sender.broadcast(bytes.clone()).await {
            debug!(error = %e, "presence not broadcast on a community topic");
        }
    }
}

/// Devices that get our beacon directly: those of our contacts, minus blocked people.
pub(crate) fn beacon_devices(w: &World) -> Vec<DeviceId> {
    let mut devices: BTreeSet<DeviceId> = BTreeSet::new();
    for card in &w.contacts {
        let root = card.cert.root;
        if root == w.me || w.contact_book.blocked.contains(&root) {
            continue;
        }
        devices.extend(w.directory.devices_of(&root));
    }
    devices.remove(&w.device);
    devices.into_iter().collect()
}

/// Where the offline beacon goes directly: every contact device, plus the devices of community
/// members whose beacon we heard within the presence window (so they currently show us online).
fn offline_devices(w: &World, now: u64) -> Vec<DeviceId> {
    let mut devices: BTreeSet<DeviceId> = beacon_devices(w).into_iter().collect();
    for (root, heard) in &w.presence {
        let fresh = now.saturating_sub(heard.received_ms) < millis(w.timings.presence_window);
        let shared = w.communities.values().any(|c| c.is_member(root));
        if fresh && shared && *root != w.me && !w.contact_book.blocked.contains(root) {
            devices.extend(w.directory.devices_of(root));
        }
    }
    devices.remove(&w.device);
    devices.into_iter().collect()
}

/// The people whose last beacon ran out of its window after `since` and no later than `now`. An
/// invisible beacon already reads as offline, so it is left out.
pub(crate) fn aged_out(
    presence: &BTreeMap<RootId, Heard>,
    window_ms: u64,
    since: u64,
    now: u64,
) -> Vec<RootId> {
    presence
        .iter()
        .filter(|(_, heard)| heard.status != Status::Invisible)
        .filter(|(_, heard)| {
            let end = heard.received_ms.saturating_add(window_ms);
            since < end && end <= now
        })
        .map(|(root, _)| *root)
        .collect()
}

/// Runs for the life of the engine. Once a second it reverts an expired status. Every
/// `beacon_every` it sends a beacon, starting at once. Every `presence_sweep` it reports peers gone
/// quiet. Both intervals are whole seconds.
pub(crate) async fn presence_loop(weak: Weak<Inner>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut seconds: u64 = 0;
    let mut swept_at = now_ms();
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else {
            break;
        };
        let engine = Engine { inner };
        let timings = engine.inner.timings;
        engine.expire_status();
        if seconds.is_multiple_of(timings.beacon_every.as_secs()) {
            engine.send_presence().await;
        }
        if seconds.is_multiple_of(timings.presence_sweep.as_secs()) {
            engine.sweep_presence(swept_at);
            swept_at = now_ms();
        }
        seconds += 1;
    }
}

#[cfg(test)]
mod tests;
