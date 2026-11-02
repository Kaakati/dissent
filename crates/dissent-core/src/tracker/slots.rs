//! Upload slots: how many tracker seeds this device sends at once. Every seed is available; `SeedingSettings::max_active`
//! is the number of GET requests for seeds served at the same time. See `docs/design/14-file-tracker.md`, "Upload slots".
//!
//! A request that finds the slots full is answered at once: a downloader that knows the busy answer (`FEATURE_SEED_SLOTS`)
//! is refused with iroh-blobs' `RateLimited`, moves on to other holders and asks again later. A downloader that does not
//! know it would count the refusal as a failure and drop this device, so its request is held instead, for a while, until a
//! slot frees. A freed slot goes to whoever waited longest, so nobody keeps a slot by asking again the moment a slice ends.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use dissent_identity::DeviceId;
use tokio::sync::oneshot;

/// A served request: its connection and request ids.
pub(crate) type RequestKey = (u64, u64);

/// How long a refused downloader keeps its claim on the next free slot.
const CLAIM: Duration = Duration::from_secs(10);
/// How long a held request waits for a slot before it is refused.
pub(crate) const HOLD: Duration = Duration::from_secs(30);
/// Most requests held at once, and most from one device.
const MAX_HELD: usize = 16;
const MAX_HELD_PER_PEER: usize = 4;

/// What a request for a seed gets.
#[derive(Debug)]
pub(crate) enum Take {
    /// Served now. The slot is released when the request ends.
    Granted,
    /// Refused: the slots are full.
    Busy,
    /// Held: the receiver says when it was granted a slot. Dropped unanswered, or after `HOLD`, it is refused.
    Held(oneshot::Receiver<()>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Room {
    Free,
    Full,
    /// The device holds half the slots while others want one.
    OwnShareTaken,
}

struct Held {
    key: RequestKey,
    device: DeviceId,
    since: Instant,
    grant: oneshot::Sender<()>,
}

#[derive(Default)]
pub(crate) struct Slots {
    active: HashMap<RequestKey, DeviceId>,
    held: VecDeque<Held>,
    /// Devices refused lately, with when: they come first for the next free slot.
    claims: Vec<(DeviceId, Instant)>,
}

impl Slots {
    /// Upload transfers running now.
    pub(crate) fn active(&self) -> usize {
        self.active.len()
    }

    /// Whether `device` may take one of `max` slots now: there is one free that nobody who waited longer is owed, and
    /// it would not hold more than half of them while someone else wants one.
    fn may_take(&mut self, device: DeviceId, max: usize, now: Instant) -> Room {
        self.claims.retain(|(_, at)| now.duration_since(*at) < CLAIM);
        let free = max.saturating_sub(self.active.len());
        // Whoever started waiting before this device is owed a slot first.
        let mine_since = self.claims.iter().find(|(d, _)| *d == device).map(|(_, at)| *at);
        let before = |at: &Instant| mine_since.is_none_or(|mine| *at < mine);
        let owed = self.held.iter().filter(|h| h.device != device && before(&h.since)).count()
            + self.claims.iter().filter(|(d, at)| *d != device && before(at)).count();
        let others_want = owed > 0 || self.active.values().any(|d| *d != device);
        let mine = self.active.values().filter(|d| **d == device).count();
        if others_want && mine >= max.div_ceil(2) && max > 1 {
            Room::OwnShareTaken
        } else if free > owed {
            Room::Free
        } else {
            Room::Full
        }
    }

    /// A request for a seed from `device`. `knows_busy` says whether it understands a busy refusal.
    pub(crate) fn take(&mut self, key: RequestKey, device: DeviceId, max: usize, knows_busy: bool, now: Instant) -> Take {
        let max = max.max(1);
        let room = self.may_take(device, max, now);
        if room == Room::Free {
            self.claims.retain(|(d, _)| *d != device);
            self.active.insert(key, device);
            return Take::Granted;
        }
        if knows_busy {
            // A device that already has its share is not owed the next slot: it is already being served.
            if room == Room::Full && !self.claims.iter().any(|(d, _)| *d == device) {
                self.claims.push((device, now));
            }
            return Take::Busy;
        }
        let from_peer = self.held.iter().filter(|h| h.device == device).count();
        if self.held.len() >= MAX_HELD || from_peer >= MAX_HELD_PER_PEER {
            return Take::Busy;
        }
        let (grant, wait) = oneshot::channel();
        self.held.push_back(Held { key, device, since: now, grant });
        Take::Held(wait)
    }

    /// Ends a served request, and hands its slot on to the longest-held request that may take it.
    pub(crate) fn release(&mut self, key: RequestKey, max: usize, now: Instant) {
        self.active.remove(&key);
        self.hand_on(max.max(1), now);
    }

    /// Forgets a held request that stopped waiting.
    pub(crate) fn forget(&mut self, key: RequestKey) {
        self.held.retain(|h| h.key != key);
    }

    fn hand_on(&mut self, max: usize, now: Instant) {
        self.held.retain(|h| !h.grant.is_closed());
        while self.active.len() < max {
            let Some(i) = (0..self.held.len()).find(|i| {
                let device = self.held[*i].device;
                let mine = self.active.values().filter(|d| **d == device).count();
                let others_want = self.held.iter().any(|h| h.device != device) || self.active.values().any(|d| *d != device);
                !(others_want && mine >= max.div_ceil(2) && max > 1)
            }) else {
                break;
            };
            let Some(next) = self.held.remove(i) else { break };
            self.claims.retain(|(d, _)| *d != next.device);
            if next.grant.send(()).is_ok() {
                self.active.insert(next.key, next.device);
            }
        }
        let _ = now;
    }

    /// Requests held now.
    #[cfg(test)]
    fn held(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn granted(t: &Take) -> bool {
        matches!(t, Take::Granted)
    }

    #[test]
    fn requests_are_served_up_to_the_slots_then_refused_at_once() {
        let mut s = Slots::default();
        let now = Instant::now();
        assert!(granted(&s.take((1, 1), dev(1), 2, true, now)));
        assert!(granted(&s.take((2, 1), dev(2), 2, true, now)));
        assert!(matches!(s.take((3, 1), dev(3), 2, true, now), Take::Busy));
        assert_eq!(s.active(), 2);
        s.release((1, 1), 2, now);
        assert_eq!(s.active(), 1);
    }

    #[test]
    fn a_refused_downloader_gets_the_next_slot_before_one_that_just_finished() {
        let mut s = Slots::default();
        let now = Instant::now();
        assert!(granted(&s.take((1, 1), dev(1), 1, true, now)));
        assert!(matches!(s.take((2, 1), dev(2), 1, true, now), Take::Busy));
        s.release((1, 1), 1, now);
        // The first asks again straight away for its next slice: the slot is owed to the second.
        assert!(matches!(s.take((1, 2), dev(1), 1, true, now), Take::Busy));
        assert!(granted(&s.take((2, 2), dev(2), 1, true, now)));
        // A claim lapses, so a downloader that went away does not block the slot for good.
        s.release((2, 2), 1, now);
        assert!(matches!(s.take((3, 1), dev(3), 1, true, now), Take::Busy), "the first is owed it now");
        let later = now + CLAIM + Duration::from_secs(1);
        assert!(granted(&s.take((3, 2), dev(3), 1, true, later)));
    }

    #[test]
    fn no_peer_takes_more_than_half_while_someone_else_wants_one() {
        let mut s = Slots::default();
        let now = Instant::now();
        // Alone, a peer may use every slot.
        for r in 0..4 {
            assert!(granted(&s.take((1, r), dev(1), 4, true, now)));
        }
        let mut s = Slots::default();
        assert!(granted(&s.take((1, 1), dev(1), 4, true, now)));
        assert!(granted(&s.take((2, 1), dev(2), 4, true, now)));
        assert!(granted(&s.take((1, 2), dev(1), 4, true, now)));
        assert!(matches!(s.take((1, 3), dev(1), 4, true, now), Take::Busy), "two of four is its half");
        assert!(granted(&s.take((2, 2), dev(2), 4, true, now)));
    }

    #[test]
    fn a_downloader_that_does_not_know_busy_is_held_and_granted_when_a_slot_frees() {
        let mut s = Slots::default();
        let now = Instant::now();
        assert!(granted(&s.take((1, 1), dev(1), 1, true, now)));
        let Take::Held(mut wait) = s.take((2, 1), dev(2), 1, false, now) else { panic!("held") };
        assert_eq!(s.held(), 1);
        assert!(wait.try_recv().is_err(), "not yet");
        // A new client asking meanwhile does not jump the queue.
        assert!(matches!(s.take((3, 1), dev(3), 1, true, now), Take::Busy));
        s.release((1, 1), 1, now);
        assert!(wait.try_recv().is_ok(), "granted when the slot freed");
        assert_eq!(s.active(), 1);
        assert_eq!(s.held(), 0);
    }

    #[test]
    fn the_hold_is_bounded_and_a_request_that_stopped_waiting_is_skipped() {
        let mut s = Slots::default();
        let now = Instant::now();
        assert!(granted(&s.take((0, 0), dev(9), 1, true, now)));
        let mut waits = Vec::new();
        for r in 0..MAX_HELD_PER_PEER as u64 {
            let Take::Held(w) = s.take((1, r), dev(1), 1, false, now) else { panic!("held") };
            waits.push(w);
        }
        assert!(matches!(s.take((1, 99), dev(1), 1, false, now), Take::Busy), "one device's share of the hold");
        for n in 2..(2 + (MAX_HELD - MAX_HELD_PER_PEER) as u8) {
            assert!(matches!(s.take((n as u64, 0), dev(n), 1, false, now), Take::Held(_)));
        }
        assert!(matches!(s.take((50, 0), dev(50), 1, false, now), Take::Busy), "the whole hold is full");
        // The first held request gave up: the slot goes to the next one still waiting.
        let first = waits.remove(0);
        drop(first);
        s.forget((1, 0));
        s.release((0, 0), 1, now);
        let mut second = waits.remove(0);
        assert!(second.try_recv().is_ok());
    }
}
