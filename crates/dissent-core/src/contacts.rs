//! Contact requests and direct-message privacy.
//!
//! Adding a contact link no longer makes a contact. It sends a signed `ContactRequest` to the
//! person's devices. They become a contact once the recipient accepts, and the accept comes back
//! as a signed `Accept`. A declined sender hears nothing, and is remembered so it cannot spam.
//! Undelivered requests and accepts wait in `ContactBook::outbox` and are retried by maintenance.

use std::collections::{BTreeMap, BTreeSet};

use crate::presence::Presence;
use dissent_discovery::ContactCard;
use dissent_identity::{DeviceId, DeviceKey, RootId, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{decode, encode, Request, Response};
use crate::store::{read_bin, write_bin, Paths};
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
            ContactMsg::Accept { from_card, .. } | ContactMsg::Inboxes { from_card, .. } => from_card,
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

impl ContactBook {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(read_bin(&paths.contact_book())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_bin(&paths.contact_book(), self)
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
        let card = ContactCard::parse(link)?;
        let user = card.cert.root;
        if user == self.inner.me {
            return Err(CoreError::Invalid("that is your own contact link".into()));
        }
        let note = note.trim().to_string();
        if note.chars().count() > MAX_NOTE_CHARS {
            return Err(CoreError::Invalid(format!(
                "note is longer than {MAX_NOTE_CHARS} characters"
            )));
        }
        {
            let mut guard = self.write();
            let w = &mut *guard;
            w.directory.learn(&card.cert);
            for peer in card.peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()) {
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
                ContactMsg::Accept { .. } | ContactMsg::Inboxes { .. } => None,
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

    /// Our own card, built the same way as `contact_link`.
    fn my_card(&self, w: &World) -> Result<ContactCard> {
        let peers: Vec<String> = w.my_addr.iter().map(|a| a.to_string()).collect();
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

    /// Retries contact messages in the background. See `Engine::kick_conv_flush`.
    fn kick_contact_outbox(&self) {
        let engine = Engine {
            inner: self.inner.clone(),
        };
        self.spawn_task(async move { engine.retry_contact_outbox().await });
    }

    /// The inbox devices a contact message announces.
fn announced_inboxes(msg: &ContactMsg) -> Vec<DeviceId> {
    match msg {
        ContactMsg::Request(r) => r.inboxes.clone(),
        ContactMsg::Accept { inboxes, .. } => inboxes.clone(),
        ContactMsg::Inboxes { devices, .. } => devices.clone(),
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
        w.contact_book
            .known_inboxes
            .insert(from, Self::announced_inboxes(&signed.msg));
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
        w.contacts.retain(|c| c.cert.root != user);
        w.save_contacts()?;
        drop(guard);
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

    /// A `dissent://contact/...` link for one of our contacts.
    pub fn peer_contact_link(&self, user: RootId) -> Result<String> {
        self.read()
            .contacts
            .iter()
            .find(|c| c.cert.root == user)
            .map(|c| c.to_url().to_string())
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
