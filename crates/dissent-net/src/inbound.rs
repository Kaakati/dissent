//! What a device that dials this node can make it hold. Anyone who knows a device id can dial it, and ids are
//! published, so the request and ping handlers admit each connection through [`Inbound`]: a few at once per device,
//! a bounded number in all, each under a deadline. When every slot is taken, the oldest connection still sending its
//! request gives its slot up, so connections that send nothing cannot keep an honest request out.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use dissent_identity::DeviceId;
use tokio::sync::Notify;

/// The limits on inbound requests and pings. The defaults are chosen against the requester's own budget: a request
/// gives up after 15 s ([`crate::rpc::REQUEST_TIMEOUT`]), dial included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundLimits {
    /// Requests and pings served at once, from every device together. 64: a seeder answers the syncs of many members
    /// at once when they all come online, and a reply holds its slot until the requester closes. A connection holds at
    /// most the bytes its peer actually sent, so the slots cost memory only when someone uploads that much, and the
    /// deadlines below bound how long.
    pub max_concurrent: usize,
    /// Served at once for one device. 8: a device that joins or comes back online runs sync, history, key, address,
    /// presence and inbox requests to the same peer side by side, each from its own task.
    pub per_device: usize,
    /// Time for the stream and the request's length to arrive, and the least time any exchange gets. 20 s: a
    /// requester stops waiting after 15 s, and our clock starts after the handshake, so an honest exchange always
    /// ends inside it, over a relay too.
    pub deadline_floor: Duration,
    /// The slowest rate a declared payload is given extra time for, in bytes per second. 256 KiB/s adds 64 s for the
    /// largest frame. An honest requester must send far faster to finish within its own 15 s, so this never cuts one
    /// off; it bounds how long a peer that declares a large frame and sends it slowly can hold a slot.
    pub min_rate: u64,
    /// How long a reply waits for the requester to close the connection. 5 s: the requester closes as soon as it has
    /// read the reply, one round trip away.
    pub close_wait: Duration,
    /// When every slot is taken, a connection that has been sending its request for this long gives its slot to a new
    /// one. 2 s: an honest request of ordinary size arrives within a few round trips, even over a relay.
    pub evict_after: Duration,
}

impl Default for InboundLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 64,
            per_device: 8,
            deadline_floor: Duration::from_secs(20),
            min_rate: 256 * 1024,
            close_wait: Duration::from_secs(5),
            evict_after: Duration::from_secs(2),
        }
    }
}

impl InboundLimits {
    /// The whole budget of an exchange whose request declares `len` bytes, counted from when it was admitted.
    pub fn deadline(&self, len: usize) -> Duration {
        let extra = Duration::from_secs_f64(len as f64 / self.min_rate.max(1) as f64);
        self.deadline_floor + extra
    }
}

/// Close code for a connection refused because this node is busy with others.
pub(crate) const BUSY: u32 = 3;
/// Close code for a connection that ran past its deadline.
pub(crate) const TIMED_OUT: u32 = 4;

/// The slots of inbound connections. One is shared by the request and ping handlers.
#[derive(Debug)]
pub(crate) struct Inbound {
    limits: InboundLimits,
    state: Mutex<Slots>,
}

#[derive(Debug, Default)]
struct Slots {
    next: u64,
    live: HashMap<u64, Slot>,
}

#[derive(Debug)]
struct Slot {
    device: DeviceId,
    started: Instant,
    /// Still receiving its request. Only such a slot can be given up for a new connection.
    reading: bool,
    cancel: Arc<Notify>,
}

impl Inbound {
    pub(crate) fn new(limits: InboundLimits) -> Arc<Self> {
        Arc::new(Self { limits, state: Mutex::new(Slots::default()) })
    }

    pub(crate) fn limits(&self) -> &InboundLimits {
        &self.limits
    }

    /// A slot for a connection from `device`, or `None` when it already has its share or every slot is taken by a
    /// connection too young to give its slot up.
    pub(crate) fn admit(self: &Arc<Self>, device: DeviceId) -> Option<Permit> {
        let mut slots = self.lock();
        if slots.live.values().filter(|s| s.device == device).count() >= self.limits.per_device {
            return None;
        }
        if slots.live.len() >= self.limits.max_concurrent {
            let oldest = slots
                .live
                .iter()
                .filter(|(_, s)| s.reading && s.started.elapsed() >= self.limits.evict_after)
                .min_by_key(|(_, s)| s.started)
                .map(|(id, _)| *id)?;
            if let Some(evicted) = slots.live.remove(&oldest) {
                evicted.cancel.notify_one();
            }
        }
        let id = slots.next;
        slots.next += 1;
        let cancel = Arc::new(Notify::new());
        slots.live.insert(id, Slot { device, started: Instant::now(), reading: true, cancel: cancel.clone() });
        Some(Permit { inbound: self.clone(), id, cancel })
    }

    /// Connections holding a slot now.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.lock().live.len()
    }

    fn lock(&self) -> MutexGuard<'_, Slots> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One admitted connection. Dropping it frees the slot.
#[derive(Debug)]
pub(crate) struct Permit {
    inbound: Arc<Inbound>,
    id: u64,
    cancel: Arc<Notify>,
}

impl Permit {
    /// The request has arrived. From now on the slot is not given up for a newer connection.
    pub(crate) fn done_reading(&self) {
        if let Some(slot) = self.inbound.lock().live.get_mut(&self.id) {
            slot.reading = false;
        }
    }

    /// Resolves when the slot was given to a newer connection.
    pub(crate) async fn cancelled(&self) {
        self.cancel.notified().await;
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.inbound.lock().live.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn limits(max: usize, per_device: usize, evict_after: Duration) -> InboundLimits {
        InboundLimits { max_concurrent: max, per_device, evict_after, ..InboundLimits::default() }
    }

    #[test]
    fn the_default_deadline_covers_the_requesters_budget_and_grows_with_the_frame() {
        let l = InboundLimits::default();
        assert!(l.deadline(0) > crate::rpc::REQUEST_TIMEOUT);
        assert_eq!(l.deadline(16 * 1024 * 1024), Duration::from_secs(20 + 64));
    }

    #[test]
    fn one_device_gets_only_its_share() {
        let inbound = Inbound::new(limits(8, 2, Duration::from_secs(60)));
        let a = inbound.admit(device(1)).unwrap();
        let _b = inbound.admit(device(1)).unwrap();
        assert!(inbound.admit(device(1)).is_none());
        assert!(inbound.admit(device(2)).is_some(), "another device is not held back");
        drop(a);
        assert!(inbound.admit(device(1)).is_some(), "a freed slot can be used again");
    }

    #[test]
    fn a_full_node_refuses_while_no_slot_is_old_enough_to_give_up() {
        let inbound = Inbound::new(limits(2, 2, Duration::from_secs(60)));
        let _a = inbound.admit(device(1)).unwrap();
        let _b = inbound.admit(device(2)).unwrap();
        assert!(inbound.admit(device(3)).is_none());
        assert_eq!(inbound.live(), 2);
    }

    #[tokio::test]
    async fn the_oldest_slot_still_reading_is_given_up_and_a_replying_one_is_kept() {
        let inbound = Inbound::new(limits(2, 2, Duration::ZERO));
        let oldest = inbound.admit(device(1)).unwrap();
        let replying = inbound.admit(device(2)).unwrap();
        replying.done_reading();
        let new = inbound.admit(device(3)).expect("the oldest reader gives its slot up");
        tokio::time::timeout(Duration::from_secs(1), oldest.cancelled()).await.expect("the evicted slot is told");
        assert_eq!(inbound.live(), 2);
        drop(oldest);
        assert_eq!(inbound.live(), 2, "dropping an evicted permit frees nothing twice");
        new.done_reading();
        assert!(inbound.admit(device(4)).is_none(), "a slot that has its request is never given up");
    }
}
