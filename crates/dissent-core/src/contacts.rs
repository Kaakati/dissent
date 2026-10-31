//! Contact requests and direct-message privacy.
//!
//! Adding a contact link no longer makes a contact. It sends a signed `ContactRequest` to the
//! person's devices. They become a contact once the recipient accepts, and the accept comes back
//! as a signed `Accept`. A declined sender hears nothing, and is remembered so it cannot spam.
//! Undelivered requests and accepts wait in `ContactBook::outbox` and are retried by maintenance.

use std::collections::{BTreeMap, BTreeSet};

use crate::presence::Presence;
use dissent_discovery::{ContactCard, ContactLink, UserRecord, MAX_DEVICES};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::communities::{public_addr, DHT_LOOKUP_WAIT};
use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{decode, encode, Request, Response};
use crate::store::Paths;
use crate::groups::dm_conv_id;
use crate::views::{EngineEvent, UserView};
use crate::world::World;

/// Longest note a request may carry, in Unicode scalar values.
pub const MAX_NOTE_CHARS: usize = 200;
/// Requests one sender may place in a rolling day. Further ones are refused.
pub const MAX_REQUESTS_PER_DAY: usize = 5;
/// Requests waiting for an answer. Beyond this, new senders are refused.
pub const MAX_PENDING_INCOMING: usize = 200;
/// Rate-limit log entries kept. The oldest go first.
const MAX_RECENT_ENTRIES: usize = 10_000;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const DOMAIN: &[u8] = b"dissent/contact-msg/v1";

/// Who may send a direct message. Contacts are always allowed, except under `ContactsOnly`
/// where they are too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Anyone who knows the device can send a direct message.
    Everyone,
    /// Contacts, and people who share a server with us. The default.
    #[default]
    SharedServers,
    /// Only accepted contacts.
    ContactsOnly,
}

/// A request to become contacts. `from_card` is the sender's card, so the recipient learns the
/// sender's cert, name and addresses from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRequest {
    pub from_card: ContactCard,
    pub note: String,
    pub ts: u64,
    /// The sender's inbox devices, which hold its messages while it is offline.
    pub inboxes: Vec<DeviceId>,
}

/// What travels between devices about contacts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactMsg {
    Request(ContactRequest),
    /// The recipient of a request accepted it. `from_card` is the accepter's card.
    Accept {
        from_card: ContactCard,
        ts: u64,
        inboxes: Vec<DeviceId>,
    },
    /// The sender changed its inbox devices. Sent to every contact.
    Inboxes {
        from_card: ContactCard,
        devices: Vec<DeviceId>,
        ts: u64,
    },
    /// The sending device started (`on`) or stopped holding the recipient's offline mail. Sent to the
    /// person it hosts, so they learn of it without looking. Appended; an older device refuses it, and the
    /// notice leaves the outbox. See `inbox::host_inbox_for`.
    InboxHost {
        from_card: ContactCard,
        on: bool,
        ts: u64,
    },
}

/// A contact message with the sender device's signature over it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedContact {
    pub msg: ContactMsg,
    pub sig: Signature,
}

fn signed_bytes(msg: &ContactMsg) -> Result<Vec<u8>> {
    let mut out = DOMAIN.to_vec();
    out.extend(encode(msg)?);
    Ok(out)
}

impl SignedContact {
    fn sign(msg: ContactMsg, device: &DeviceKey) -> Result<Self> {
        let sig = device.sign(&signed_bytes(&msg)?);
        Ok(Self { msg, sig })
    }

    /// The sender's card.
    pub fn card(&self) -> &ContactCard {
        match &self.msg {
            ContactMsg::Request(r) => &r.from_card,
            ContactMsg::Accept { from_card, .. }
            | ContactMsg::Inboxes { from_card, .. }
            | ContactMsg::InboxHost { from_card, .. } => from_card,
        }
    }

    /// Checks the sender's cert, the device signature, the card's fields and the note length.
    pub fn verify(&self) -> Result<()> {
        let card = self.card();
        card.cert
            .verify()
            .map_err(|e| CoreError::Invalid(format!("cert: {e}")))?;
        card.cert
            .device
            .verify(&signed_bytes(&self.msg)?, &self.sig)
            .map_err(|e| CoreError::Invalid(format!("signature: {e}")))?;
        let checked = ContactCard::new(card.cert.clone(), &card.display_name, card.peers.clone())?;
        if checked != *card {
            return Err(CoreError::Invalid("contact card fields are invalid".into()));
        }
        if let ContactMsg::Request(r) = &self.msg {
            if r.note.chars().count() > MAX_NOTE_CHARS {
                return Err(CoreError::Invalid("note is too long".into()));
            }
        }
        Ok(())
    }
}

/// A message we owe another person. `delivered` is set once their device acknowledged a request.
/// Accepts leave the outbox as soon as they are acknowledged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Outbound {
    pub to: RootId,
    pub msg: SignedContact,
    pub delivered: bool,
}

/// Contact state that survives restarts. Lives in `World`, beside the contact cards.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ContactBook {
    /// Requests from others, waiting for an answer.
    pub incoming: Vec<ContactRequest>,
    /// Requests we sent, and accepts we owe.
    pub outbox: Vec<Outbound>,
    /// Senders whose requests were declined. Their later requests are dropped without a reply.
    pub declined: BTreeSet<RootId>,
    /// When each incoming request arrived, for the per-sender daily limit.
    pub recent: Vec<(RootId, u64)>,
    pub policy: DmPolicy,
    /// Our own inbox devices.
    pub my_inboxes: Vec<DeviceId>,
    /// Inbox devices each contact has announced, for deposits.
    pub known_inboxes: BTreeMap<RootId, Vec<DeviceId>>,
    /// Users we block. Their requests and messages are dropped without a reply.
    pub blocked: BTreeSet<RootId>,
    /// When each sender was last sent an explicit refusal. One per sender per refusal window.
    pub last_refusal: BTreeMap<RootId, u64>,
}

/// Whether "Let people add me by address" is off. Missing or unreadable means on.
fn hide_address(paths: &Paths) -> bool {
    paths.read_bin::<bool>(&paths.address_privacy()).ok().flatten().unwrap_or_default()
}

impl ContactBook {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(paths.read_bin(&paths.contact_book())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.write_bin(&paths.contact_book(), self)
    }

    /// Replaces any request we already have queued for `to`.
    fn queue_request(&mut self, to: RootId, msg: SignedContact) {
        self.outbox
            .retain(|o| !(o.to == to && matches!(o.msg.msg, ContactMsg::Request(_))));
        self.outbox.push(Outbound {
            to,
            msg,
            delivered: false,
        });
    }

    fn sent_request_to(&self, to: &RootId) -> bool {
        self.outbox
            .iter()
            .any(|o| o.to == *to && matches!(o.msg.msg, ContactMsg::Request(_)))
    }

    fn drop_sent_request_to(&mut self, to: &RootId) {
        self.outbox
            .retain(|o| !(o.to == *to && matches!(o.msg.msg, ContactMsg::Request(_))));
    }

    /// True when `from` has used up its daily allowance.
    fn over_limit(&mut self, from: &RootId, now: u64) -> bool {
        self.recent.retain(|(_, t)| now.saturating_sub(*t) < DAY_MS);
        let used = self.recent.iter().filter(|(r, _)| r == from).count();
        used >= MAX_REQUESTS_PER_DAY
    }

    fn record(&mut self, from: RootId, now: u64) {
        self.recent.push((from, now));
        if self.recent.len() > MAX_RECENT_ENTRIES {
            let excess = self.recent.len() - MAX_RECENT_ENTRIES;
            self.recent.drain(..excess);
        }
    }
}

/// A contact request, as shown to the user. `delivered` is true for incoming requests, and for
/// sent ones once the other side's device acknowledged them.
#[derive(Clone, Debug, Serialize)]
pub struct ContactRequestView {
    pub user: String,
    pub suffix: String,
    pub display_name: String,
    pub note: String,
    pub ms: u64,
    pub delivered: bool,
}

/// Whether `peer` may send us a direct message under the current policy.
pub(crate) fn dm_allowed(w: &World, peer: &RootId) -> bool {
    if w.contact_book.blocked.contains(peer) {
        return false;
    }
    if w.is_contact(peer) {
        return true;
    }
    match w.contact_book.policy {
        DmPolicy::Everyone => true,
        DmPolicy::ContactsOnly => false,
        DmPolicy::SharedServers => w
            .communities
            .values()
            .any(|c| c.is_member(&w.me) && c.is_member(peer)),
    }
}

/// Adds or replaces a contact from a verified card.
fn befriend(w: &mut World, card: ContactCard) -> Result<()> {
    w.directory.learn(&card.cert);
    w.contact_times.became_contact(card.cert.root, now_ms());
    w.contact_times.save(&w.paths)?;
    w.contacts.retain(|c| c.cert.root != card.cert.root);
    w.contacts.push(card);
    w.save_contacts()
}

fn view(w: &World, user: &RootId, note: String, ms: u64, delivered: bool) -> ContactRequestView {
    ContactRequestView {
        user: user.to_string(),
        suffix: user.suffix(),
        display_name: w.display_name(user),
        note,
        ms,
        delivered,
    }
}

enum Attempt {
    Acked,
    Refused,
    Unreachable,
}

impl Engine {
    /// Asks `link`'s owner to become a contact, with a short note. The person becomes a contact
    /// once they accept. Until then the request is listed under `sent_contact_requests`.
    pub async fn request_contact(&self, link: &str, note: &str) -> Result<UserView> {
        let note = self.checked_note(note)?;
        match ContactLink::parse(link)? {
            ContactLink::Card(card) => {
                let user = card.cert.root;
                self.check_not_me(&user)?;
                self.send_request(user, std::slice::from_ref(&card.cert), &card.peers, note)
                    .await
            }
            ContactLink::Address(user) => self.lookup_and_request(user, note).await,
        }
    }

    /// Asks the owner of `address` to become a contact, without a link. Looks up the address's user
    /// record on the DHT and verifies its device certs against the address. Refused when the record is
    /// missing or empty, which is what an address that is not discoverable looks like.
    pub async fn add_contact_by_address(&self, address: &str, note: &str) -> Result<UserView> {
        let user = RootId::parse_address(address.trim())?;
        let note = self.checked_note(note)?;
        self.lookup_and_request(user, note).await
    }

    fn checked_note(&self, note: &str) -> Result<String> {
        let note = note.trim().to_string();
        if note.chars().count() > MAX_NOTE_CHARS {
            return Err(CoreError::Invalid(format!(
                "note is longer than {MAX_NOTE_CHARS} characters"
            )));
        }
        Ok(note)
    }

    fn check_not_me(&self, user: &RootId) -> Result<()> {
        if *user == self.inner.me {
            return Err(CoreError::Invalid("that is your own contact link or address".into()));
        }
        Ok(())
    }

    /// Resolves `user` from the DHT, then asks them. A lookup that finds nothing, or a record that
    /// does not verify, gives the same message: the person may be offline or not discoverable.
    async fn lookup_and_request(&self, user: RootId, note: String) -> Result<UserView> {
        self.check_not_me(&user)?;
        if self.read().is_contact(&user) {
            return Ok(self.read().user_view(&user));
        }
        let not_found = || {
            CoreError::Invalid(
                "They may be offline, or they don't allow adding by address. Ask for their contact link."
                    .into(),
            )
        };
        let Some(discovery) = self.inner.discovery.clone() else {
            return Err(CoreError::Invalid(
                "looking up an address needs the internet. Ask for their contact link.".into(),
            ));
        };
        let record: UserRecord = match tokio::time::timeout(DHT_LOOKUP_WAIT, discovery.resolve_user(&user)).await {
            Ok(Ok(Some(rec))) if !rec.is_empty() => rec,
            Ok(Ok(_)) | Err(_) => return Err(not_found()),
            Ok(Err(e)) => {
                warn!(error = %e, "user record does not verify");
                return Err(not_found());
            }
        };
        self.send_request(user, &record.devices, &record.peers, note).await
    }

    /// Learns the person's device certs and dial addresses, queues a request to them, and sends it now.
    async fn send_request(
        &self,
        user: RootId,
        certs: &[DeviceCert],
        peers: &[String],
        note: String,
    ) -> Result<UserView> {
        {
            let mut guard = self.write();
            let w = &mut *guard;
            for cert in certs {
                w.directory.learn(cert);
            }
            for peer in peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()) {
                self.learn_address(w, peer);
            }
            if !w.is_contact(&user) {
                let mine = self.my_card(w)?;
                let msg = ContactMsg::Request(ContactRequest {
                    from_card: mine,
                    note,
                    ts: now_ms(),
                    inboxes: w.contact_book.my_inboxes.clone(),
                });
                let signed = SignedContact::sign(msg, &self.inner.identity.device)?;
                w.contact_book.queue_request(user, signed);
                w.contact_book.save(&w.paths)?;
            }
        }
        self.retry_contact_outbox().await;
        Ok(self.read().user_view(&user))
    }

    /// Asks for a contact without a note. See `request_contact`.
    pub async fn add_contact(&self, link: &str) -> Result<UserView> {
        self.request_contact(link, "").await
    }

    /// Requests from other people that are waiting for an answer, oldest first.
    pub fn contact_requests(&self) -> Vec<ContactRequestView> {
        let w = self.read();
        let mut out: Vec<ContactRequestView> = w
            .contact_book
            .incoming
            .iter()
            .map(|r| view(&w, &r.from_card.cert.root, r.note.clone(), r.ts, true))
            .collect();
        out.sort_by_key(|v| v.ms);
        out
    }

    /// Requests we sent that the other person has not accepted yet, oldest first.
    pub fn sent_contact_requests(&self) -> Vec<ContactRequestView> {
        let w = self.read();
        let mut out: Vec<ContactRequestView> = w
            .contact_book
            .outbox
            .iter()
            .filter_map(|o| match &o.msg.msg {
                ContactMsg::Request(r) => Some(view(&w, &o.to, r.note.clone(), r.ts, o.delivered)),
                ContactMsg::Accept { .. } | ContactMsg::Inboxes { .. } | ContactMsg::InboxHost { .. } => None,
            })
            .collect();
        out.sort_by_key(|v| v.ms);
        out
    }

    /// Accepts a pending request. The sender becomes a contact, and is told so.
    pub async fn accept_request(&self, user: RootId) -> Result<UserView> {
        {
            let mut guard = self.write();
            let w = &mut *guard;
            let Some(pos) = w
                .contact_book
                .incoming
                .iter()
                .position(|r| r.from_card.cert.root == user)
            else {
                return Err(CoreError::NotFound(format!("contact request from {user}")));
            };
            let request = w.contact_book.incoming.remove(pos);
            befriend(w, request.from_card)?;
            self.queue_accept(w, user)?;
            w.contact_book.save(&w.paths)?;
        }
        self.emit(EngineEvent::ContactsChanged);
        self.retry_contact_outbox().await;
        // The new contact can see our status now, so it need not wait for the next beacon.
        self.send_presence().await;
        Ok(self.read().user_view(&user))
    }

    /// Declines a pending request. The sender gets no reply and cannot send another.
    pub fn decline_request(&self, user: RootId) -> Result<()> {
        {
            let mut guard = self.write();
            let w = &mut *guard;
            let before = w.contact_book.incoming.len();
            w.contact_book
                .incoming
                .retain(|r| r.from_card.cert.root != user);
            if w.contact_book.incoming.len() == before {
                return Err(CoreError::NotFound(format!("contact request from {user}")));
            }
            w.contact_book.declined.insert(user);
            w.contact_book.save(&w.paths)?;
        }
        self.emit(EngineEvent::ContactsChanged);
        Ok(())
    }

    /// Who may send us direct messages.
    pub fn dm_policy(&self) -> DmPolicy {
        self.read().contact_book.policy
    }

    /// Changes who may send us direct messages. Takes effect for the next message.
    pub fn set_dm_policy(&self, policy: DmPolicy) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        w.contact_book.policy = policy;
        w.contact_book.save(&w.paths)
    }

    /// Whether people may add us by address. On unless turned off.
    pub fn address_discovery(&self) -> bool {
        !hide_address(&self.read().paths)
    }

    /// Turns "Let people add me by address" on or off. Off withdraws our user record from the DHT.
    /// Contact links keep working either way.
    pub fn set_address_discovery(&self, on: bool) -> Result<()> {
        {
            let guard = self.write();
            guard.paths.write_bin(&guard.paths.address_privacy(), &!on)?;
        }
        self.spawn_publish_user_record();
        Ok(())
    }

    /// Publishes the user record in the background. See [`Engine::publish_user_record`].
    pub(crate) fn spawn_publish_user_record(&self) {
        let engine = self.clone();
        self.spawn_task(async move { engine.publish_user_record().await });
    }

    /// Publishes this address's user record: our device certs and a dial address, or an empty
    /// record while address discovery is off. Only a device that holds the root key can publish,
    /// so a linked device without it does nothing here; the root device lists its certs.
    pub(crate) async fn publish_user_record(&self) {
        let (Some(discovery), Some(root)) = (self.inner.discovery.clone(), self.inner.identity.root.clone()) else {
            return;
        };
        let listed = self.load_devices().unwrap_or_default();
        let _publishing = self.inner.publish_lock.lock().await;
        let rec = {
            let w = self.read();
            if hide_address(&w.paths) {
                UserRecord::default()
            } else {
                let mut devices = vec![self.inner.identity.cert.clone()];
                devices.extend(listed.into_iter().map(|d| d.cert));
                let mut seen = BTreeSet::new();
                devices.retain(|c| seen.insert(c.device));
                devices.truncate(MAX_DEVICES);
                UserRecord {
                    devices,
                    peers: w.my_addr.iter().map(|a| public_addr(a).to_string()).collect(),
                }
            }
        };
        if let Err(e) = discovery.publish_user(&root, &rec).await {
            warn!(error = %e, "could not publish the user record");
        }
    }

    /// Our own card, built the same way as `contact_link`.
    fn my_card(&self, w: &World) -> Result<ContactCard> {
        let peers: Vec<String> = w.my_addr.iter().map(|a| self.shareable(a).to_string()).collect();
        let cert = self.inner.identity.cert.clone();
        ContactCard::new(cert.clone(), &w.my_name(), peers.clone())
            .or_else(|_| ContactCard::new(cert, &w.anon_name(), peers))
            .map_err(CoreError::from)
    }

    fn queue_accept(&self, w: &mut World, to: RootId) -> Result<()> {
        let from_card = self.my_card(w)?;
        let msg = ContactMsg::Accept {
            from_card,
            ts: now_ms(),
            inboxes: w.contact_book.my_inboxes.clone(),
        };
        let signed = SignedContact::sign(msg, &self.inner.identity.device)?;
        w.contact_book.outbox.push(Outbound {
            to,
            msg: signed,
            delivered: false,
        });
        Ok(())
    }

    /// Sends one message to every known device of `to`. An ack from any device counts.
    async fn send_contact(&self, to: RootId, msg: &SignedContact) -> Attempt {
        let devices = self.read().directory.devices_of(&to);
        let Ok(bytes) = self.request_bytes(&Request::Contact(Box::new(msg.clone()))) else {
            return Attempt::Refused;
        };
        // Every device of the person gets it, so each one can answer. One acknowledgement is enough.
        let node = self.node();
        let mut acked = false;
        let mut refused = false;
        for device in devices {
            match node.request(device, bytes.clone()).await {
                Ok(reply) => match decode::<Response>(&reply) {
                    Ok(Response::Ack) => acked = true,
                    _ => refused = true,
                },
                Err(e) => debug!(device = %device, error = %e, "contact message not delivered"),
            }
        }
        if acked {
            Attempt::Acked
        } else if refused {
            Attempt::Refused
        } else {
            Attempt::Unreachable
        }
    }

    /// Retries every undelivered request and accept. A message the other side refused leaves
    /// the queue, so it is not retried forever. Called on each maintenance pass and after changes.
    pub(crate) async fn retry_contact_outbox(&self) {
        let pending: Vec<(RootId, SignedContact)> = {
            let w = self.read();
            w.contact_book
                .outbox
                .iter()
                .filter(|o| !o.delivered)
                .map(|o| (o.to, o.msg.clone()))
                .collect()
        };
        for (to, msg) in pending {
            let attempt = self.send_contact(to, &msg).await;
            if matches!(attempt, Attempt::Unreachable) {
                continue;
            }
            let is_accept = !matches!(msg.msg, ContactMsg::Request(_));
            let mut guard = self.write();
            let w = &mut *guard;
            let book = &mut w.contact_book;
            if is_accept || matches!(attempt, Attempt::Refused) {
                book.outbox.retain(|o| !(o.to == to && o.msg == msg));
            } else if let Some(o) = book.outbox.iter_mut().find(|o| o.to == to && o.msg == msg) {
                o.delivered = true;
            }
            if let Err(e) = book.save(&w.paths) {
                warn!(error = %e, "could not save contact outbox");
            }
        }
    }

    /// Handles a contact message from another device. Replies `Ack`, or an error for a refusal.
    pub(crate) async fn receive_contact(&self, signed: SignedContact) -> Response {
        if let Err(e) = signed.verify() {
            return Response::Error(format!("invalid contact message: {e}"));
        }
        if signed.card().cert.root == self.inner.me {
            return Response::Ack;
        }
        if self.read().contact_book.blocked.contains(&signed.card().cert.root) {
            return Response::Ack;
        }
        let mut events = Vec::new();
        let outcome = {
            let mut guard = self.write();
            self.apply_contact(&mut guard, signed, &mut events)
        };
        match outcome {
            Ok(None) => {}
            Ok(Some(refusal)) => return Response::Error(refusal),
            Err(e) => return Response::Error(format!("could not store contact message: {e}")),
        }
        for ev in events {
            self.emit(ev);
        }
        self.kick_contact_outbox();
        Response::Ack
    }

    /// Queues our inbox list for every contact, replacing any announcement still waiting. Their
    /// outbox delivers it, and it is retried like any other contact message.
    pub(crate) fn queue_inbox_announcements(&self) {
        let mut guard = self.write();
        let w = &mut *guard;
        let Ok(mine) = self.my_card(w) else {
            return;
        };
        let contacts: Vec<RootId> = w.contacts.iter().map(|c| c.cert.root).collect();
        for to in contacts {
            let msg = ContactMsg::Inboxes {
                from_card: mine.clone(),
                devices: w.contact_book.my_inboxes.clone(),
                ts: now_ms(),
            };
            let Ok(signed) = SignedContact::sign(msg, &self.inner.identity.device) else {
                continue;
            };
            w.contact_book
                .outbox
                .retain(|o| !(o.to == to && matches!(o.msg.msg, ContactMsg::Inboxes { .. })));
            w.contact_book.outbox.push(Outbound {
                to,
                msg: signed,
                delivered: false,
            });
        }
        if let Err(e) = w.contact_book.save(&w.paths) {
            warn!(error = %e, "could not save inbox announcements");
        }
    }

    /// Queues a notice to `to` that this device started (`on`) or stopped holding their mail, replacing a
    /// notice still waiting. Their outbox delivers it.
    pub(crate) fn queue_inbox_host_notice(&self, w: &mut World, to: RootId, on: bool) {
        let Ok(mine) = self.my_card(w) else {
            return;
        };
        let msg = ContactMsg::InboxHost { from_card: mine, on, ts: now_ms() };
        let Ok(signed) = SignedContact::sign(msg, &self.inner.identity.device) else {
            return;
        };
        w.contact_book
            .outbox
            .retain(|o| !(o.to == to && matches!(o.msg.msg, ContactMsg::InboxHost { .. })));
        w.contact_book.outbox.push(Outbound {
            to,
            msg: signed,
            delivered: false,
        });
        if let Err(e) = w.contact_book.save(&w.paths) {
            warn!(error = %e, "could not save an inbox notice");
        }
    }

    /// Retries contact messages in the background. See `Engine::kick_conv_flush`.
    pub(crate) fn kick_contact_outbox(&self) {
        let engine = Engine {
            inner: self.inner.clone(),
        };
        self.spawn_task(async move { engine.retry_contact_outbox().await });
    }

    /// The inbox devices a contact message announces.
fn announced_inboxes(msg: &ContactMsg) -> Option<Vec<DeviceId>> {
    match msg {
        ContactMsg::Request(r) => Some(r.inboxes.clone()),
        ContactMsg::Accept { inboxes, .. } => Some(inboxes.clone()),
        ContactMsg::Inboxes { devices, .. } => Some(devices.clone()),
        ContactMsg::InboxHost { .. } => None,
    }
}

/// Applies a verified contact message to the world. `Ok(Some(reason))` refuses it.
    fn apply_contact(
        &self,
        guard: &mut World,
        signed: SignedContact,
        events: &mut Vec<EngineEvent>,
    ) -> Result<Option<String>> {
        let w = guard;
        let card = signed.card().clone();
        let from = card.cert.root;
        if let Some(inboxes) = Self::announced_inboxes(&signed.msg) {
            w.contact_book.known_inboxes.insert(from, inboxes);
        }
        w.directory.learn(&card.cert);
        for peer in card.peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()) {
            self.learn_address(w, peer);
        }
        match signed.msg {
            ContactMsg::Accept { .. } => {
                // Only an answer to a request we sent counts. Anything else is ignored.
                if w.contact_book.sent_request_to(&from) {
                    w.contact_book.drop_sent_request_to(&from);
                    befriend(w, card)?;
                    events.push(EngineEvent::ContactsChanged);
                }
            }
            ContactMsg::Inboxes { .. } => {}
            ContactMsg::InboxHost { on, ts, .. } => {
                if w.inbox.record_notice(from, card.cert.device, on, ts) {
                    w.inbox.save(&w.paths)?;
                    events.push(EngineEvent::InboxHostChanged {
                        root_id: from.to_string(),
                        display_name: w.display_name(&from),
                        device_id: card.cert.device.to_string(),
                        on,
                    });
                }
            }
            ContactMsg::Request(request) => {
                if w.is_contact(&from) || w.contact_book.declined.contains(&from) {
                    // Already settled, or declined. Ack without storing, so the sender stops.
                    return Ok(None);
                }
                let now = now_ms();
                if w.contact_book.over_limit(&from, now) {
                    return Ok(Some("too many contact requests today".into()));
                }
                let known = w
                    .contact_book
                    .incoming
                    .iter()
                    .any(|r| r.from_card.cert.root == from);
                if !known && w.contact_book.incoming.len() >= MAX_PENDING_INCOMING {
                    return Ok(Some("too many pending contact requests".into()));
                }
                w.contact_book.record(from, now);
                if w.contact_book.sent_request_to(&from) {
                    // Both sides asked. Accepting completes both requests.
                    w.contact_book.drop_sent_request_to(&from);
                    befriend(w, card)?;
                    self.queue_accept(w, from)?;
                    events.push(EngineEvent::ContactsChanged);
                } else {
                    w.contact_book
                        .incoming
                        .retain(|r| r.from_card.cert.root != from);
                    w.contact_book.incoming.push(request);
                    events.push(EngineEvent::ContactRequest {
                        user: from.to_string(),
                    });
                }
            }
        }
        w.contact_book.save(&w.paths)?;
        Ok(None)
    }
}

/// One person on the contacts page.
#[derive(Clone, Debug, Serialize)]
pub struct ContactEntry {
    pub root_id: String,
    pub display_name: String,
    pub suffix: String,
    /// What others see of this person. See `status`.
    pub status: crate::presence::Presence,
    /// True unless `status` is offline. Kept for older clients.
    pub online: bool,
    /// Names of the servers we share with them.
    pub shared_servers: Vec<String>,
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
    /// When this person was added as a contact, in milliseconds. `None` if they are not one.
    pub contact_since_ms: Option<u64>,
    /// When this device first saw this person, in milliseconds.
    pub first_seen_ms: Option<u64>,
}

/// The contacts page: who asked us, whom we asked, contacts, and blocked people.
#[derive(Clone, Debug, Serialize)]
pub struct ContactsView {
    pub incoming: Vec<ContactEntry>,
    pub outgoing: Vec<ContactEntry>,
    pub contacts: Vec<ContactEntry>,
    pub blocked: Vec<ContactEntry>,
}

impl Engine {
    /// The contacts page, as the desktop lists it.
    pub fn list_contacts(&self) -> ContactsView {
        let w = self.read();
        let now = now_ms();
        let entry = |root: RootId| {
            let status = w.presence_of(&root, now);
            ContactEntry {
            root_id: root.to_string(),
            display_name: w.display_name(&root),
            suffix: root.suffix(),
            status,
            online: status != Presence::Offline,
            shared_servers: w
                .communities
                .values()
                .filter(|c| c.is_member(&root) && c.is_member(&w.me))
                .map(|c| c.name().to_string())
                .collect(),
            avatar: w.avatar_of(&root),
            contact_since_ms: w.contact_times.since.get(&root).copied(),
            first_seen_ms: w.contact_times.first_seen.get(&root).copied(),
            }
        };
        ContactsView {
            incoming: w
                .contact_book
                .incoming
                .iter()
                .map(|r| entry(r.from_card.cert.root))
                .collect(),
            outgoing: w
                .contact_book
                .outbox
                .iter()
                .filter(|o| matches!(o.msg.msg, ContactMsg::Request(_)))
                .map(|o| entry(o.to))
                .collect(),
            contacts: w.contacts.iter().map(|c| entry(c.cert.root)).collect(),
            blocked: w.contact_book.blocked.iter().map(|r| entry(*r)).collect(),
        }
    }

    /// Blocks `user`. Their requests and messages are dropped without a reply, and any pending
    /// request either way is cancelled. Their one-to-one conversation is hidden.
    pub fn block_user(&self, user: RootId) -> Result<()> {
        {
            let mut guard = self.write();
            let w = &mut *guard;
            w.contact_book.blocked.insert(user);
            w.contacts.retain(|c| c.cert.root != user);
            w.contact_times.stopped_contact(&user);
            w.contact_times.save(&w.paths)?;
            w.contact_book.incoming.retain(|r| r.from_card.cert.root != user);
            w.contact_book.outbox.retain(|o| o.to != user);
            if let Some(meta) = w.conv_book.convs.get_mut(&dm_conv_id(&w.me, &user)) {
                meta.left = true;
            }
            w.contact_book.save(&w.paths)?;
            w.save_contacts()?;
            w.conv_book.silenced.insert(dm_conv_id(&w.me, &user));
            w.conv_book.save(&w.paths)?;
        }
        self.emit(EngineEvent::ContactsChanged);
        Ok(())
    }

    pub fn unblock_user(&self, user: RootId) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        w.contact_book.blocked.remove(&user);
        w.contact_book.save(&w.paths)?;
        drop(guard);
        self.emit(EngineEvent::ContactsChanged);
        Ok(())
    }

    /// Removes a contact. Messages with them keep working unless the policy says otherwise.
    pub fn remove_contact(&self, user: RootId) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        let pictures = w.pictures_of(&user);
        w.contacts.retain(|c| c.cert.root != user);
        w.contact_times.stopped_contact(&user);
        w.contact_times.save(&w.paths)?;
        w.save_contacts()?;
        drop(guard);
        // Their pictures go unless we still share a server or a conversation with them.
        self.release_blobs(pictures);
        self.emit(EngineEvent::ContactsChanged);
        Ok(())
    }

    /// Withdraws a request we sent, if it is still waiting.
    pub fn cancel_contact_request(&self, user: RootId) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        w.contact_book
            .outbox
            .retain(|o| !(o.to == user && matches!(o.msg.msg, ContactMsg::Request(_))));
        w.contact_book.save(&w.paths)?;
        drop(guard);
        self.emit(EngineEvent::ContactsChanged);
        Ok(())
    }

    /// A `dissent://contact/...` link for one of our contacts. It carries only their public reachability, as our own
    /// link does: whatever local addresses their card held stay on this device.
    pub fn peer_contact_link(&self, user: RootId) -> Result<String> {
        self.read()
            .contacts
            .iter()
            .find(|c| c.cert.root == user)
            .map(|c| {
                let mut card = c.clone();
                card.peers = card
                    .peers
                    .iter()
                    .filter_map(|p| p.parse::<PeerAddr>().ok())
                    .map(|a| self.shareable(&a).to_string())
                    .collect();
                card.to_url().to_string()
            })
            .ok_or_else(|| CoreError::NotFound(format!("contact {user}")))
    }
}

/// How long a sender waits between explicit refusals. Within this window, a refused sender gets
/// the same silent acknowledgement a blocked one does.
pub(crate) const REFUSAL_WINDOW_MS: u64 = 10 * 60 * 1000;

/// The reply to a message the recipient's policy refuses. At most one explicit refusal per sender
/// per window. After that, the sender gets a silent acknowledgement, the same as a blocked sender,
/// so repeated sends cannot show whether this person is around.
pub(crate) fn refusal_for(w: &mut World, from: RootId, reason: String) -> Response {
    if w.contact_book.blocked.contains(&from) {
        return Response::Ack;
    }
    let now = now_ms();
    w.contact_book
        .last_refusal
        .retain(|_, t| now.saturating_sub(*t) < REFUSAL_WINDOW_MS);
    if w.contact_book.last_refusal.contains_key(&from) {
        return Response::Ack;
    }
    w.contact_book.last_refusal.insert(from, now);
    if let Err(e) = w.contact_book.save(&w.paths) {
        warn!(error = %e, "could not save refusal state");
    }
    Response::Refused(reason)
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::store::Paths;
    use dissent_identity::RootKey;

    /// The contact book as written before the address setting existed. Its layout is what is on disk
    /// for every existing install, so it must keep loading. Do not change this struct.
    #[derive(Serialize, Deserialize)]
    struct ContactBookV1 {
        incoming: Vec<ContactRequest>,
        outbox: Vec<Outbound>,
        declined: BTreeSet<RootId>,
        recent: Vec<(RootId, u64)>,
        policy: DmPolicy,
        my_inboxes: Vec<DeviceId>,
        known_inboxes: BTreeMap<RootId, Vec<DeviceId>>,
        blocked: BTreeSet<RootId>,
        last_refusal: BTreeMap<RootId, u64>,
    }

    fn scratch_paths(name: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("dissent-{name}-{}", now_ms()));
        Paths::for_test(root)
    }

    #[test]
    fn a_contact_book_from_before_the_address_setting_still_loads() {
        let paths = scratch_paths("contact-book-v1");
        let blocked = RootKey::generate().id();
        let old = ContactBookV1 {
            incoming: vec![],
            outbox: vec![],
            declined: BTreeSet::new(),
            recent: vec![],
            policy: DmPolicy::ContactsOnly,
            my_inboxes: vec![],
            known_inboxes: BTreeMap::new(),
            blocked: BTreeSet::from([blocked]),
            last_refusal: BTreeMap::new(),
        };
        crate::store::write_bin_plain(&paths.contact_book(), &old).unwrap();
        let book = ContactBook::load(&paths).expect("an older contact book loads");
        assert_eq!(book.policy, DmPolicy::ContactsOnly);
        assert!(book.blocked.contains(&blocked));
        let _ = std::fs::remove_dir_all(paths.contact_book().parent().unwrap());
    }

    #[test]
    fn the_address_setting_defaults_to_on_and_is_stored_apart() {
        let paths = scratch_paths("address-privacy");
        assert!(!hide_address(&paths), "missing means discoverable");
        crate::store::write_bin_plain(&paths.address_privacy(), &true).unwrap();
        assert!(hide_address(&paths));
        assert!(!paths.contact_book().exists(), "the contact book is not touched by the setting");
        let _ = std::fs::remove_dir_all(paths.address_privacy().parent().unwrap());
    }
}
