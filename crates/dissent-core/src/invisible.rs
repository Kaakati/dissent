//! Invisible mode: while this person's status is invisible, the device looks offline to others, as
//! far as the app can make it. See `docs/design/06-threat-model.md`, "Invisible mode".
//!
//! - **No beacons and no typing.** As before (`presence`, `typing`).
//! - **Receipts are held.** A message that arrives is stored and shown, but its delivery receipt waits
//!   in memory until the status is visible again, so the sender does not see it delivered at once. A
//!   restart drops the held receipts; the messages stay delivered on this device.
//! - **No profile sync** with a contact whose beacon arrives.
//! - **No address announcements.** A new address is signed and kept, but not announced, pushed or
//!   rebroadcast, and the user record is not republished. All of it goes out once visible again.
//! - **No answers.** A request from a device this one has not contacted lately gets no reply at all:
//!   the connection is closed, as when a device goes away. This device's own other devices are always
//!   answered, and so is a device it dialled itself within `CONTACTED_WITHIN`, which already knows it
//!   is online: a call it joined, a server it is syncing with, a message it sent.
//! - **Except hosted mail.** A device that holds offline mail for a person (`host_inbox_for`, or its own
//!   inbox switch) still answers deposits, collects and the other inbox requests for that person: hosting
//!   is consent. Anyone who writes to that person, or knows their root and this device's id, can therefore
//!   tell this device is up. See `inbox::hosted_inbox_request`.
//!
//! What still goes out: messages this person sends, and whatever this device asks of others. Those
//! show that it is online to whoever receives them, as they must.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use dissent_identity::{DeviceId, RootId};
use tracing::debug;

use crate::engine::Engine;
use crate::groups::ConvId;
use crate::presence::Status;

/// A device this one dialled within this long is answered while invisible.
pub(crate) const CONTACTED_WITHIN: Duration = Duration::from_secs(10 * 60);

/// A receipt held while invisible: the conversation, the message id, and the sender's device and person.
type HeldReceipt = (ConvId, [u8; 32], DeviceId, RootId);

/// Receipts held while invisible, oldest first.
#[derive(Default)]
pub(crate) struct HeldReceipts(Mutex<Vec<HeldReceipt>>);

impl Engine {
    /// Whether this person's status is invisible now.
    pub(crate) fn invisible(&self) -> bool {
        self.status().0 == Status::Invisible
    }

    /// Whether a request from `from` should go unanswered, as if this device were offline. Only while
    /// invisible, and never for this person's own devices or a device this one contacted lately.
    pub(crate) fn hidden_from(&self, from: &DeviceId) -> bool {
        if !self.invisible() {
            return false;
        }
        if self.read().directory.root_of(from) == Some(self.inner.me) {
            return false;
        }
        !self.node().contacted_within(*from, CONTACTED_WITHIN)
    }

    /// Holds a receipt while invisible. Returns false when it should go now.
    pub(crate) fn hold_receipt(&self, conv: ConvId, id: [u8; 32], device: DeviceId, root: RootId) -> bool {
        if !self.invisible() {
            return false;
        }
        self.inner
            .held_receipts
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((conv, id, device, root));
        true
    }

    /// Once visible again: sends the held receipts, and announces the address that was kept back.
    pub(crate) async fn come_back_visible(&self) {
        let held = std::mem::take(&mut *self.inner.held_receipts.0.lock().unwrap_or_else(PoisonError::into_inner));
        for (conv, id, device, root) in held {
            if let Err(e) = self.queue_receipt(conv, id, device, root) {
                debug!(conv = %conv, error = %e, "held receipt not queued");
            }
        }
        self.flush_conv_queues().await;
        let signed = self.read().my_signed.clone();
        if let Some(signed) = signed {
            self.announce(&signed).await;
        }
        self.refresh_own_record_now().await;
        self.spawn_publish_user_record();
    }

    /// For tests: how many receipts wait for this device to be visible again.
    #[cfg(test)]
    pub(crate) fn held_receipts_now(&self) -> usize {
        self.inner.held_receipts.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// Over loopback: what a device that never heard from us gets while we are invisible, and what waits.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::tests::{eventually, three_members, Node};
    use crate::rpc::Request;

    fn introduce(to: &Engine, of: &Engine) {
        let card = dissent_discovery::ContactCard::parse(&of.contact_link()).expect("card parses");
        let mut guard = to.write();
        guard.directory.learn(&card.cert);
        for peer in card.peers.iter().filter_map(|p| p.parse::<dissent_net::PeerAddr>().ok()) {
            to.learn_address(&mut guard, peer);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_invisible_device_does_not_answer_and_holds_its_receipts() {
        crate::testutil::bounded(async {
            let (alice, bob, _carol, id, _invite) = three_members().await;
            let alice_dev = alice.engine.inner.identity.device_id();
            let bob_dev = bob.engine.inner.identity.device_id();
            // Dave knows alice's device; alice never dialled dave.
            let dave = Node::named("dave").await;
            introduce(&dave.engine, &alice.engine);
            introduce(&alice.engine, &dave.engine);

            // Visible, alice answers anyone, if only with a refusal.
            let reply = dave.engine.call(alice_dev, &Request::HistorySupport).await;
            assert!(reply.is_ok(), "a visible device answers: {reply:?}");

            alice.engine.set_status(Status::Invisible, None).expect("invisible");
            // Invisible, a device alice did not contact gets no reply at all, like an offline device.
            let reply = dave.engine.call(alice_dev, &Request::HistorySupport).await;
            assert!(reply.is_err(), "no answer while invisible: {reply:?}");
            let member = bob.engine.call(alice_dev, &Request::Genesis { community: id }).await;
            let dialled = alice.engine.node().contacted_within(bob_dev, CONTACTED_WITHIN);
            assert_eq!(member.is_ok(), dialled, "only a device alice dialled lately is answered: {member:?}");
            // Once alice asks dave something, dave knows she is online, and is answered.
            let _ = alice.engine.call(dave.engine.inner.identity.device_id(), &Request::HistorySupport).await;
            let reply = dave.engine.call(alice_dev, &Request::HistorySupport).await;
            assert!(reply.is_ok(), "a device alice contacted is answered: {reply:?}");

            // A receipt waits while invisible, and goes into the queue once visible again.
            let conv = ConvId([7; 32]);
            alice.engine.queue_receipt(conv, [1; 32], bob_dev, bob.root()).expect("receipt");
            assert_eq!(alice.engine.held_receipts_now(), 1, "the receipt is held");
            alice.engine.set_status(Status::Online, None).expect("visible");
            eventually("the held receipt goes out", || alice.engine.held_receipts_now() == 0).await;
            let reply = dave.engine.call(alice_dev, &Request::HistorySupport).await;
            assert!(reply.is_ok(), "visible again, alice answers: {reply:?}");
        })
        .await;
    }
}
