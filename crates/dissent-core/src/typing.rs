//! Typing indicators: who is typing in a channel, a DM or a group right now.
//!
//! A typing signal is ephemeral. It is signed by the typer's device, and it is never stored or put
//! in the event DAG. A channel signal goes on the community topic, so only members get it. A DM or
//! group signal goes direct to each device of the people in the conversation, as presence does. A
//! typer drops out `TYPING_EXPIRY_MS` after their last signal, and a message from them clears them.

use crate::engine::{not_found, now_ms, Engine, Inner, MAX_CLOCK_SKEW_MS};
use crate::error::{CoreError, Result};
use crate::groups::{dm_conv_id, ConvId};
use crate::presence::Status;
use crate::rpc::Request;
use crate::views::EngineEvent;
use crate::wire::{encode_gossip, GossipMsg};
use crate::world::World;
use dissent_events::ChannelId;
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::sync::{PoisonError, Weak};
use std::time::{Duration, Instant};
use tracing::debug;

/// Domain prefix for the signature over a typing signal.
const TYPING_DOMAIN: &[u8] = b"dissent/typing/v1";
/// The least time between two typing signals for one target.
pub(crate) const TYPING_EVERY: Duration = Duration::from_secs(3);
/// A typer drops out this long after their last signal. One lost signal is tolerated.
pub(crate) const TYPING_EXPIRY_MS: u64 = 6 * 1000;
/// How often the expiry sweep runs.
const SWEEP_EVERY: Duration = Duration::from_secs(1);

/// Where someone is typing. Channels are named by community and channel, a DM by the other party,
/// and a group by its conversation id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum TypingTarget {
    Channel {
        community: CommunityId,
        channel: ChannelId,
    },
    Dm(RootId),
    Group(ConvId),
}

/// What a typing signal is about, as it travels. A DM and a group are both conversations, and the
/// receiver tells them apart by the conversation id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Scope {
    Channel {
        community: CommunityId,
        channel: ChannelId,
    },
    Conv(ConvId),
}

/// A typing signal from one device. Anyone may relay it, so receivers check the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedTyping {
    pub cert: DeviceCert,
    pub scope: Scope,
    /// True when the typer stopped: on send, or when the composer is cleared.
    pub stop: bool,
    pub ts_ms: u64,
    pub sig: Signature,
}

impl SignedTyping {
    /// Signs a signal with the device key. `cert` must certify that key.
    pub fn sign(cert: DeviceCert, scope: Scope, stop: bool, ts_ms: u64, key: &DeviceKey) -> Result<Self> {
        let sig = key.sign(&signing_bytes(cert.device, &scope, stop, ts_ms)?);
        Ok(Self {
            cert,
            scope,
            stop,
            ts_ms,
            sig,
        })
    }

    /// Checks the cert and the device signature over every field, scope included.
    pub fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        let msg = signing_bytes(self.cert.device, &self.scope, self.stop, self.ts_ms)?;
        self.cert.device.verify(&msg, &self.sig)?;
        Ok(())
    }
}

fn signing_bytes(device: DeviceId, scope: &Scope, stop: bool, ts_ms: u64) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(device, scope, stop, ts_ms))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = Vec::with_capacity(TYPING_DOMAIN.len() + body.len());
    out.extend_from_slice(TYPING_DOMAIN);
    out.extend_from_slice(&body);
    Ok(out)
}

/// One person typing in one target.
#[derive(Clone, Copy, Debug)]
struct Typer {
    /// When this stretch of typing began. Orders the names, oldest first.
    since_ms: u64,
    /// The signal time of the newest signal. A signal older than this is ignored.
    ts_ms: u64,
    /// Our clock when the newest signal arrived. Expiry is judged from this.
    seen_ms: u64,
}

impl Typer {
    fn live(&self, now: u64) -> bool {
        self.seen_ms.saturating_add(TYPING_EXPIRY_MS) > now
    }
}

/// Who is typing where, and when we last sent a signal for each target. Lives only in memory.
#[derive(Default)]
pub(crate) struct Typing {
    typers: BTreeMap<TypingTarget, BTreeMap<RootId, Typer>>,
    /// When a typing signal last went out for each target, for the send throttle.
    sent: BTreeMap<TypingTarget, Instant>,
}

impl Typing {
    /// Notes a typing signal from `root`. Returns true when the set of typers changed.
    fn start(&mut self, target: TypingTarget, root: RootId, ts_ms: u64, now: u64) -> bool {
        let slot = self.typers.entry(target).or_default();
        match slot.entry(root) {
            Entry::Occupied(mut held) => {
                let typer = held.get_mut();
                if !typer.live(now) {
                    *typer = Typer { since_ms: now, ts_ms, seen_ms: now };
                    return true;
                }
                if ts_ms > typer.ts_ms {
                    typer.ts_ms = ts_ms;
                    typer.seen_ms = now;
                }
                false
            }
            Entry::Vacant(slot) => {
                slot.insert(Typer { since_ms: now, ts_ms, seen_ms: now });
                true
            }
        }
    }

    /// Drops `root` from `target`. Returns true when they were typing there. A stop older than the
    /// newest signal from them is ignored.
    fn stop(&mut self, target: TypingTarget, root: RootId, ts_ms: u64, now: u64) -> bool {
        let stale = self
            .typers
            .get(&target)
            .and_then(|slot| slot.get(&root))
            .is_some_and(|t| t.ts_ms > ts_ms);
        if stale {
            return false;
        }
        self.remove(target, root, now)
    }

    /// Drops `root` from `target` at once, for example because their message arrived. Returns true
    /// when they were typing there.
    fn remove(&mut self, target: TypingTarget, root: RootId, now: u64) -> bool {
        let Some(slot) = self.typers.get_mut(&target) else {
            return false;
        };
        let was_live = slot.remove(&root).is_some_and(|t| t.live(now));
        if slot.is_empty() {
            self.typers.remove(&target);
        }
        was_live
    }

    /// Drops every typer whose last signal has expired. Returns the targets that lost someone.
    fn expire(&mut self, now: u64) -> Vec<TypingTarget> {
        let mut changed = Vec::new();
        self.typers.retain(|target, slot| {
            let before = slot.len();
            slot.retain(|_, t| t.live(now));
            if slot.len() != before {
                changed.push(*target);
            }
            !slot.is_empty()
        });
        changed
    }

    /// The people typing in `target` at `now`, oldest first.
    fn live(&self, target: TypingTarget, now: u64) -> Vec<RootId> {
        let Some(slot) = self.typers.get(&target) else {
            return Vec::new();
        };
        let mut live: Vec<(u64, RootId)> = slot
            .iter()
            .filter(|(_, t)| t.live(now))
            .map(|(root, t)| (t.since_ms, *root))
            .collect();
        live.sort();
        live.into_iter().map(|(_, root)| root).collect()
    }

    /// Whether a signal for `target` may go out now. Records the send when it may.
    fn may_send(&mut self, target: TypingTarget, now: Instant) -> bool {
        if let Some(last) = self.sent.get(&target) {
            if now.duration_since(*last) < TYPING_EVERY {
                return false;
            }
        }
        self.sent.insert(target, now);
        true
    }
}

/// The target a conversation message names, seen from `me`. A 1:1 conversation is a DM with its
/// other party, and anything else is a group.
pub(crate) fn conv_target(me: &RootId, conv: ConvId, author: RootId) -> TypingTarget {
    if conv == dm_conv_id(me, &author) {
        TypingTarget::Dm(author)
    } else {
        TypingTarget::Group(conv)
    }
}

/// Devices that get a typing signal for a DM or a group: the other members', minus blocked people.
fn typing_devices(w: &World, target: TypingTarget) -> Vec<DeviceId> {
    let roots: Vec<RootId> = match target {
        TypingTarget::Dm(peer) => vec![peer],
        TypingTarget::Group(conv) => w
            .conv_book
            .convs
            .get(&conv)
            .map(|m| m.members.clone())
            .unwrap_or_default(),
        TypingTarget::Channel { .. } => return Vec::new(),
    };
    let mut devices: BTreeSet<DeviceId> = BTreeSet::new();
    for root in roots {
        if root == w.me || w.contact_book.blocked.contains(&root) {
            continue;
        }
        devices.extend(w.directory.devices_of(&root));
    }
    devices.remove(&w.device);
    devices.into_iter().collect()
}

/// Checks that `root` may type in `scope` as it arrived: on `via`'s topic, or directly. Returns the
/// target it names.
fn authorize(w: &World, via: Option<CommunityId>, root: RootId, scope: Scope) -> Result<TypingTarget> {
    match (scope, via) {
        (Scope::Channel { community, channel }, Some(topic)) if community == topic => {
            if !w.communities.get(&community).is_some_and(|c| c.is_member(&root)) {
                return Err(CoreError::Invalid("typer is not a member of this community".into()));
            }
            Ok(TypingTarget::Channel { community, channel })
        }
        (Scope::Conv(conv), None) => {
            if conv == dm_conv_id(&root, &w.me) {
                if !w.is_contact(&root) {
                    return Err(CoreError::Invalid("typer is not a contact".into()));
                }
                return Ok(TypingTarget::Dm(root));
            }
            let meta = w
                .conv_book
                .convs
                .get(&conv)
                .filter(|m| !m.left)
                .ok_or_else(|| CoreError::Invalid("typing for a conversation we are not in".into()))?;
            if !meta.members.contains(&root) || !meta.members.contains(&w.me) {
                return Err(CoreError::Invalid("typer is not a member of this conversation".into()));
            }
            Ok(TypingTarget::Group(conv))
        }
        _ => Err(CoreError::Invalid("typing signal arrived by the wrong route".into())),
    }
}

impl Engine {
    /// Says we are typing in `target`. At most one signal goes out per target every `TYPING_EVERY`,
    /// and none goes out while our status is invisible. Call it on each keystroke.
    pub fn typing(&self, target: TypingTarget) -> Result<()> {
        self.signal(target, false)
    }

    /// Says we stopped typing in `target`. Call it when the message is sent or the composer is
    /// cleared.
    pub fn stop_typing(&self, target: TypingTarget) -> Result<()> {
        self.signal(target, true)
    }

    /// The display names of the people typing in `target` now, oldest first. Our own devices are
    /// never listed.
    pub fn typing_in(&self, target: TypingTarget) -> Vec<String> {
        let roots = self.lock_typing().live(target, now_ms());
        let w = self.read();
        roots.iter().map(|root| w.display_name(root)).collect()
    }

    /// Drops `author` from `target`, because their message arrived.
    pub(crate) fn clear_typer(&self, target: TypingTarget, author: RootId) {
        let changed = self.lock_typing().remove(target, author, now_ms());
        if changed {
            self.emit(EngineEvent::TypingChanged { target });
        }
    }

    /// Drops typers whose last signal has expired, and announces each target that changed.
    pub(crate) fn sweep_typing(&self) {
        let changed = self.lock_typing().expire(now_ms());
        for target in changed {
            self.emit(EngineEvent::TypingChanged { target });
        }
    }

    /// Checks a typing signal and records it. `via` is the community topic it arrived on, or `None`
    /// for a direct signal. Signals from our own devices are ignored.
    pub(crate) fn accept_typing(&self, via: Option<CommunityId>, st: SignedTyping) -> Result<()> {
        st.verify()?;
        let now = now_ms();
        if st.ts_ms.abs_diff(now) > MAX_CLOCK_SKEW_MS {
            return Err(CoreError::Invalid("typing timestamp is too far from our clock".into()));
        }
        let root = st.cert.root;
        let target = {
            let w = self.read();
            if root == w.me || w.contact_book.blocked.contains(&root) {
                return Ok(());
            }
            authorize(&w, via, root, st.scope)?
        };
        let changed = {
            let mut typing = self.lock_typing();
            if st.stop {
                typing.stop(target, root, st.ts_ms, now)
            } else {
                typing.start(target, root, st.ts_ms, now)
            }
        };
        if changed {
            self.emit(EngineEvent::TypingChanged { target });
        }
        Ok(())
    }

    fn lock_typing(&self) -> std::sync::MutexGuard<'_, Typing> {
        self.inner.typing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Signs and sends one signal for `target`, unless the throttle or invisibility holds it back.
    fn signal(&self, target: TypingTarget, stop: bool) -> Result<()> {
        let scope = self.scope_of(target)?;
        if self.status().0 == Status::Invisible {
            return Ok(());
        }
        {
            let mut typing = self.lock_typing();
            if stop {
                typing.sent.remove(&target);
            } else if !typing.may_send(target, Instant::now()) {
                return Ok(());
            }
        }
        let st = SignedTyping::sign(
            self.inner.identity.cert.clone(),
            scope,
            stop,
            now_ms(),
            &self.inner.identity.device,
        )?;
        let engine = self.clone();
        self.spawn_task(async move { engine.send_typing(target, st).await });
        Ok(())
    }

    /// The scope a target travels under. Refuses a target we cannot type in.
    fn scope_of(&self, target: TypingTarget) -> Result<Scope> {
        let me = self.inner.me;
        match target {
            TypingTarget::Channel { community, channel } => {
                let w = self.read();
                let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
                if !c.is_member(&me) {
                    return Err(CoreError::Invalid("not a member of this community".into()));
                }
                Ok(Scope::Channel { community, channel })
            }
            TypingTarget::Dm(peer) => {
                if peer == me {
                    return Err(CoreError::Invalid("cannot type to yourself".into()));
                }
                Ok(Scope::Conv(dm_conv_id(&me, &peer)))
            }
            TypingTarget::Group(conv) => {
                let w = self.read();
                match w.conv_book.convs.get(&conv) {
                    Some(meta) if !meta.left => Ok(Scope::Conv(conv)),
                    _ => Err(CoreError::NotFound(format!("conversation {conv}"))),
                }
            }
        }
    }

    /// Sends a signal on the community topic for a channel, or to each device for a DM or group.
    async fn send_typing(&self, target: TypingTarget, st: SignedTyping) {
        match target {
            TypingTarget::Channel { community, .. } => {
                let sender = self
                    .read()
                    .communities
                    .get(&community)
                    .and_then(|c| c.sender.clone());
                let Some(sender) = sender else {
                    return;
                };
                match encode_gossip(&GossipMsg::Typing(Box::new(st))) {
                    Ok(bytes) => {
                        if let Err(e) = sender.broadcast(bytes).await {
                            debug!(error = %e, "typing not broadcast on a community topic");
                        }
                    }
                    Err(e) => debug!(error = %e, "could not encode typing"),
                }
            }
            TypingTarget::Dm(_) | TypingTarget::Group(_) => {
                let devices = typing_devices(&self.read(), target);
                let bytes = match self.request_bytes(&Request::Typing(Box::new(st))) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        debug!(error = %e, "could not encode typing");
                        return;
                    }
                };
                let node = self.node();
                for device in devices {
                    let node = node.clone();
                    let bytes = bytes.clone();
                    self.spawn_task(async move {
                        if let Err(e) = node.request(device, bytes).await {
                            debug!(device = %device, error = %e, "typing not delivered directly");
                        }
                    });
                }
            }
        }
    }
}

/// Runs for the life of the engine. Once a second it drops typers whose signal has expired.
pub(crate) async fn typing_loop(weak: Weak<Inner>) {
    let mut tick = tokio::time::interval(SWEEP_EVERY);
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else {
            break;
        };
        Engine { inner }.sweep_typing();
    }
}

#[cfg(test)]
mod tests;
