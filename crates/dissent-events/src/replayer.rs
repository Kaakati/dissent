//! Incremental replay. New events are applied at the end of the causal order.
//! An event that sorts before the applied prefix triggers a rewind to the newest
//! snapshot before it, and the suffix is re-applied.
//!
//! Callers must pass every id that `Dag::insert` reports as entered to
//! [`Replayer::on_insert`]. The replayer then matches [`crate::replay`] exactly.

use crate::control::{open_event, SealKey};
use crate::dag::OrderKey;
use crate::state::{apply_event, CommunityState, Rejection};
use crate::{Dag, Event, EventId};
use dissent_identity::RootId;

/// Take a snapshot after this many applied events.
const SNAPSHOT_EVERY: usize = 1024;
/// Keep at most this many snapshots. The oldest is dropped first.
const MAX_SNAPSHOTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayStats {
    /// Times an out-of-order event forced a rewind.
    pub rewinds: u64,
    /// Rewinds that restarted from a snapshot rather than from scratch.
    pub snapshot_restores: u64,
    pub snapshots_taken: u64,
}

#[derive(Clone, Debug)]
pub struct Replayer {
    owner: RootId,
    /// Keys that open sealed control events. Changing them rebuilds the state.
    ring: Vec<SealKey>,
    state: CommunityState,
    /// Key of the last applied event. Every stored event at or below it is applied.
    applied_up_to: Option<OrderKey>,
    /// State after applying events up to each key, oldest first.
    snapshots: Vec<(OrderKey, CommunityState)>,
    since_snapshot: usize,
    stats: ReplayStats,
}

impl Replayer {
    pub fn new(owner: RootId) -> Self {
        Self {
            owner,
            ring: Vec::new(),
            state: CommunityState::new(owner),
            applied_up_to: None,
            snapshots: Vec::new(),
            since_snapshot: 0,
            stats: ReplayStats::default(),
        }
    }

    pub fn state(&self) -> &CommunityState {
        &self.state
    }

    /// The keys this replay opens sealed control events with.
    pub fn ring(&self) -> &[SealKey] {
        &self.ring
    }

    /// Replaces the keys and rebuilds the state from `dag`, if they changed. A sealed event that
    /// was locked may open now, and an opened one may lock again if its key is dropped.
    pub fn set_ring(&mut self, dag: &Dag, ring: Vec<SealKey>) {
        if self.ring == ring {
            return;
        }
        self.ring = ring;
        self.apply_all(dag);
    }

    pub fn stats(&self) -> ReplayStats {
        self.stats
    }

    /// Discard the state and rebuild it from every event in `dag`.
    pub fn apply_all(&mut self, dag: &Dag) {
        self.state = CommunityState::new(self.owner);
        self.applied_up_to = None;
        self.snapshots.clear();
        self.since_snapshot = 0;
        self.apply_after(dag, None);
    }

    /// Account for events that `dag` just stored. `inserted` holds their ids.
    pub fn on_insert(&mut self, dag: &Dag, inserted: &[EventId]) {
        let mut keys: Vec<OrderKey> = inserted
            .iter()
            .filter_map(|id| dag.get(id).map(|e| (e.body.hlc, *id)))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let Some(&min) = keys.first() else {
            return;
        };
        match self.applied_up_to {
            Some(applied) if min <= applied => self.rewind(dag, min),
            _ => {
                for key in keys {
                    if let Some(ev) = dag.get(&key.1) {
                        self.apply_one(key, ev);
                    }
                }
            }
        }
    }

    /// Would `ev` be rejected if it were appended now? Reads only, never mutates.
    /// `ev` must sort after the applied prefix. Otherwise the answer is not exact.
    pub fn would_reject(&self, ev: &Event) -> Option<Rejection> {
        debug_assert!(self.applied_up_to.is_none_or(|k| (ev.body.hlc, ev.id) > k));
        match open_event(ev, &self.ring) {
            Ok(plain) => self.state.check(&plain).err(),
            Err(why) => Some(why.into()),
        }
    }

    /// Restore the newest snapshot before `min`, or start over, then re-apply forward.
    fn rewind(&mut self, dag: &Dag, min: OrderKey) {
        self.stats.rewinds += 1;
        // Snapshots at or after `min` saw events that are now out of order.
        self.snapshots.retain(|(k, _)| *k < min);
        match self.snapshots.last() {
            Some((k, st)) => {
                self.state = st.clone();
                self.applied_up_to = Some(*k);
                self.stats.snapshot_restores += 1;
            }
            None => {
                self.state = CommunityState::new(self.owner);
                self.applied_up_to = None;
            }
        }
        self.apply_after(dag, self.applied_up_to);
    }

    fn apply_after(&mut self, dag: &Dag, after: Option<OrderKey>) {
        for (key, ev) in dag.ordered_after(after) {
            self.apply_one(key, ev);
        }
    }

    fn apply_one(&mut self, key: OrderKey, ev: &Event) {
        apply_event(&mut self.state, ev, &self.ring);
        self.applied_up_to = Some(key);
        self.since_snapshot += 1;
        if self.since_snapshot >= SNAPSHOT_EVERY {
            self.since_snapshot = 0;
            self.snapshots.push((key, self.state.clone()));
            self.stats.snapshots_taken += 1;
            if self.snapshots.len() > MAX_SNAPSHOTS {
                self.snapshots.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{after, event, on_channel, User};
    use crate::{ChannelId, Hlc, Payload};

    fn at(wall_ms: u64) -> Hlc {
        Hlc {
            wall_ms,
            counter: 0,
        }
    }

    fn bytes(st: &CommunityState) -> Vec<u8> {
        postcard::to_allocvec(st).unwrap()
    }

    #[test]
    fn reverse_arrivals_release_and_match_replay() {
        let owner = User::new();
        let g = event(
            &owner,
            vec![],
            at(1),
            Payload::ChannelCreate { name: "g".into() },
        );
        let chan = ChannelId(g.id.0);
        let mut evs = vec![g];
        for i in 0..30u64 {
            let p = evs.last().unwrap();
            let hlc = after(&[p.body.hlc], 2 + i);
            let next = on_channel(
                &owner,
                chan,
                vec![p.id],
                hlc,
                Payload::Message {
                    text: format!("{i}"),
                },
            );
            evs.push(next);
        }

        let mut dag = Dag::new();
        let mut rep = Replayer::new(owner.id());
        for ev in evs.iter().rev() {
            if let crate::InsertOutcome::Inserted { entered } = dag.insert(ev.clone()).unwrap() {
                rep.on_insert(&dag, &entered);
            }
        }
        assert_eq!(dag.len(), 31);
        assert_eq!(bytes(rep.state()), bytes(&crate::replay(owner.id(), &dag)));
    }

    #[test]
    fn late_event_rewinds_and_matches_replay() {
        let owner = User::new();
        let g = event(
            &owner,
            vec![],
            at(1),
            Payload::ChannelCreate { name: "g".into() },
        );
        let chan = ChannelId(g.id.0);
        let mut dag = Dag::new();
        let mut rep = Replayer::new(owner.id());
        dag.insert(g.clone()).unwrap();
        rep.on_insert(&dag, &[g.id]);

        let mut prev = g.clone();
        for w in [2u64, 3, 4] {
            let ev = on_channel(
                &owner,
                chan,
                vec![prev.id],
                after(&[prev.body.hlc], w),
                Payload::Message {
                    text: format!("a{w}"),
                },
            );
            dag.insert(ev.clone()).unwrap();
            rep.on_insert(&dag, &[ev.id]);
            prev = ev;
        }
        // Concurrent with the chain (parent `g`) but sorts before its tail.
        let late = on_channel(
            &owner,
            chan,
            vec![g.id],
            after(&[g.body.hlc], 2),
            Payload::Message {
                text: "late".into(),
            },
        );
        match dag.insert(late.clone()).unwrap() {
            crate::InsertOutcome::Inserted { entered } => rep.on_insert(&dag, &entered),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(rep.stats().rewinds, 1);
        assert_eq!(bytes(rep.state()), bytes(&crate::replay(owner.id(), &dag)));
    }

    #[test]
    fn would_reject_reports_ban_without_mutating() {
        let (owner, user) = (User::new(), User::new());
        let g = event(
            &owner,
            vec![],
            at(1),
            Payload::ChannelCreate { name: "g".into() },
        );
        let chan = ChannelId(g.id.0);
        let ban = event(&owner, vec![g.id], at(2), Payload::Ban { user: user.id() });
        let mut dag = Dag::new();
        let mut rep = Replayer::new(owner.id());
        for ev in [&g, &ban] {
            dag.insert(ev.clone()).unwrap();
        }
        rep.on_insert(&dag, &[g.id, ban.id]);

        let candidate = |u: &User, hlc| {
            on_channel(
                u,
                chan,
                vec![ban.id],
                hlc,
                Payload::Message { text: "hi".into() },
            )
        };
        let before = bytes(rep.state());
        let banned = candidate(&user, after(&[ban.body.hlc], 3));
        assert_eq!(rep.would_reject(&banned), Some(Rejection::Banned));
        let ok = candidate(&owner, after(&[ban.body.hlc], 3));
        assert_eq!(rep.would_reject(&ok), None);
        assert_eq!(bytes(rep.state()), before, "would_reject must not mutate");
    }
}
