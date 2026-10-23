//! In-memory event store. Events wait here until all their parents have arrived.
//!
//! The store checks that each event's id matches its body and that its HLC is
//! strictly greater than every parent's HLC. Signatures need a device
//! certificate, so callers run [`crate::Event::verify`] first.
//!
//! Because each event sorts after its parents, sorting by `(hlc, id)` is a
//! valid topological order. The store keeps that order as an index.

use crate::{Error, Event, EventId, Hlc, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};

/// Position of a stored event in causal order.
pub type OrderKey = (Hlc, EventId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertOutcome {
    /// Stored. `entered` lists the inserted event first, then any pending events
    /// it released, in the order they were stored. Every id here is new to the DAG.
    Inserted { entered: Vec<EventId> },
    /// Already stored or already pending.
    Duplicate,
    /// Held until these parents (not yet stored) arrive.
    Pending { missing: Vec<EventId> },
}

#[derive(Clone, Debug, Default)]
pub struct Dag {
    events: BTreeMap<EventId, Event>,
    order: BTreeSet<OrderKey>,
    pending: BTreeMap<EventId, Event>,
    /// Parent id that is not stored yet, mapped to the pending children waiting on it.
    waiting: BTreeMap<EventId, BTreeSet<EventId>>,
}

impl Dag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, ev: Event) -> Result<InsertOutcome> {
        if EventId::of(&ev.body) != ev.id {
            return Err(Error::IdMismatch);
        }
        if self.events.contains_key(&ev.id) || self.pending.contains_key(&ev.id) {
            return Ok(InsertOutcome::Duplicate);
        }
        let missing: Vec<EventId> = ev
            .body
            .parents
            .iter()
            .filter(|p| !self.events.contains_key(p))
            .copied()
            .collect();
        if missing.is_empty() {
            if !self.after_parents(&ev) {
                self.drop_children_of(ev.id);
                return Err(Error::HlcNotAfterParents);
            }
            let entered = self.accept(ev);
            return Ok(InsertOutcome::Inserted { entered });
        }
        for p in &missing {
            self.waiting.entry(*p).or_default().insert(ev.id);
        }
        self.pending.insert(ev.id, ev);
        Ok(InsertOutcome::Pending { missing })
    }

    /// True when every parent of `ev` is stored and has a lower HLC.
    fn after_parents(&self, ev: &Event) -> bool {
        ev.body.parents.iter().all(|p| {
            self.events
                .get(p)
                .is_none_or(|pe| pe.body.hlc < ev.body.hlc)
        })
    }

    /// Store `first`, then release pending children whose parents are now all stored.
    /// A child that breaks the HLC rule is dropped with its own pending descendants.
    fn accept(&mut self, first: Event) -> Vec<EventId> {
        let mut entered = Vec::new();
        let mut ready = vec![first];
        while let Some(ev) = ready.pop() {
            let id = ev.id;
            self.order.insert((ev.body.hlc, id));
            self.events.insert(id, ev);
            entered.push(id);
            for child in self.waiting.remove(&id).unwrap_or_default() {
                let all_parents = self
                    .pending
                    .get(&child)
                    .is_some_and(|c| c.body.parents.iter().all(|p| self.events.contains_key(p)));
                if !all_parents {
                    continue;
                }
                let ok = self
                    .pending
                    .get(&child)
                    .is_some_and(|c| self.after_parents(c));
                if let Some(c) = self.pending.remove(&child) {
                    if ok {
                        ready.push(c);
                    } else {
                        self.drop_children_of(child);
                    }
                }
            }
        }
        entered
    }

    /// Remove a pending event and, recursively, every pending event waiting on it.
    fn drop_children_of(&mut self, id: EventId) {
        for child in self.waiting.remove(&id).unwrap_or_default() {
            self.drop_pending(child);
        }
    }

    fn drop_pending(&mut self, id: EventId) {
        let Some(ev) = self.pending.remove(&id) else {
            return;
        };
        for p in &ev.body.parents {
            if let Some(set) = self.waiting.get_mut(p) {
                set.remove(&id);
                if set.is_empty() {
                    self.waiting.remove(p);
                }
            }
        }
        self.drop_children_of(id);
    }

    pub fn get(&self, id: &EventId) -> Option<&Event> {
        self.events.get(id)
    }

    /// Stored events that no stored event references, sorted.
    pub fn heads(&self) -> Vec<EventId> {
        let referenced: BTreeSet<EventId> = self
            .events
            .values()
            .flat_map(|e| e.body.parents.iter().copied())
            .collect();
        self.events
            .keys()
            .filter(|id| !referenced.contains(id))
            .copied()
            .collect()
    }

    /// Parents that pending events need and that are neither stored nor pending.
    pub fn missing(&self) -> Vec<EventId> {
        self.waiting
            .keys()
            .filter(|p| !self.pending.contains_key(p))
            .copied()
            .collect()
    }

    /// Number of stored events. Pending events are not counted.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Stored events in causal order, which is `(hlc, id)` ascending.
    pub fn causal_order(&self) -> Vec<&Event> {
        self.order.iter().map(|(_, id)| &self.events[id]).collect()
    }

    /// Stored events strictly after `after` in causal order, with their keys.
    pub(crate) fn ordered_after(
        &self,
        after: Option<OrderKey>,
    ) -> impl Iterator<Item = (OrderKey, &Event)> + '_ {
        let lower = match after {
            Some(k) => Excluded(k),
            None => Unbounded,
        };
        self.order
            .range((lower, Unbounded))
            .map(|k| (*k, &self.events[&k.1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{after, event, User};
    use crate::Payload;
    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::SeedableRng;

    fn msg(i: u64) -> Payload {
        Payload::Message {
            text: format!("m{i}"),
        }
    }

    fn at(wall_ms: u64) -> Hlc {
        Hlc {
            wall_ms,
            counter: 0,
        }
    }

    #[test]
    fn out_of_order_insert_releases_pending_chain() {
        let u = User::new();
        let a = event(&u, vec![], at(1), msg(0));
        let b = event(&u, vec![a.id], at(2), msg(1));
        let c = event(&u, vec![b.id], at(3), msg(2));

        assert_eq!(
            Dag::new().insert(c.clone()).unwrap(),
            InsertOutcome::Pending {
                missing: vec![b.id]
            }
        );
        let mut dag = Dag::new();
        dag.insert(c.clone()).unwrap();
        assert_eq!(dag.missing(), vec![b.id]);
        assert_eq!(
            dag.insert(b.clone()).unwrap(),
            InsertOutcome::Pending {
                missing: vec![a.id]
            }
        );
        assert_eq!(dag.missing(), vec![a.id]);
        assert!(dag.is_empty());
        assert_eq!(dag.insert(c.clone()).unwrap(), InsertOutcome::Duplicate);

        assert_eq!(
            dag.insert(a.clone()).unwrap(),
            InsertOutcome::Inserted {
                entered: vec![a.id, b.id, c.id]
            }
        );
        assert_eq!(dag.len(), 3);
        assert!(dag.missing().is_empty());
        assert_eq!(dag.heads(), vec![c.id]);
        assert_eq!(dag.insert(b).unwrap(), InsertOutcome::Duplicate);
    }

    #[test]
    fn diamond_waits_for_both_parents() {
        let u = User::new();
        let root = event(&u, vec![], at(1), msg(0));
        let l = event(&u, vec![root.id], at(2), msg(1));
        let r = event(&u, vec![root.id], at(2), msg(2));
        let join = event(&u, vec![l.id, r.id], at(3), msg(3));

        let mut dag = Dag::new();
        dag.insert(root.clone()).unwrap();
        dag.insert(join.clone()).unwrap();
        dag.insert(l.clone()).unwrap();
        assert_eq!(dag.len(), 2);
        assert_eq!(
            dag.insert(r.clone()).unwrap(),
            InsertOutcome::Inserted {
                entered: vec![r.id, join.id]
            }
        );
        assert_eq!(dag.len(), 4);
        assert_eq!(dag.heads(), vec![join.id]);
    }

    #[test]
    fn rejects_id_mismatch() {
        let u = User::new();
        let mut ev = event(&u, vec![], Hlc::default(), msg(0));
        ev.id = EventId([3; 32]);
        assert!(matches!(Dag::new().insert(ev), Err(Error::IdMismatch)));
    }

    #[test]
    fn rejects_hlc_not_after_parent() {
        let u = User::new();
        let parent = event(&u, vec![], at(5), msg(0));
        let equal = event(&u, vec![parent.id], at(5), msg(1));
        let earlier = event(&u, vec![parent.id], at(4), msg(2));

        let mut dag = Dag::new();
        dag.insert(parent.clone()).unwrap();
        assert!(matches!(
            dag.insert(equal.clone()),
            Err(Error::HlcNotAfterParents)
        ));
        assert!(matches!(
            dag.insert(earlier),
            Err(Error::HlcNotAfterParents)
        ));
        assert_eq!(dag.len(), 1);
        assert!(dag.missing().is_empty());
    }

    #[test]
    fn pending_child_violating_hlc_is_dropped_with_descendants() {
        let u = User::new();
        let a = event(&u, vec![], at(5), msg(0));
        // Child stamped before its parent is valid only if the parent is not after it.
        let bad = event(&u, vec![a.id], at(3), msg(1));
        let grandchild = event(&u, vec![bad.id], at(9), msg(2));
        let unrelated = event(&u, vec![], at(1), msg(3));

        let mut dag = Dag::new();
        dag.insert(grandchild).unwrap();
        dag.insert(bad).unwrap();
        dag.insert(unrelated.clone()).unwrap();
        assert_eq!(dag.missing(), vec![a.id]);

        let outcome = dag.insert(a.clone()).unwrap();
        assert_eq!(
            outcome,
            InsertOutcome::Inserted {
                entered: vec![a.id]
            }
        );
        assert_eq!(dag.len(), 2, "a and unrelated only");
        assert!(
            dag.missing().is_empty(),
            "dropped children leave no waiters"
        );
        let mut expected = vec![a.id, unrelated.id];
        expected.sort();
        assert_eq!(dag.heads(), expected);
    }

    #[test]
    fn causal_order_is_insertion_independent() {
        let u = User::new();
        let mut evs = vec![event(&u, vec![], at(1), msg(0))];
        for i in 1..12u64 {
            // Several events share a parent. Stamps respect the HLC rule.
            let p = (i as usize * 7) % evs.len();
            let hlc = after(&[evs[p].body.hlc], 1 + i % 3);
            let parent = evs[p].id;
            evs.push(event(&u, vec![parent], hlc, msg(i)));
        }
        let order_of = |order: &[Event]| {
            let mut d = Dag::new();
            for e in order {
                d.insert(e.clone()).unwrap();
            }
            d.causal_order().iter().map(|e| e.id).collect::<Vec<_>>()
        };
        let forward = order_of(&evs);
        let mut rev = evs.clone();
        rev.reverse();
        assert_eq!(forward, order_of(&rev));
        let mut shuffled = evs.clone();
        shuffled.shuffle(&mut StdRng::seed_from_u64(7));
        assert_eq!(forward, order_of(&shuffled));
        assert_eq!(forward.len(), evs.len());
    }
}
