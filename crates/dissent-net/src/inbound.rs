//! What a device that dials this node can make it hold. Anyone who knows a device id can dial it, and ids are
//! published, so the request and ping handlers admit each connection through [`Inbound`]: a few at once per device,
//! a bounded number in all, each under a deadline. When every slot is taken, the connection that is sending its request
//! the slowest gives its slot up, so connections that send little or nothing cannot keep an honest request out, and a
//! large honest request, which arrives fast but takes long, is the last to go. A connection that finds no slot to take
//! waits its turn in a short queue, first come first served, so peers that dial again and again cannot take every slot
//! that frees up before an honest one gets its turn.

use std::{
    collections::{HashMap, VecDeque},
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
    /// When every slot is taken, only a connection that has been sending its request for this long can give its slot
    /// to a new one, and of those the one that sent the fewest bytes per second goes. 2 s: an honest request of ordinary
    /// size arrives within a few round trips, even over a relay, and a younger rate says little.
    pub evict_after: Duration,
    /// How long a connection that finds every slot taken waits in line for one before it is turned away. 2 s: enough
    /// for the line ahead of it to be served, well within the requester's 15 s, and the requester asks again anyway.
    pub admit_wait: Duration,
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
            admit_wait: Duration::from_secs(2),
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
    /// Connections waiting for a slot, oldest first. Only the first may take one.
    line: VecDeque<u64>,
}

/// How often a connection in line looks for a slot.
const LINE_POLL: Duration = Duration::from_millis(20);

#[derive(Debug)]
struct Slot {
    device: DeviceId,
    started: Instant,
    /// Still receiving its request. Only such a slot can be given up for a new connection.
    reading: bool,
    /// Request bytes received so far.
    received: u64,
    cancel: Arc<Notify>,
}

impl Inbound {
    pub(crate) fn new(limits: InboundLimits) -> Arc<Self> {
        Arc::new(Self { limits, state: Mutex::new(Slots::default()) })
    }

    pub(crate) fn limits(&self) -> &InboundLimits {
        &self.limits
    }

    /// A slot for a connection from `device`, or `None` when it already has its share, or when it waited its turn in
    /// line for [`InboundLimits::admit_wait`] and still found every slot taken by a connection too young to give it up.
    pub(crate) async fn admit(self: &Arc<Self>, device: DeviceId) -> Option<Permit> {
        let ticket = {
            let mut slots = self.lock();
            if self.over_share(&slots, device) {
                return None;
            }
            if slots.line.is_empty() {
                if let Some(permit) = self.take(&mut slots, device) {
                    return Some(permit);
                }
            }
            if slots.line.len() >= self.limits.max_concurrent {
                return None;
            }
            let ticket = slots.next;
            slots.next += 1;
            slots.line.push_back(ticket);
            ticket
        };
        let _in_line = InLine { inbound: self, ticket };
        let deadline = tokio::time::Instant::now() + self.limits.admit_wait;
        loop {
            tokio::time::sleep(LINE_POLL).await;
            let mut slots = self.lock();
            if slots.line.front() == Some(&ticket) {
                if self.over_share(&slots, device) {
                    return None;
                }
                if let Some(permit) = self.take(&mut slots, device) {
                    return Some(permit);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
        }
    }

    fn over_share(&self, slots: &Slots, device: DeviceId) -> bool {
        slots.live.values().filter(|s| s.device == device).count() >= self.limits.per_device
    }

    /// Takes a free slot, or the slot of the slowest sender that has had long enough to show its rate.
    fn take(self: &Arc<Self>, slots: &mut Slots, device: DeviceId) -> Option<Permit> {
        if slots.live.len() >= self.limits.max_concurrent {
            let slowest = slots
                .live
                .iter()
                .filter(|(_, s)| s.reading && s.started.elapsed() >= self.limits.evict_after)
                .map(|(id, s)| (*id, s.received as f64 / s.started.elapsed().as_secs_f64().max(f64::MIN_POSITIVE)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id)?;
            if let Some(evicted) = slots.live.remove(&slowest) {
                evicted.cancel.notify_one();
            }
        }
        let id = slots.next;
        slots.next += 1;
        let cancel = Arc::new(Notify::new());
        slots.live.insert(id, Slot { device, started: Instant::now(), reading: true, received: 0, cancel: cancel.clone() });
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

/// A place in line. Dropping it leaves the line.
struct InLine<'a> {
    inbound: &'a Inbound,
    ticket: u64,
}

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        self.inbound.lock().line.retain(|t| *t != self.ticket);
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

    /// Counts `bytes` more of the request as received.
    pub(crate) fn received(&self, bytes: usize) {
        if let Some(slot) = self.inbound.lock().live.get_mut(&self.id) {
            slot.received += bytes as u64;
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
        InboundLimits { max_concurrent: max, per_device, evict_after, admit_wait: Duration::from_millis(100), ..InboundLimits::default() }
    }

    #[test]
    fn the_default_deadline_covers_the_requesters_budget_and_grows_with_the_frame() {
        let l = InboundLimits::default();
        assert!(l.deadline(0) > crate::rpc::REQUEST_TIMEOUT);
        assert_eq!(l.deadline(16 * 1024 * 1024), Duration::from_secs(20 + 64));
    }

    #[tokio::test]
    async fn one_device_gets_only_its_share() {
        let inbound = Inbound::new(limits(8, 2, Duration::from_secs(60)));
        let a = inbound.admit(device(1)).await.unwrap();
        let _b = inbound.admit(device(1)).await.unwrap();
        assert!(inbound.admit(device(1)).await.is_none());
        assert!(inbound.admit(device(2)).await.is_some(), "another device is not held back");
        drop(a);
        assert!(inbound.admit(device(1)).await.is_some(), "a freed slot can be used again");
    }

    #[tokio::test]
    async fn a_full_node_refuses_while_no_slot_is_old_enough_to_give_up() {
        let inbound = Inbound::new(limits(2, 2, Duration::from_secs(60)));
        let _a = inbound.admit(device(1)).await.unwrap();
        let _b = inbound.admit(device(2)).await.unwrap();
        assert!(inbound.admit(device(3)).await.is_none());
        assert_eq!(inbound.live(), 2);
    }

    #[tokio::test]
    async fn a_connection_in_line_gets_the_next_slot_before_a_later_one() {
        let inbound = Inbound::new(InboundLimits { admit_wait: Duration::from_secs(2), ..limits(2, 4, Duration::from_secs(60)) });
        let (held_a, held_b) = (inbound.admit(device(1)).await.unwrap(), inbound.admit(device(1)).await.unwrap());
        let order = Arc::new(Mutex::new(Vec::new()));
        let wait = |n: u8| {
            let (inbound, order) = (inbound.clone(), order.clone());
            tokio::spawn(async move {
                let permit = inbound.admit(device(n)).await;
                order.lock().unwrap().push(n);
                permit
            })
        };
        let first = wait(2);
        tokio::time::sleep(Duration::from_millis(60)).await;
        let later = wait(3);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(inbound.admit(device(4)).await.is_none(), "the line is full, so a third is turned away");
        drop(held_a);
        let first = first.await.unwrap().expect("the first in line gets the freed slot");
        assert_eq!(*order.lock().unwrap(), vec![2], "the later one is still waiting");
        drop(held_b);
        assert!(later.await.unwrap().is_some(), "and the next slot goes to the next in line");
        drop(first);
    }

    #[tokio::test]
    async fn the_slowest_sender_is_given_up_not_the_oldest() {
        let inbound = Inbound::new(limits(2, 2, Duration::ZERO));
        let fast_and_old = inbound.admit(device(1)).await.unwrap();
        let slow = inbound.admit(device(2)).await.unwrap();
        fast_and_old.received(8 * 1024 * 1024);
        slow.received(10);
        let _new = inbound.admit(device(3)).await.expect("a slot is given up");
        tokio::time::timeout(Duration::from_secs(1), slow.cancelled()).await.expect("the slow sender is told");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), fast_and_old.cancelled()).await.is_err(),
            "the older, faster sender keeps its slot"
        );
    }

    #[tokio::test]
    async fn the_oldest_slot_still_reading_is_given_up_and_a_replying_one_is_kept() {
        let inbound = Inbound::new(limits(2, 2, Duration::ZERO));
        let oldest = inbound.admit(device(1)).await.unwrap();
        let replying = inbound.admit(device(2)).await.unwrap();
        replying.done_reading();
        let new = inbound.admit(device(3)).await.expect("the oldest reader gives its slot up");
        tokio::time::timeout(Duration::from_secs(1), oldest.cancelled()).await.expect("the evicted slot is told");
        assert_eq!(inbound.live(), 2);
        drop(oldest);
        assert_eq!(inbound.live(), 2, "dropping an evicted permit frees nothing twice");
        new.done_reading();
        assert!(inbound.admit(device(4)).await.is_none(), "a slot that has its request is never given up");
    }
}
