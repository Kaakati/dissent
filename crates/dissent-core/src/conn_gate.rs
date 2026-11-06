//! What the connection gate knows about other devices, as one snapshot. The node judges every incoming connection
//! from it, so the judgement never reads the world or waits on its lock. The snapshot is rebuilt whole and swapped in
//! when membership, bans, revocations, conversations or contacts change, and once a while in any case, so a change
//! that reaches the world by a path that emits no event is picked up too.

use std::{
    collections::HashSet,
    sync::{
        Arc, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use dissent_identity::DeviceId;
use dissent_net::{ConnectionGate, Gate};

use crate::{
    engine::{Engine, Inner},
    views::EngineEvent,
    world::World,
};

/// How often a dirty snapshot is rebuilt. Changes arrive in bursts (a join brings many events), so they are taken
/// together.
const REBUILD_EVERY: Duration = Duration::from_millis(250);
/// The longest a snapshot goes without a rebuild.
const REBUILD_AT_LEAST: Duration = Duration::from_secs(30);

/// The devices the gate tells apart.
#[derive(Debug, Default)]
pub(crate) struct GateSnapshot {
    revoked: HashSet<DeviceId>,
    refused: HashSet<DeviceId>,
    known: HashSet<DeviceId>,
}

impl GateSnapshot {
    pub(crate) fn verdict(&self, device: &DeviceId) -> Gate {
        if self.revoked.contains(device) || self.refused.contains(device) {
            Gate::Refuse
        } else if self.known.contains(device) {
            Gate::Known
        } else {
            Gate::Stranger
        }
    }
}

/// The current snapshot, and whether it needs a rebuild.
#[derive(Debug, Default)]
pub(crate) struct GateCell {
    snapshot: RwLock<Arc<GateSnapshot>>,
    dirty: AtomicBool,
}

impl GateCell {
    fn current(&self) -> Arc<GateSnapshot> {
        self.snapshot.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn replace(&self, snapshot: GateSnapshot) {
        *self.snapshot.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(snapshot);
    }
}

impl ConnectionGate for GateCell {
    fn verdict(&self, device: DeviceId) -> Gate {
        self.current().verdict(&device)
    }
}

impl World {
    /// The snapshot of who is who. Our own devices are known. A device is refused only when a server revoked it, or
    /// when its user is banned somewhere and shares nothing else with us; a device we know nothing about is a
    /// stranger, never refused.
    pub(crate) fn gate_snapshot(&self) -> GateSnapshot {
        let mut snapshot = GateSnapshot::default();
        let mut banned = HashSet::new();
        for c in self.communities.values() {
            let s = c.state();
            snapshot.revoked.extend(s.revoked_devices.keys().copied());
            banned.extend(s.banned.iter().copied());
        }
        let me = self.me;
        let devices = self
            .directory
            .device_roots()
            .chain(self.communities.values().flat_map(|c| c.state().device_roots.clone()))
            .collect::<Vec<_>>();
        for (device, _) in devices {
            let Some(root) = self.root_of_device(&device) else { continue };
            if root == me || self.shares_with(&root) {
                snapshot.known.insert(device);
            } else if banned.contains(&root) {
                snapshot.refused.insert(device);
            }
        }
        snapshot
    }
}

/// Builds the snapshot from the world and swaps it in.
pub(crate) fn rebuild(inner: &Arc<Inner>) {
    inner.conn_gate.dirty.store(false, Ordering::SeqCst);
    let snapshot = Engine { inner: inner.clone() }.read().gate_snapshot();
    inner.conn_gate.replace(snapshot);
}

/// Marks the snapshot for a rebuild when `event` can change who is known, refused or revoked.
pub(crate) fn note_event(inner: &Inner, event: &EngineEvent) {
    if matches!(
        event,
        EngineEvent::CommunitiesChanged
            | EngineEvent::CommunityUpdated { .. }
            | EngineEvent::ConversationChanged { .. }
            | EngineEvent::ContactsChanged
            | EngineEvent::IdentityChanged { .. }
    ) {
        inner.conn_gate.dirty.store(true, Ordering::SeqCst);
    }
}

/// Rebuilds the snapshot soon after a change, and now and then in any case.
pub(crate) async fn gate_loop(weak: Weak<Inner>) {
    let mut tick = tokio::time::interval(REBUILD_EVERY);
    let mut since = Duration::ZERO;
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else { break };
        since += REBUILD_EVERY;
        if inner.conn_gate.dirty.load(Ordering::SeqCst) || since >= REBUILD_AT_LEAST {
            since = Duration::ZERO;
            rebuild(&inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoked_or_refused_device_is_refused_a_known_one_known_and_the_rest_strangers() {
        let mut s = GateSnapshot::default();
        s.revoked.insert(DeviceId([1; 32]));
        s.refused.insert(DeviceId([2; 32]));
        s.known.insert(DeviceId([3; 32]));
        s.known.insert(DeviceId([1; 32]));
        assert_eq!(s.verdict(&DeviceId([1; 32])), Gate::Refuse, "a revocation wins over anything else");
        assert_eq!(s.verdict(&DeviceId([2; 32])), Gate::Refuse);
        assert_eq!(s.verdict(&DeviceId([3; 32])), Gate::Known);
        assert_eq!(s.verdict(&DeviceId([4; 32])), Gate::Stranger, "a device we know nothing about is not refused");
    }

    #[test]
    fn the_cell_answers_from_the_snapshot_swapped_in_last() {
        let cell = GateCell::default();
        assert_eq!(cell.verdict(DeviceId([3; 32])), Gate::Stranger);
        let mut s = GateSnapshot::default();
        s.known.insert(DeviceId([3; 32]));
        cell.replace(s);
        assert_eq!(cell.verdict(DeviceId([3; 32])), Gate::Known);
    }
}
