//! Repairs a call link that died mid-call. A link is made at join, at an answered join and at a tier move only, so a
//! peer whose link died to a network change, a NAT rebinding or sleep stayed unheard until the next move. Now the side
//! that offers for the pair (`Room::offers_to`: the lower sender id in a mesh, the leaf in a forwarded call) offers
//! again once the link has been dead for `Timings::relink_after`, and again with a growing wait while it stays dead.
//!
//! A link is dead when the other side would let a new offer replace it: the test `replace_if_stale` applies, so the two
//! sides never disagree. The other side only waits: its call is replaced when the offer lands. Both offering would make
//! each replace the other's new call, and break the pair.
//!
//! This repairs one pair inside the right room. A device that is in the wrong room, or has lost most of its links, is
//! brought back by `rejoin`, which starts later (`Timings::rejoin_after`). An offer refused as "not in this call" leaves
//! the pair to it. A mesh pair relinks only with a device that sent `FEATURE_RELINK`: an older one cannot replace a dead
//! call when the offer comes. A forwarder of any version takes a leaf's new offer.
//!
//! A relink stays in its room, so it does not start `Timings::settle_after_swap`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dissent_identity::DeviceId;
use dissent_media::{LinkTransport, Mesh};
use tracing::debug;

use super::{channel_call_devices, lock, JoinAnswer, Role, Room, Scope, FEATURE_RELINK, STALE_LINK};
use crate::engine::{now_ms, Engine};
use crate::timings::millis;

/// One dead pair this device offers for.
#[derive(Debug, Clone)]
pub(crate) struct DeadPair {
    attempts: u32,
    next_at: Instant,
    in_flight: bool,
    /// An offer was refused as not in this call: the room is not the peer's, which `rejoin` repairs.
    given_up: bool,
}

/// A device of this channel call that this device asks to join: one whose `Join` went unanswered, or that says it is
/// in the call while this device does not have it live. Asked again with `backoff` until it answers.
#[derive(Debug, Clone)]
pub(crate) struct JoinAsk {
    attempts: u32,
    next_at: Instant,
    in_flight: bool,
    /// It answered that it is not in this call: asked no more, so a peer's word cannot make this device ask a stranger
    /// again and again.
    given_up: bool,
}

impl JoinAsk {
    fn new(next_at: Instant) -> Self {
        Self { attempts: 0, next_at, in_flight: false, given_up: false }
    }
}

/// The wait after `attempts` offers: `first`, doubling each time, up to `cap`.
pub(crate) fn backoff(first: Duration, cap: Duration, attempts: u32) -> Duration {
    first.saturating_mul(1u32 << attempts.min(16)).min(cap)
}

/// What to do for one pair now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The link works: forget the pair.
    Clear,
    /// Nothing yet.
    Wait,
    /// Offer again now.
    Offer,
}

/// Decides the step for a pair, and updates its state.
pub(crate) fn step(state: &mut Option<DeadPair>, dead: bool, now: Instant, after: Duration) -> Step {
    if !dead {
        *state = None;
        return Step::Clear;
    }
    let pair = state.get_or_insert(DeadPair { attempts: 0, next_at: now + after, in_flight: false, given_up: false });
    if pair.in_flight || pair.given_up || now < pair.next_at {
        return Step::Wait;
    }
    pair.in_flight = true;
    Step::Offer
}

/// Records how an offer for a pair went, and when the next may go.
pub(crate) fn offered(pair: &mut DeadPair, now: Instant, after: Duration, cap: Duration, refused_as_elsewhere: bool) {
    pair.in_flight = false;
    pair.attempts += 1;
    pair.given_up = refused_as_elsewhere;
    pair.next_at = now + backoff(after, cap, pair.attempts);
}

impl Engine {
    /// Offers again for each dead link of `room` this device offers for, and drops a forwarder's downlink that has
    /// been silent for `Timings::relink_drop_after`. Called from `check_membership`, under the mesh lock.
    pub(super) async fn relink_check(&self, room: &Arc<Room>, mesh: &mut Mesh<LinkTransport>) {
        #[cfg(feature = "test-hooks")]
        self.apply_relink_hooks(room, mesh).await;
        // A move or a rejoin under way takes this device to another room, where its links are made afresh.
        if room.migrating.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let now = Instant::now();
        let grace = self.inner.timings.connect_grace;
        // (sid, device, dead) for each pair this device offers for.
        let pairs: Vec<(u64, DeviceId, bool)> = match room.forwarding.as_ref().map(|f| f.role) {
            None => {
                let me = self.inner.identity.device_id();
                let live: Vec<(u64, DeviceId)> = lock(&room.live).iter().map(|(s, d)| (*s, *d)).collect();
                let features = lock(&room.features).clone();
                live.into_iter()
                    .filter(|(_, device)| *device != me && room.offers_to(device))
                    .filter(|(sid, _)| features.get(sid).is_some_and(|(bits, _)| bits & FEATURE_RELINK != 0))
                    .map(|(sid, device)| (sid, device, mesh.is_stale(sid, STALE_LINK, grace)))
                    .collect()
            }
            Some(Role::Leaf) => {
                let Some(forwarding) = room.forwarding.as_ref() else {
                    return;
                };
                let device = forwarding.volunteer.map(|(device, _)| device).or_else(|| room.forwarder_device());
                let dead = forwarding.leaf.lock().await.as_ref().is_some_and(|p| p.is_stale(STALE_LINK, grace));
                device.map(|d| (forwarding.forwarder, d, dead)).into_iter().collect()
            }
            Some(Role::Root) => {
                self.drop_silent_downlinks(room).await;
                Vec::new()
            }
            // A cascade has its own failover.
            Some(Role::Child | Role::Standby) => Vec::new(),
        };
        let after = self.inner.timings.relink_after;
        let mut offers = Vec::new();
        {
            let mut state = lock(&room.relink);
            let offered_for: Vec<u64> = pairs.iter().map(|(sid, ..)| *sid).collect();
            state.retain(|sid, _| offered_for.contains(sid));
            for (sid, device, dead) in pairs {
                let mut entry = state.remove(&sid);
                if step(&mut entry, dead, now, after) == Step::Offer {
                    offers.push((sid, device));
                }
                if let Some(entry) = entry {
                    state.insert(sid, entry);
                }
            }
        }
        for (sid, device) in offers {
            #[cfg(feature = "test-hooks")]
            {
                *lock(&room.relink_tries).entry(sid).or_default() += 1;
            }
            let (engine, room) = (self.clone(), room.clone());
            self.spawn_task(async move {
                let outcome = engine.connect(room.clone(), device).await;
                let elsewhere = outcome.as_ref().is_err_and(|e| e.to_string().contains("not in this call"));
                match &outcome {
                    Ok(()) => debug!(peer = sid, "relinked a dead call link"),
                    Err(e) => debug!(peer = sid, error = %e, "relink not made"),
                }
                let timings = &engine.inner.timings;
                if let Some(pair) = lock(&room.relink).get_mut(&sid) {
                    offered(pair, Instant::now(), timings.relink_after, timings.relink_backoff_cap, elsewhere);
                }
            });
        }
    }

    /// Records a channel-call `Join` that got no answer, so it is asked again (`join_reconcile`), and notes it.
    pub(super) fn note_join_unanswered(&self, room: &Arc<Room>, device: DeviceId) {
        #[cfg(feature = "test-hooks")]
        if self.inner.calls.no_join_retry.load(std::sync::atomic::Ordering::Relaxed) {
            self.note_tier(format!("join to {} unanswered", &device.to_string()[..8]));
            return;
        }
        let first = Instant::now() + self.inner.timings.relink_after;
        lock(&room.join_asks).entry(device).or_insert_with(|| JoinAsk::new(first));
        self.note_tier(format!("join to {} unanswered", &device.to_string()[..8]));
    }

    /// Asks again the devices of this channel call that should be linked with this one and are not: a `Join` that got
    /// no answer, and a device whose own presence says it is in this room while this device does not have it live, as
    /// when its `Join` reached this device but the answer was lost. Without this, one lost request kept the pair apart
    /// for the whole call, each in it without the other. Only members of this channel, by this device's own state, are
    /// asked, each at most once per backoff step, and one that answers it is not in the call is asked no more.
    pub(super) fn join_reconcile(&self, room: &Arc<Room>) {
        let Scope::Channel { community, .. } = room.scope else {
            return;
        };
        if room.migrating.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let me = self.inner.identity.device_id();
        let live: BTreeSet<DeviceId> = lock(&room.live).values().copied().collect();
        let ttl = millis(self.inner.timings.call_presence_ttl);
        let now_wall = now_ms();
        let claimants: Vec<DeviceId> = lock(&self.inner.calls.heard)
            .iter()
            .filter(|(d, h)| h.room == room.id && now_wall.saturating_sub(h.seen_ms) < ttl && **d != me && !live.contains(d))
            .map(|(d, _)| *d)
            .collect();
        let members: BTreeSet<DeviceId> = {
            let w = self.read();
            w.communities
                .get(&community)
                .map(|c| channel_call_devices(&w, c, false).into_iter().collect())
                .unwrap_or_default()
        };
        let now = Instant::now();
        let first = now + self.inner.timings.relink_after;
        let due: Vec<DeviceId> = {
            let mut asks = lock(&room.join_asks);
            asks.retain(|d, _| members.contains(d) && !live.contains(d));
            for device in claimants.into_iter().filter(|d| members.contains(d)) {
                asks.entry(device).or_insert_with(|| JoinAsk::new(first));
            }
            asks.iter_mut()
                .filter(|(_, ask)| !ask.in_flight && !ask.given_up && now >= ask.next_at)
                .map(|(device, ask)| {
                    ask.in_flight = true;
                    *device
                })
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let Ok(from) = self.my_addr() else {
            return;
        };
        let muted = room.is_muted();
        for device in due {
            let msg = self.join_msg(room, None, muted, false, from.clone());
            let (engine, room) = (self.clone(), room.clone());
            self.spawn_task(async move {
                let answer = engine.join_one(room.clone(), device, msg).await;
                let timings = &engine.inner.timings;
                {
                    let mut asks = lock(&room.join_asks);
                    match answer {
                        JoinAnswer::Here => {
                            asks.remove(&device);
                        }
                        JoinAnswer::NotIn => {
                            if let Some(ask) = asks.get_mut(&device) {
                                ask.in_flight = false;
                                ask.given_up = true;
                            }
                        }
                        JoinAnswer::Unanswered => {
                            if let Some(ask) = asks.get_mut(&device) {
                                ask.in_flight = false;
                                ask.attempts += 1;
                                ask.next_at = Instant::now() + backoff(timings.relink_after, timings.relink_backoff_cap, ask.attempts);
                            }
                        }
                    }
                }
                match answer {
                    JoinAnswer::Here => engine.note_tier(format!("asked {} to join again: linked", &device.to_string()[..8])),
                    JoinAnswer::Unanswered => engine.note_tier(format!("join to {} unanswered again", &device.to_string()[..8])),
                    JoinAnswer::NotIn => {}
                }
            });
        }
    }

    /// On a forwarder: drops a downlink that has heard nothing for `Timings::relink_drop_after`. Its leaf offers again
    /// when its own side sees the link dead.
    async fn drop_silent_downlinks(&self, room: &Arc<Room>) {
        let Some(forwarding) = room.forwarding.as_ref() else {
            return;
        };
        let limit = self.inner.timings.relink_drop_after;
        let mut sfu = forwarding.sfu.lock().await;
        let Some(sfu) = sfu.as_mut() else {
            return;
        };
        let silent: Vec<u64> = sfu
            .downlinks()
            .into_iter()
            .filter(|d| *d != room.my_sid && sfu.downlink_quiet(*d).is_some_and(|quiet| quiet >= limit))
            .collect();
        for downlink in silent {
            debug!(downlink, "dropping a downlink silent past the relink limit");
            sfu.remove_downlink(downlink);
        }
    }

    /// Test hook: this device's current room, and the sender id of `root` in it.
    #[cfg(feature = "test-hooks")]
    fn room_and_sid(&self, root: &str) -> Option<(Arc<Room>, u64)> {
        let room = lock(&self.inner.calls.current).clone()?;
        let sid = room.sid_of_root(root)?;
        Some((room, sid))
    }

    /// Test hook: drops `percent` of the datagrams this device's mesh call to `root` sends. False without that peer.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn drop_mesh_media_to_now(&self, root: &str, percent: u32) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        // A 0 stays in the map, so the next tick also clears what the mesh holds for that peer.
        lock(&room.drop_to).insert(root.to_string(), percent.min(100));
        true
    }

    /// Test hook: closes this device's media link to `root`, as a network change does. False without one.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn close_media_link_now(&self, root: &str) -> bool {
        let Some((room, sid)) = self.room_and_sid(root) else {
            return false;
        };
        let Some(device) = room.device_of(sid) else {
            return false;
        };
        let link = lock(&room.media_links).get(&device).cloned();
        match link {
            Some(link) => {
                link.close();
                true
            }
            None => false,
        }
    }

    /// Test hook: drops `percent` of the datagrams this device's leaf links send. False without a call.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn drop_leaf_media_now(&self, percent: u32) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        room.leaf_drop.store(percent.min(100), std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// Test hook: the relink offers this device made for its link to `root` in the current room.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn relink_attempts_now(&self, root: &str) -> u32 {
        self.room_and_sid(root).map_or(0, |(room, sid)| lock(&room.relink_tries).get(&sid).copied().unwrap_or(0))
    }

    /// Test hook: whether this device's leaf link to its forwarder works. False without a forwarded call.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn leaf_link_live_now(&self) -> bool {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return false;
        };
        let Some(forwarding) = room.forwarding.as_ref() else {
            return false;
        };
        let live = forwarding.leaf.lock().await.as_ref().is_some_and(|p| p.live_links(STALE_LINK) > 0);
        live
    }

    /// Test hook: the downlinks this device's relay holds, its own leaf included. Zero when it does not forward.
    #[cfg(feature = "test-hooks")]
    pub(crate) async fn forwarder_downlinks_now(&self) -> usize {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return 0;
        };
        let Some(forwarding) = room.forwarding.as_ref() else {
            return 0;
        };
        let count = forwarding.sfu.lock().await.as_ref().map_or(0, |sfu| sfu.downlinks().len());
        count
    }

    /// Test hooks: the datagrams this device drops toward one mesh peer, and on its leaf links.
    #[cfg(feature = "test-hooks")]
    async fn apply_relink_hooks(&self, room: &Arc<Room>, mesh: &mut Mesh<LinkTransport>) {
        let drops = lock(&room.drop_to).clone();
        for (root, percent) in drops {
            if let Some(sid) = room.sid_of_root(&root) {
                mesh.set_drop_percent_for(sid, percent);
            }
        }
        let leaf_drop = room.leaf_drop.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(forwarding) = room.forwarding.as_ref() {
            if let Some(participant) = forwarding.leaf.lock().await.as_mut() {
                participant.set_drop_percent(leaf_drop);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: Duration = Duration::from_secs(4);
    const CAP: Duration = Duration::from_secs(30);

    /// The bit is on the wire: older and newer clients must agree on it, and it must not share one with another feature
    /// (64 is the link tokens').
    #[test]
    fn the_relink_feature_bit_is_pinned() {
        assert_eq!(FEATURE_RELINK, 128);
        assert_ne!(super::super::FEATURES & FEATURE_RELINK, 0, "this device announces it");
    }

    #[test]
    fn the_wait_doubles_from_the_first_up_to_the_cap() {
        let waits: Vec<u64> = (0..6).map(|n| backoff(AFTER, CAP, n).as_secs()).collect();
        assert_eq!(waits, vec![4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_dead_pair_offers_once_it_has_been_dead_long_enough_and_then_backs_off() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut pair = None;
        assert_eq!(step(&mut pair, true, at(0), AFTER), Step::Wait, "not dead long enough");
        assert_eq!(step(&mut pair, true, at(3), AFTER), Step::Wait);
        assert_eq!(step(&mut pair, true, at(4), AFTER), Step::Offer);
        assert_eq!(step(&mut pair, true, at(5), AFTER), Step::Wait, "one offer in flight at a time");
        offered(pair.as_mut().unwrap(), at(5), AFTER, CAP, false);
        assert_eq!(step(&mut pair, true, at(12), AFTER), Step::Wait, "the next waits 8 s");
        assert_eq!(step(&mut pair, true, at(13), AFTER), Step::Offer);
    }

    #[test]
    fn a_link_that_works_again_forgets_the_pair() {
        let t0 = Instant::now();
        let mut pair = None;
        step(&mut pair, true, t0, AFTER);
        assert_eq!(step(&mut pair, false, t0 + Duration::from_secs(1), AFTER), Step::Clear);
        assert!(pair.is_none());
        assert_eq!(step(&mut pair, true, t0 + Duration::from_secs(2), AFTER), Step::Wait, "a new death waits again");
    }

    #[test]
    fn a_refusal_as_not_in_this_call_leaves_the_pair_to_rejoin() {
        let t0 = Instant::now();
        let mut pair = None;
        assert_eq!(step(&mut pair, true, t0 + AFTER, AFTER), Step::Wait);
        assert_eq!(step(&mut pair, true, t0 + AFTER * 2, AFTER), Step::Offer);
        offered(pair.as_mut().unwrap(), t0 + AFTER * 2, AFTER, CAP, true);
        assert_eq!(step(&mut pair, true, t0 + Duration::from_secs(600), AFTER), Step::Wait);
    }
}
