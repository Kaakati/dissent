//! One community's replayed state: its DAG, the signed events behind it, and its gossip wiring.
//!
//! Replay is incremental. A [`Replayer`] follows the DAG, so a new event costs one step at the
//! end of the causal order instead of a full replay.

use crate::error::{CoreError, Result};
use crate::net::ReceiverTask;
use dissent_events::{
    open_event, CommunityState, Dag, EventId, Hlc, InsertOutcome, Payload, Rejection, Replayer,
    Role, SealKey, SignedEvent,
};
use dissent_identity::{CommunityId, Genesis, RootId};
use dissent_net::TopicSender;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Outcome of ingesting a batch of verified events.
pub(crate) struct Batch {
    /// Events stored by this batch, including waiting children it released.
    pub stored: Vec<SignedEvent>,
    /// True when an event from this batch still waits for parents we do not have.
    pub stalled: bool,
}

/// Events put into the DAG by one step. Replay has not seen them yet.
struct Step {
    /// Events stored by the step, in the order they entered.
    items: Vec<SignedEvent>,
    /// Ids the DAG reported as entered, for [`Replayer::on_insert`].
    entered: Vec<EventId>,
    stalled: bool,
}

pub(crate) struct Community {
    pub genesis: Genesis,
    /// Stored events in causal order.
    dag: Dag,
    replayer: Replayer,
    /// Stored events that no stored event cites. These are the parents of new local events.
    heads: BTreeSet<EventId>,
    /// Highest HLC among stored events. Local events are stamped above it.
    max_hlc: Option<Hlc>,
    /// Every stored event with its cert. Feeds sync replies and the log.
    signed: BTreeMap<EventId, SignedEvent>,
    /// Events whose parents are missing. Held here so they can be persisted once released.
    waiting: BTreeMap<EventId, SignedEvent>,
    /// Accepted events whose side effects have already been handed out, so each runs once.
    handled: BTreeSet<EventId>,
    /// Set when the keys change. Any accepted event may then have unlocked, so the next
    /// [`Community::take_unhandled`] considers every stored event.
    ring_dirty: bool,
    /// HLC of each user's latest accepted Join. Picks the freshest display name.
    pub join_hlc: BTreeMap<RootId, Hlc>,
    /// Decrypted text by content key: a message or reply id, or the target of an edit. The newest
    /// hlc wins. Local to this device, so it is not part of `CommunityState`.
    pub content: BTreeMap<EventId, (Hlc, String)>,
    pub sender: Option<Arc<TopicSender>>,
    pub task: Option<ReceiverTask>,
    /// Control keys of the MLS epochs this device has been in. See `epoch_keys`.
    pub epochs: crate::epoch_keys::EpochStore,
}

impl Community {
    pub fn new(genesis: Genesis) -> Self {
        let replayer = Replayer::new(genesis.owner);
        Self {
            genesis,
            dag: Dag::new(),
            replayer,
            heads: BTreeSet::new(),
            max_hlc: None,
            signed: BTreeMap::new(),
            waiting: BTreeMap::new(),
            handled: BTreeSet::new(),
            ring_dirty: false,
            join_hlc: BTreeMap::new(),
            content: BTreeMap::new(),
            sender: None,
            task: None,
            epochs: Default::default(),
        }
    }

    /// Rebuilds a community from events read off disk. The caller has verified each event.
    pub fn load(genesis: Genesis, items: Vec<SignedEvent>) -> Self {
        let mut c = Self::new(genesis);
        let step = c.store(items);
        c.replayer.apply_all(&c.dag);
        c.after_change(&step.entered, &[]);
        c
    }

    pub fn id(&self) -> CommunityId {
        self.genesis.community
    }

    pub fn name(&self) -> &str {
        &self.genesis.name
    }

    /// The canonical owner. After a key rotation this is the new root, not the genesis owner.
    pub fn owner(&self) -> RootId {
        self.state().owner
    }

    /// The replayed state of the community.
    pub fn state(&self) -> &CommunityState {
        self.replayer.state()
    }

    /// The keys this community's sealed control events open with.
    pub fn ring(&self) -> &[SealKey] {
        self.replayer.ring()
    }

    /// Installs the keys that open sealed control events, and rebuilds the replay if they changed.
    pub fn set_ring(&mut self, ring: Vec<SealKey>) {
        self.replayer.set_ring(&self.dag, ring);
        self.join_times_refresh();
        self.ring_dirty = true;
    }

    /// Installs the encryption keys of the MLS epochs, and rebuilds the replay if they changed.
    pub fn set_epoch_ring(&mut self, epochs: Vec<dissent_events::EpochKey>) {
        self.replayer.set_epoch_ring(&self.dag, epochs);
        self.join_times_refresh();
        self.ring_dirty = true;
    }

    /// Accepted events whose side effects have not run yet, in causal order, and marks them as
    /// handled. `stored` are the events the last step stored. After a key change every stored
    /// event is considered, since any of them may have unlocked.
    pub fn take_unhandled(&mut self, stored: &[SignedEvent]) -> Vec<SignedEvent> {
        let rejected: BTreeSet<EventId> = self.rejected_ids().into_iter().collect();
        let candidates: Vec<EventId> = if std::mem::take(&mut self.ring_dirty) {
            self.dag.causal_order().iter().map(|ev| ev.id).collect()
        } else {
            stored.iter().map(|se| se.event.id).collect()
        };
        let fresh: Vec<EventId> = candidates
            .into_iter()
            .filter(|id| !rejected.contains(id) && self.handled.insert(*id))
            .collect();
        self.signed_for(&fresh)
    }

    pub fn is_member(&self, user: &RootId) -> bool {
        self.state().members.get(user).is_some_and(|m| !m.left)
    }

    pub fn is_rejected(&self, id: &EventId) -> bool {
        self.state().rejected.iter().any(|(rejected, _)| rejected == id)
    }

    /// Hashes of the stored events no other stored event cites, sorted.
    pub fn heads(&self) -> Vec<EventId> {
        self.heads.iter().copied().collect()
    }

    /// HLC of the newest stored event.
    pub fn max_hlc(&self) -> Option<Hlc> {
        self.max_hlc
    }

    /// `(wall time, id)` for every stored event. The sync index is built from these.
    pub fn index_items(&self) -> Vec<(u64, EventId)> {
        self.signed
            .values()
            .map(|se| (se.event.body.hlc.wall_ms, se.event.id))
            .collect()
    }

    /// Accepted secret rotations, oldest generation first: the generation and the sealed secret.
    pub fn rotations_in_order(&self) -> Vec<(u64, [u8; 32], Vec<u8>)> {
        let mut out: Vec<(u64, [u8; 32], Vec<u8>)> = self
            .signed
            .values()
            .filter(|se| !self.is_rejected(&se.event.id))
            .filter_map(|se| match &se.event.body.payload {
                Payload::SecretRotation {
                    generation, sealed, ..
                } => Some((*generation, se.event.id.0, sealed.clone())),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(generation, _, _)| *generation);
        out
    }

    /// Stored sealed events, in causal order.
    pub fn sealed_in_order(&self) -> Vec<SignedEvent> {
        let mut out: Vec<SignedEvent> = self
            .signed
            .values()
            .filter(|se| matches!(se.event.body.payload, Payload::Sealed { .. }))
            .cloned()
            .collect();
        out.sort_by_key(|se| (se.event.body.hlc, se.event.id));
        out
    }

    /// The stored events with these ids, in causal order. Unknown ids are skipped.
    pub fn signed_for(&self, ids: &[EventId]) -> Vec<SignedEvent> {
        let mut out: Vec<SignedEvent> = ids
            .iter()
            .filter_map(|id| self.signed.get(id).cloned())
            .collect();
        out.sort_by_key(|se| (se.event.body.hlc, se.event.id));
        out
    }

    /// `owner`, `admin`, `moderator`, or `member`.
    pub fn role_of(&self, user: &RootId) -> &'static str {
        if *user == self.owner() {
            return "owner";
        }
        match self.state().roles.get(user) {
            Some(roles) if roles.contains(&Role::Admin) => "admin",
            Some(roles) if roles.contains(&Role::Moderator) => "moderator",
            _ => "member",
        }
    }

    pub fn handle_of(&self, user: &RootId) -> Option<String> {
        self.state()
            .handles
            .iter()
            .find(|(_, holder)| *holder == user)
            .map(|(handle, _)| handle.clone())
    }

    /// Stores events that already passed `SignedEvent::verify`, then folds them into the state.
    ///
    /// An event whose parents are missing waits. It is stored when the parents arrive.
    pub fn ingest(&mut self, items: Vec<SignedEvent>) -> Batch {
        let rejected_before = self.rejected_ids();
        let step = self.store(items);
        if !step.entered.is_empty() {
            self.replayer.on_insert(&self.dag, &step.entered);
            self.after_change(&step.entered, &rejected_before);
        }
        Batch {
            stored: step.items,
            stalled: step.stalled,
        }
    }

    /// Adds a locally authored event, but only if replay accepts it.
    ///
    /// The caller stamps the event above [`Self::max_hlc`]. The new event then sorts after every
    /// applied event, which is the case where [`Replayer::would_reject`] is exact.
    /// Returns every event the step stored, the new one and any waiting children it released.
    pub fn author(&mut self, se: SignedEvent) -> Result<Vec<SignedEvent>> {
        let id = se.event.id;
        if self.max_hlc.is_some_and(|max| se.event.body.hlc <= max) {
            return Err(CoreError::Invalid(
                "a local event must sort after every stored event".into(),
            ));
        }
        if let Some(why) = self.replayer.would_reject(&se.event) {
            return Err(CoreError::Rejected(format!("{why:?}")));
        }
        let rejected_before = self.rejected_ids();
        let step = self.store(vec![se]);
        if !step.items.iter().any(|stored| stored.event.id == id) {
            return Err(CoreError::Invalid("parents of the new event are missing".into()));
        }
        self.replayer.on_insert(&self.dag, &step.entered);
        self.after_change(&step.entered, &rejected_before);
        // Local events are handled by the caller that authored them, not by the inbound path.
        self.handled.extend(step.items.iter().map(|se| se.event.id));
        Ok(step.items)
    }

    /// Puts events into the DAG and the signed set. Replay is left to the caller.
    fn store(&mut self, items: Vec<SignedEvent>) -> Step {
        let mut step = Step {
            items: Vec::new(),
            entered: Vec::new(),
            stalled: false,
        };
        let mut seen = Vec::new();
        for se in items {
            let id = se.event.id;
            if self.signed.contains_key(&id) || self.waiting.contains_key(&id) {
                continue;
            }
            seen.push(id);
            match self.dag.insert(se.event.clone()) {
                Ok(InsertOutcome::Inserted { entered }) => {
                    for eid in entered {
                        let item = if eid == id {
                            Some(se.clone())
                        } else {
                            self.waiting.remove(&eid)
                        };
                        if let Some(item) = item {
                            self.signed.insert(eid, item.clone());
                            step.items.push(item);
                        }
                        step.entered.push(eid);
                    }
                }
                Ok(InsertOutcome::Pending { .. }) => {
                    self.waiting.insert(id, se);
                }
                Ok(InsertOutcome::Duplicate) | Err(_) => {}
            }
        }
        step.stalled = seen.iter().any(|id| self.waiting.contains_key(id));
        step
    }

    /// Bookkeeping after events entered the DAG: heads, the newest HLC, and join times.
    fn after_change(&mut self, entered: &[EventId], rejected_before: &[EventId]) {
        self.heads.extend(entered.iter().copied());
        let mut joined = false;
        for id in entered {
            let Some(ev) = self.dag.get(id) else {
                continue;
            };
            for parent in &ev.body.parents {
                self.heads.remove(parent);
            }
            if self.max_hlc.is_none_or(|max| ev.body.hlc > max) {
                self.max_hlc = Some(ev.body.hlc);
            }
            joined |= open_event(ev, self.replayer.ring())
                .is_ok_and(|plain| matches!(plain.body.payload, Payload::Join { .. } | Payload::JoinVia { .. }));
        }
        if joined || self.rejected_ids() != rejected_before {
            self.join_times_refresh();
        }
    }

    fn join_times_refresh(&mut self) {
        self.join_hlc = join_times(&self.dag, self.replayer.ring(), &self.state().rejected);
    }

    fn rejected_ids(&self) -> Vec<EventId> {
        self.state().rejected.iter().map(|(id, _)| *id).collect()
    }
}

/// HLC of the latest accepted Join per author.
fn join_times(dag: &Dag, ring: &[SealKey], rejected: &[(EventId, Rejection)]) -> BTreeMap<RootId, Hlc> {
    let bad: BTreeSet<EventId> = rejected.iter().map(|(id, _)| *id).collect();
    let mut out: BTreeMap<RootId, Hlc> = BTreeMap::new();
    for stored in dag.causal_order() {
        let Ok(ev) = open_event(stored, ring) else {
            continue;
        };
        if !matches!(ev.body.payload, Payload::Join { .. } | Payload::JoinVia { .. }) || bad.contains(&ev.id) {
            continue;
        }
        let latest = out.entry(ev.body.author).or_insert(ev.body.hlc);
        if ev.body.hlc > *latest {
            *latest = ev.body.hlc;
        }
    }
    out
}

#[cfg(test)]
mod tests;
