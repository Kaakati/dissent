//! Members-only blob fetch (`docs/design/14-file-tracker.md`, decision 5, and `docs/design/08-files.md`, "Who may
//! fetch a blob").
//!
//! The blob server serves a device only when its root may see something the blob belongs to:
//!
//! - **Community**: an active person or storage member of a community whose messages, manifests or hints name it.
//! - **Channel**: a reader of the private channel whose message names it. Storage members are left out, as they are
//!   left out of private channels' hints.
//! - **Conv**: a member of the direct or group conversation whose message names it.
//! - **Shared**: a person's picture (avatar, profile banner). A contact, or anyone who shares a community or a
//!   conversation with us.
//! - **Public**: a server's icon and banner. Anyone: an invite card shows them to people who are not members yet,
//!   as the invite link that carries their keys does.
//!
//! Our own devices are always served, and so is a `Public` blob. A device whose certificate we do not hold yet is refused, and we ask it for its
//! signed address record at once, which carries the certificate, so the fetcher's retry is served. A hash with no
//! scope is refused: we serve only what we hold for someone.
//!
//! The scopes live in memory. They are built at start from what this device already keeps, and added as blobs come
//! in. Hints a seeder accepted are the only scopes that cannot be rebuilt from the content, so they are kept in their
//! own file. A request for a hash with no scope rebuilds the index first, at most every `REBUILD_GAP`, so a blob
//! that came in through a path that records no scope is still served.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use dissent_events::{ChannelId, EventId, Payload};
use dissent_files::gate::{
    AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode, RequestUpdate,
    RequestUpdates, ThrottleMode,
};
use dissent_identity::{CommunityId, DeviceId, RootId};
use tracing::debug;

use crate::engine::{Engine, Inner};
use crate::groups::ConvId;
use crate::rpc::{Request, Response};
use crate::world::World;

/// The shortest time between two rebuilds that a request for an unknown hash may start.
const REBUILD_GAP: Duration = Duration::from_secs(2);
/// The shortest time between two certificate asks to one device.
const INTRODUCE_GAP: Duration = Duration::from_secs(10);
/// Messages the blob server may queue for the gate before it waits.
const GATE_QUEUE: usize = 64;

/// Something a blob belongs to. Whoever may see it may fetch the blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Community(CommunityId),
    Channel(CommunityId, ChannelId),
    Conv(ConvId),
    Shared,
    Public,
}

/// Which scopes each blob held on this device belongs to.
#[derive(Debug, Default)]
pub(crate) struct BlobScopes {
    by_hash: HashMap<[u8; 32], Vec<Scope>>,
    /// Blobs a seeder keeps because a member hinted them, with the hint's community. Persisted.
    hinted: BTreeMap<[u8; 32], CommunityId>,
    rebuilt: Option<Instant>,
    /// Devices a test told the gate to treat as unknown until it asks them for their certificate. See
    /// `test_support::gate_forget_device`.
    pub(crate) forgotten: std::collections::BTreeSet<DeviceId>,
}

impl BlobScopes {
    pub(crate) fn add(&mut self, hash: [u8; 32], scope: Scope) {
        let scopes = self.by_hash.entry(hash).or_default();
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }

    /// The blobs a seeder keeps because members hinted them. They stay referenced.
    pub(crate) fn hinted_blobs(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        self.hinted.keys().copied()
    }

    fn scopes(&self, hash: &[u8; 32]) -> &[Scope] {
        self.by_hash.get(hash).map_or(&[], Vec::as_slice)
    }
}

/// What the gate says to one device about one blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allow,
    Deny,
    /// No scope holds the hash. A rebuild may find one.
    NoScope,
    /// We hold no certificate for the device.
    Unknown,
}

/// Every blob a message's sealed text names: its files, their thumbnails and its link previews' pictures.
pub(crate) fn message_blobs(plain: &str) -> Vec<[u8; 32]> {
    let files = crate::attachments::unpack(plain).1.into_iter().map(|a| a.file.hash);
    let thumbs = crate::attachments::thumbs_of(plain).into_iter().map(|t| t.file.hash);
    let pictures = crate::attachments::embed_extras_of(plain)
        .into_iter()
        .filter_map(|e| e.image)
        .map(|i| i.file.hash);
    files.chain(thumbs).chain(pictures).collect()
}

/// Every picture blob this device may hold, with its scope: people's avatars and banners are `Shared`, servers'
/// icons and banners `Public`.
fn picture_hashes(w: &World) -> Vec<([u8; 32], Scope)> {
    let mut people = Vec::new();
    let mut servers = Vec::new();
    people.extend(w.own_avatar.map(|p| p.hash));
    people.extend(w.banners.own.iter().flat_map(|b| b.pictures()).map(|p| p.hash));
    people.extend(w.peer_avatars.values().filter_map(|(_, p)| p.map(|p| p.hash)));
    people.extend(w.banners.peers.values().flat_map(|(_, b)| b.iter().flat_map(|b| b.pictures())).map(|p| p.hash));
    for c in w.communities.values() {
        let s = c.state();
        people.extend(s.avatars.values().filter_map(|(_, p)| p.map(|p| p.hash)));
        people.extend(s.profile_banners.values().flat_map(|(_, b)| b.iter().flat_map(|b| b.pictures())).map(|p| p.hash));
        servers.extend(s.icon.and_then(|(_, p)| p).map(|p| p.hash));
        servers.extend(s.animated_icon.and_then(|(_, i)| i).into_iter().flat_map(|i| [i.image.hash, i.poster.hash]));
        servers.extend(s.banner.and_then(|(_, b)| b).iter().flat_map(|b| b.pictures()).map(|p| p.hash));
    }
    let people = people.into_iter().map(|h| (h, Scope::Shared));
    people.chain(servers.into_iter().map(|h| (h, Scope::Public))).collect()
}

/// The scope of a message in a community: its private channel, or the community. `None` when the message is in no
/// channel we know of, so nothing is recorded for it rather than too much.
fn channel_scope(w: &World, community: CommunityId, message: EventId) -> Option<Scope> {
    let s = w.communities.get(&community)?.state();
    let (channel, _) = s.messages.iter().find(|(_, ms)| ms.iter().any(|m| m.id == message))?;
    Some(if s.is_private(*channel) { Scope::Channel(community, *channel) } else { Scope::Community(community) })
}

impl World {
    /// Builds the scope index again from what this device keeps. Hinted blobs are kept as they are.
    pub(crate) fn rebuild_blob_scopes(&mut self) {
        let mut scopes = BlobScopes {
            hinted: std::mem::take(&mut self.blob_scopes.hinted),
            forgotten: std::mem::take(&mut self.blob_scopes.forgotten),
            ..BlobScopes::default()
        };
        for (hash, community) in &scopes.hinted.clone() {
            scopes.add(*hash, Scope::Community(*community));
        }
        for (id, c) in &self.communities {
            let s = c.state();
            let channel_of: HashMap<EventId, ChannelId> =
                s.messages.iter().flat_map(|(ch, ms)| ms.iter().map(move |m| (m.id, *ch))).collect();
            for (message, (_, text)) in &c.content {
                let blobs = message_blobs(text);
                let Some(channel) = channel_of.get(message).filter(|_| !blobs.is_empty()) else {
                    continue;
                };
                let scope = if s.is_private(*channel) { Scope::Channel(*id, *channel) } else { Scope::Community(*id) };
                for hash in blobs {
                    scopes.add(hash, scope);
                }
            }
            // A seeder cannot open the sealed text, so its mirrors are found through the cleartext manifests.
            for se in c.sealed_in_order() {
                if let Payload::Sealed { ciphertext, .. } = &se.event.body.payload {
                    for entry in crate::attachments::split_manifest(ciphertext).0 {
                        scopes.add(entry.hash, Scope::Community(*id));
                    }
                }
            }
        }
        for (conv, msgs) in &self.conv_msgs {
            for m in msgs {
                for hash in message_blobs(&m.text) {
                    scopes.add(hash, Scope::Conv(*conv));
                }
            }
        }
        for (hash, scope) in picture_hashes(self) {
            scopes.add(hash, scope);
        }
        scopes.rebuilt = Some(Instant::now());
        self.blob_scopes = scopes;
    }

    /// Records the blobs a community message names, under its channel's scope.
    pub(crate) fn index_community_message(&mut self, community: CommunityId, message: EventId, plain: &str) {
        let blobs = message_blobs(plain);
        if blobs.is_empty() {
            return;
        }
        if let Some(scope) = channel_scope(self, community, message) {
            for hash in blobs {
                self.blob_scopes.add(hash, scope);
            }
        }
    }

    /// Records the blobs a conversation message names.
    pub(crate) fn index_conv_message(&mut self, conv: ConvId, plain: &str) {
        for hash in message_blobs(plain) {
            self.blob_scopes.add(hash, Scope::Conv(conv));
        }
    }

    /// The root of `device`: from its certificate, else from the events it signed in a community.
    fn root_of_device(&self, device: &DeviceId) -> Option<RootId> {
        if self.blob_scopes.forgotten.contains(device) {
            return None;
        }
        self.directory
            .root_of(device)
            .or_else(|| self.communities.values().find_map(|c| c.state().device_roots.get(device).copied()))
    }

    /// Whether `root` is an active member of `community`: a person or a storage member, not banned and not gone.
    /// `person` leaves storage members out.
    fn is_member(&self, community: &CommunityId, root: &RootId, person: bool) -> bool {
        let Some(c) = self.communities.get(community) else {
            return false;
        };
        let s = c.state();
        let root = s.canonical(root);
        !s.banned.contains(&root) && s.members.get(&root).is_some_and(|m| !m.left && !(person && m.storage))
    }

    fn in_conv(&self, conv: &ConvId, root: &RootId) -> bool {
        self.conv_book.convs.get(conv).is_some_and(|m| !m.left && m.members.contains(root))
    }

    /// Whether `root` is a contact, or shares a community or a conversation with this device.
    fn shares_with(&self, root: &RootId) -> bool {
        self.is_contact(root)
            || self.communities.keys().any(|c| self.is_member(c, root, false))
            || self.conv_book.convs.keys().any(|conv| self.in_conv(conv, root))
    }

    fn admits(&self, scope: &Scope, root: &RootId) -> bool {
        match scope {
            Scope::Community(c) => self.is_member(c, root, false),
            Scope::Channel(c, channel) => {
                self.is_member(c, root, true)
                    && self.communities.get(c).is_some_and(|c| {
                        let s = c.state();
                        s.can_read(*channel, s.canonical(root))
                    })
            }
            Scope::Conv(conv) => self.in_conv(conv, root),
            Scope::Shared => self.shares_with(root),
            Scope::Public => true,
        }
    }

    /// Whether `hash` is the icon or banner of a server we are in: a few lookups per server.
    fn is_server_art(&self, hash: &[u8; 32]) -> bool {
        self.communities.values().any(|c| {
            let s = c.state();
            s.icon.and_then(|(_, p)| p).is_some_and(|p| p.hash == *hash)
                || s.animated_icon.and_then(|(_, i)| i).is_some_and(|i| i.image.hash == *hash || i.poster.hash == *hash)
                || s.banner.and_then(|(_, b)| b).is_some_and(|b| b.pictures().iter().any(|p| p.hash == *hash))
        })
    }

    /// Whether `device` may fetch the blob `hash`. One read of the world, a directory lookup and a map lookup.
    pub(crate) fn may_fetch(&self, me: &RootId, device: &DeviceId, hash: &[u8; 32]) -> Verdict {
        let scopes = self.blob_scopes.scopes(hash);
        // A server's art is read from the state, not only the index, so a banner set a moment ago is served at once.
        if scopes.contains(&Scope::Public) || self.is_server_art(hash) {
            return Verdict::Allow;
        }
        let Some(root) = self.root_of_device(device) else {
            return Verdict::Unknown;
        };
        if root == *me {
            return Verdict::Allow;
        }
        if self.communities.values().any(|c| c.state().revoked_devices.contains_key(device)) {
            return Verdict::Deny;
        }
        if scopes.is_empty() {
            return Verdict::NoScope;
        }
        if scopes.iter().any(|s| self.admits(s, &root)) {
            Verdict::Allow
        } else {
            Verdict::Deny
        }
    }
}

/// When the gate last asked each device for its certificate.
#[derive(Default)]
pub(crate) struct Introductions {
    asked: Mutex<BTreeMap<DeviceId, Instant>>,
}

impl Introductions {
    fn admit(&self, device: DeviceId, now: Instant) -> bool {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        if asked.get(&device).is_some_and(|t| now.duration_since(*t) < INTRODUCE_GAP) {
            return false;
        }
        asked.insert(device, now);
        true
    }
}

impl Engine {
    /// Records that `hash` belongs to `scope`, so the devices that may see the scope may fetch it.
    pub(crate) fn add_blob_scope(&self, hash: [u8; 32], scope: Scope) {
        self.write().blob_scopes.add(hash, scope);
    }

    /// Records blobs a seeder keeps for `community` because a member hinted them, and keeps the record on disk.
    pub(crate) fn add_hinted_blobs(&self, community: CommunityId, hashes: &[[u8; 32]]) {
        let mut w = self.write();
        for hash in hashes {
            w.blob_scopes.add(*hash, Scope::Community(community));
            w.blob_scopes.hinted.insert(*hash, community);
        }
        let hinted: Vec<([u8; 32], CommunityId)> = w.blob_scopes.hinted.iter().map(|(h, c)| (*h, *c)).collect();
        if let Err(e) = w.paths.write_bin(&w.paths.blob_hints(), &hinted) {
            debug!(error = %e, "could not save hinted blobs");
        }
    }

    /// Loads the hinted blobs and builds the scope index. Called once at start.
    pub(crate) fn load_blob_scopes(&self) {
        let mut w = self.write();
        let hinted: Vec<([u8; 32], CommunityId)> = w.paths.read_bin(&w.paths.blob_hints()).ok().flatten().unwrap_or_default();
        w.blob_scopes.hinted = hinted.into_iter().collect();
        w.rebuild_blob_scopes();
    }

    /// The gate's verdict for one request. A refusal rebuilds the index first, at most every `REBUILD_GAP`, so a
    /// blob whose scope came in by a path that records none, such as a new server banner, is still served.
    fn gate_fetch(&self, device: &DeviceId, hash: &[u8; 32]) -> Verdict {
        let me = self.inner.me;
        let verdict = self.read().may_fetch(&me, device, hash);
        if verdict == Verdict::Allow {
            return verdict;
        }
        let mut w = self.write();
        if w.blob_scopes.rebuilt.is_some_and(|t| t.elapsed() < REBUILD_GAP) {
            return verdict;
        }
        w.rebuild_blob_scopes();
        w.may_fetch(&me, device, hash)
    }

    /// Asks a device we hold no certificate for for its signed address record, and learns the certificate from it.
    /// The record is kept only if its root shares something with us, as every record is.
    fn introduce(&self, device: DeviceId) {
        if !self.inner.introductions.admit(device, Instant::now()) {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            match engine.call(device, &Request::AddrLookup { device }).await {
                Ok(Response::AddrLookup { record: Some(record), .. }) if record.device() == device => {
                    // The certificate is learned before the address. A record no newer than the one held still
                    // brings the certificate, so only the outcome below matters.
                    if let Err(e) = engine.learn_record(*record) {
                        debug!(device = %device, error = %e, "the record of a refused device was not kept");
                    }
                    let mut w = engine.write();
                    if w.directory.root_of(&device).is_some() {
                        w.blob_scopes.forgotten.remove(&device);
                    }
                }
                Ok(other) => debug!(device = %device, reply = ?other, "a refused device sent no record"),
                Err(e) => debug!(device = %device, error = %e, "could not ask a refused device for its record"),
            }
        });
    }
}

/// Whether the gate asks `Engine::throttle` before each chunk it sends. Off: the tracker turns it on with its upload
/// caps. Each ask is one in-process round trip per 16 KiB.
const THROTTLE: ThrottleMode = ThrottleMode::None;

impl Engine {
    /// Bytes of `hash` this device just sent to `device`. The tracker's seeding counters read it.
    pub(crate) fn on_served(&self, _device: DeviceId, _hash: [u8; 32], _bytes: u64) {}

    /// How long to wait before sending `bytes` more of `hash` to `device`. The tracker's upload caps answer it;
    /// asked only while `THROTTLE` is on.
    pub(crate) fn throttle(&self, _device: DeviceId, _hash: [u8; 32], _bytes: u64) -> Duration {
        Duration::ZERO
    }

    /// Whether this device serves `hash` at all once the requester may fetch it. The tracker says no for a listing
    /// whose seeding is paused, stopped or queued. Attachments and pictures are always served.
    pub(crate) fn serves(&self, _hash: &[u8; 32]) -> bool {
        true
    }
}

/// The blob server's event sender, and the task that answers it. The task holds the engine weakly, so it ends with
/// the engine; until the engine is in place every request is refused.
pub(crate) fn gate_events(inner: &Arc<Inner>) -> EventSender {
    let mask = EventMask {
        connected: ConnectMode::Intercept,
        // `InterceptLog` adds the transfer events, which count what was served.
        get: RequestMode::InterceptLog,
        get_many: RequestMode::InterceptLog,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
        throttle: THROTTLE,
    };
    let (events, rx) = EventSender::channel(GATE_QUEUE, mask);
    tokio::spawn(run_gate(Arc::downgrade(inner), rx));
    events
}

fn answer(verdict: Verdict) -> std::result::Result<(), AbortReason> {
    match verdict {
        Verdict::Allow => Ok(()),
        _ => Err(AbortReason::Permission),
    }
}

/// The blob being sent for each served request, by (connection, request), for the throttle.
type Sending = Arc<Mutex<HashMap<(u64, u64), (DeviceId, [u8; 32])>>>;

/// Reads one served request's transfer events: each chunk sent is credited to its blob, and the request is
/// forgotten when it ends.
async fn count_served(
    engine: Engine,
    device: DeviceId,
    key: (u64, u64),
    sending: Sending,
    mut updates: RequestUpdates,
) {
    let mut current: Option<([u8; 32], u64)> = None;
    while let Ok(Some(update)) = updates.recv().await {
        match update {
            RequestUpdate::Started(started) => {
                let hash = *started.hash.as_bytes();
                current = Some((hash, 0));
                sending.lock().unwrap_or_else(PoisonError::into_inner).insert(key, (device, hash));
            }
            RequestUpdate::Progress(progress) => {
                if let Some((hash, sent)) = current.as_mut() {
                    let delta = progress.end_offset.saturating_sub(*sent);
                    *sent = progress.end_offset.max(*sent);
                    if delta > 0 {
                        engine.on_served(device, *hash, delta);
                    }
                }
            }
            RequestUpdate::Completed(_) | RequestUpdate::Aborted(_) => break,
        }
    }
    sending.lock().unwrap_or_else(PoisonError::into_inner).remove(&key);
}

async fn run_gate(inner: Weak<Inner>, mut rx: tokio::sync::mpsc::Receiver<ProviderMessage>) {
    // The device on each open connection, from its connect event.
    let mut devices: BTreeMap<u64, DeviceId> = BTreeMap::new();
    let sending: Sending = Arc::default();
    while let Some(msg) = rx.recv().await {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let engine = Engine { inner };
        // Each hash checked for one request, the connection it came on, and the request's id.
        let (conn, request, hashes): (u64, u64, Vec<[u8; 32]>) = match &msg {
            ProviderMessage::ClientConnected(m) => {
                // Every device may connect: a server's icon and banner are for anyone, so each request is checked.
                let device = m.endpoint_id.map(|id| DeviceId(*id.as_bytes()));
                let verdict = match device {
                    Some(device) => {
                        devices.insert(m.connection_id, device);
                        Verdict::Allow
                    }
                    None => Verdict::Deny,
                };
                if let ProviderMessage::ClientConnected(m) = msg {
                    m.tx.send(answer(verdict)).await.ok();
                }
                continue;
            }
            ProviderMessage::ConnectionClosed(m) => {
                devices.remove(&m.connection_id);
                continue;
            }
            ProviderMessage::Throttle(m) => {
                let blob = sending.lock().unwrap_or_else(PoisonError::into_inner).get(&(m.connection_id, m.request_id)).copied();
                let wait = blob.map_or(Duration::ZERO, |(device, hash)| engine.throttle(device, hash, m.size));
                if let ProviderMessage::Throttle(m) = msg {
                    // The wait runs on its own, so one slow transfer does not hold the gate.
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        m.tx.send(Ok(())).await.ok();
                    });
                }
                continue;
            }
            ProviderMessage::GetRequestReceived(m) => (m.connection_id, m.request_id, vec![*m.request.hash.as_bytes()]),
            ProviderMessage::GetManyRequestReceived(m) => {
                (m.connection_id, m.request_id, m.request.hashes.iter().map(|h| *h.as_bytes()).collect())
            }
            ProviderMessage::ObserveRequestReceived(m) => {
                (m.connection_id, m.request_id, vec![*m.request.hash.as_bytes()])
            }
            // Push is disabled and nothing else is intercepted, so nothing else needs an answer.
            _ => continue,
        };
        let device = devices.get(&conn).copied();
        let verdict = match device {
            None => Verdict::Deny,
            Some(device) => {
                let verdicts: Vec<Verdict> = hashes.iter().map(|h| engine.gate_fetch(&device, h)).collect();
                if verdicts.contains(&Verdict::Unknown) {
                    engine.introduce(device);
                }
                let allowed = !hashes.is_empty() && verdicts.iter().all(|v| *v == Verdict::Allow);
                if allowed && hashes.iter().all(|h| engine.serves(h)) {
                    Verdict::Allow
                } else {
                    Verdict::Deny
                }
            }
        };
        let result = answer(verdict);
        let count = |updates| {
            if let (Verdict::Allow, Some(device)) = (verdict, device) {
                tokio::spawn(count_served(engine.clone(), device, (conn, request), sending.clone(), updates));
            }
        };
        match msg {
            ProviderMessage::GetRequestReceived(m) => {
                m.tx.send(result).await.ok();
                count(m.rx);
            }
            ProviderMessage::GetManyRequestReceived(m) => {
                m.tx.send(result).await.ok();
                count(m.rx);
            }
            ProviderMessage::ObserveRequestReceived(m) => {
                m.tx.send(result).await.ok();
            }
            _ => {}
        }
    }
}
