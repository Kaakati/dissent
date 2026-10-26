//! Network side: inbound gossip and requests, direct message delivery, and background work.
//! No lock is held across a network await.

use crate::access::InEnvelope;
use crate::community::Community;
use crate::engine::{Engine, Inner, MAX_CLOCK_SKEW_MS, RETRY_EVERY, not_found, now_ms};
use crate::error::Result;
use crate::groups::ConvBook;
use crate::identity_watch;
use crate::identity_watch::IdentityChange;
use crate::messages::event_for;
use crate::rpc::{Request, Response, decode};
use crate::store::{append_records, Paths};
use crate::typing::TypingTarget;
use crate::views::EngineEvent;
use crate::wire::{GossipMsg, decode_gossip};
use crate::world::World;
use dissent_events::{open_event_keyed, Payload, SealKey, SealedKind, SignedDm, SignedEvent};
use dissent_identity::{CommunityId, DeviceId, RootId};
use dissent_net::{PeerAddr, TopicReceiver};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// What accepted events ask of the engine. Collected under the lock, performed after it.
#[derive(Default)]
pub(crate) struct ControlEffects {
    emitted: Vec<EngineEvent>,
    relay_sets: Vec<Vec<String>>,
    pictures: Vec<crate::avatars::PictureFetch>,
    /// An accepted `CommunityRename`, so the founder republishes the record.
    renamed: bool,
}

/// Collects what the accepted `events` ask of the engine: a notice for each, the relays a
/// `SetRelays` names, and an identity notice for a `KeyRotation` that concerns someone we know.
/// The caller passes only events that replay accepted and that have not been handled yet.
#[allow(clippy::too_many_arguments)]
pub(crate) fn control_effects(
    community: CommunityId,
    c: &Community,
    paths: &Paths,
    dms: &BTreeMap<RootId, Vec<SignedDm>>,
    conv_book: &ConvBook,
    identity_changes: &mut Vec<IdentityChange>,
    now: u64,
    events: &[SignedEvent],
) -> ControlEffects {
    let mut fx = ControlEffects::default();
    for se in events {
        // Both rings, so a member-sealed event has its effects too.
        let Ok(ev) = open_event_keyed(&se.event, c.ring(), c.epoch_ring()) else {
            continue;
        };
        if let Payload::SetRelays { relays } = &ev.body.payload {
            fx.relay_sets.push(relays.clone());
        }
        if let Some(fetch) = crate::avatars::picture_fetch_for(community, c, ev.body.author, &ev.body.payload) {
            fx.pictures.push(fetch);
        }
        fx.renamed |= matches!(ev.body.payload, Payload::CommunityRename { .. });
        if let Payload::KeyRotation(rot) = &ev.body.payload {
            let (old, new) = (rot.request.old_root, rot.request.new_root);
            if c.is_member(&new)
                || c.is_member(&old)
                || identity_watch::messaged(dms, conv_book, &new)
                || identity_watch::messaged(dms, conv_book, &old)
            {
                let key = format!("rot:{:?}", se.event.id);
                let path = paths.identity_changes();
                let reason = identity_watch::ROTATED;
                if let Some(notice) =
                    identity_watch::record(identity_changes, &path, new, key, reason, now)
                {
                    fx.emitted.push(notice);
                }
            }
        }
        let notice = event_for(&community, &se.event);
        if !fx.emitted.contains(&notice) {
            fx.emitted.push(notice);
        }
    }
    fx
}

impl World {
    /// Installs `ring` on `community`. Returns the effects of every event it unlocked, each one
    /// exactly once, for the caller to perform after releasing the lock.
    pub(crate) fn install_ring(&mut self, community: CommunityId, ring: Vec<SealKey>) -> ControlEffects {
        let World {
            paths,
            communities,
            dms,
            conv_book,
            identity_changes,
            ..
        } = self;
        let Some(c) = communities.get_mut(&community) else {
            return ControlEffects::default();
        };
        c.set_ring(ring);
        let unlocked = c.take_unhandled(&[]);
        control_effects(community, c, paths, dms, conv_book, identity_changes, now_ms(), &unlocked)
    }
}

/// The receive loop of one community's topic, and the means to stop it.
pub(crate) struct ReceiverTask {
    pub handle: JoinHandle<TopicReceiver>,
    pub stop: oneshot::Sender<()>,
}

impl Engine {
    pub(crate) async fn call(&self, peer: DeviceId, req: &Request) -> Result<Response> {
        let reply = self.node().request(peer, self.request_bytes(req)?).await?;
        decode(&reply)
    }

    /// Verifies and stores events for `community`. Returns true if some of them still wait for
    /// parents. Events stamped further ahead of our clock than the allowed skew are dropped, so
    /// every stored event is one this clock has observed.
    pub(crate) fn ingest(&self, community: CommunityId, items: Vec<SignedEvent>) -> Result<bool> {
        let now = now_ms();
        let mut guard = self.write();
        let World {
            paths,
            communities,
            directory,
            dms,
            conv_book,
            identity_changes,
            ..
        } = &mut *guard;
        let mut emitted: Vec<EngineEvent> = Vec::new();
        let mut valid = Vec::with_capacity(items.len());
        // A storage member is outside the group, so it keeps a member-sealed control event only if
        // its tag checks out. An event whose epoch has no write key yet waits for one.
        let storage_here = communities.get(&community).is_some_and(|c| {
            c.state()
                .members
                .get(&self.inner.me)
                .is_some_and(|m| m.storage && !m.left)
        });
        let writes = communities
            .get(&community)
            .map(|c| c.epochs.write_ring())
            .unwrap_or_default();
        let mut missing_writes = false;
        for se in items {
            let hlc = se.event.body.hlc;
            // A cert seen before was already checked, so only the event signature is left to verify.
            let checked = if directory.knows(&se.cert) {
                se.event.verify(&se.cert)
            } else {
                se.verify()
            };
            if checked.is_err() || se.event.body.community != community {
                debug!(community = %community, "dropping event that failed verification");
                continue;
            }
            if hlc.wall_ms > now.saturating_add(MAX_CLOCK_SKEW_MS) {
                debug!(community = %community, "dropping event stamped too far ahead of our clock");
                continue;
            }
            if storage_here && dissent_events::is_member_sealed(&se.event.body.payload) {
                match dissent_events::verify_member_tag(&writes, &se.event.body) {
                    Ok(()) => {}
                    Err(dissent_events::OpenError::NoKey) => {
                        missing_writes = true;
                        continue;
                    }
                    Err(_) => {
                        debug!(community = %community, "dropping a member-sealed event with a bad tag");
                        continue;
                    }
                }
            }
            let path = paths.identity_changes();
            if let Some(ev) = identity_watch::learn_device(
                directory,
                dms,
                conv_book,
                identity_changes,
                &path,
                &se.cert,
                now,
            ) {
                emitted.push(ev);
            }
            self.observe(hlc);
            valid.push(se);
        }
        let c = communities
            .get_mut(&community)
            .ok_or_else(|| not_found(&community))?;
        let batch = c.ingest(valid);
        if let Err(e) = append_records(&paths.events_log(&community), &batch.stored) {
            warn!(community = %community, error = %e, "could not persist events");
        }
        let fresh = c.take_unhandled(&batch.stored);
        for se in &fresh {
            if matches!(
                se.event.body.payload,
                Payload::Message { .. }
                    | Payload::Sealed {
                        kind: SealedKind::Message | SealedKind::HistoryMessage,
                        ..
                    }
            ) {
                let target = TypingTarget::Channel {
                    community,
                    channel: se.event.body.channel,
                };
                self.clear_typer(target, se.event.body.author);
            }
        }
        let mut fx = control_effects(
            community,
            c,
            paths,
            dms,
            conv_book,
            identity_changes,
            now,
            &fresh,
        );
        fx.emitted.splice(0..0, emitted);
        drop(guard);
        if missing_writes {
            let engine = self.clone();
            self.spawn_task(async move { engine.fetch_write_keys(community).await });
        }
        self.retry_deferred_commits(community);
        self.perform_effects(fx);
        self.unseal_pending(community);
        Ok(batch.stalled)
    }

    /// Performs what accepted events asked for, after the lock is released. A `SetRelays` adopts
    /// its relays, and every notice is emitted.
    pub(crate) fn perform_effects(&self, fx: ControlEffects) {
        for relays in fx.relay_sets {
            let engine = self.clone();
            self.spawn_task(async move { engine.adopt_relays(&relays).await });
        }
        if fx.renamed {
            self.after_rename();
        }
        for fetch in fx.pictures {
            if let Some(poster) = fetch.poster {
                self.fetch_picture(fetch.owner, poster, fetch.roots.clone());
            }
            match fetch.picture {
                Some(picture) => self.fetch_picture(fetch.owner, picture, fetch.roots),
                None => self.emit_owner(fetch.owner),
            }
        }
        for ev in fx.emitted {
            self.emit(ev);
        }
    }

    /// Handles one gossip message. The sender is the neighbour that relayed it, not the author.
    pub(crate) async fn on_gossip(
        &self,
        community: CommunityId,
        neighbour: DeviceId,
        bytes: &[u8],
    ) {
        match decode_gossip(bytes) {
            Ok(GossipMsg::Event(se)) => match self.ingest(community, vec![*se]) {
                Ok(true) => {
                    if let Err(e) = self.sync_with(community, neighbour).await {
                        debug!(community = %community, error = %e, "sync after gossip failed");
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    debug!(community = %community, error = %e, "could not store gossip event")
                }
            },
            Ok(GossipMsg::Commit(announce)) => {
                self.receive_commit(community, neighbour, *announce).await
            }
            Ok(GossipMsg::Addr(sa)) => {
                if let Err(e) = self.accept_addr(*sa) {
                    debug!(error = %e, "rejected gossiped address");
                }
            }
            Ok(GossipMsg::Presence(sp)) => {
                if let Err(e) = self.accept_presence(Some(community), *sp) {
                    debug!(community = %community, error = %e, "rejected presence beacon");
                }
            }
            Ok(GossipMsg::Typing(st)) => {
                if let Err(e) = self.accept_typing(Some(community), *st) {
                    debug!(community = %community, error = %e, "rejected typing signal");
                }
            }
            Err(e) => debug!(error = %e, "undecodable gossip message"),
        }
    }

    /// Serves one request from `conn_from`. Runs under the RPC handler.
    pub(crate) async fn handle_request(&self, conn_from: DeviceId, bytes: &[u8]) -> Response {
        let env: InEnvelope = match decode(bytes) {
            Ok(env) => env,
            Err(e) => return Response::Error(format!("bad request: {e}")),
        };
        if let Err(e) = self.check_request(&env, conn_from) {
            return Response::Error(e.to_string());
        }
        match env.req {
            Request::Genesis { community } => {
                let genesis = self
                    .read()
                    .communities
                    .get(&community)
                    .map(|c| c.genesis.clone());
                match genesis {
                    Some(genesis) => Response::Genesis {
                        genesis,
                        addrs: self.member_addrs(community),
                    },
                    None => Response::Error("unknown community".into()),
                }
            }
            Request::Reconcile {
                community,
                msg,
                from,
            } => {
                self.learn_peer(conn_from, from);
                match self.reconcile_reply(community, &msg) {
                    Ok(reply) => Response::Reconcile {
                        msg: Some(reply),
                        addrs: self.member_addrs(community),
                    },
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Fetch { community, ids } => Response::Events(self.events_for(community, &ids)),
            Request::FetchRaw { community, ids } => Response::EventsRaw(
                self.events_for(community, &ids).iter().map(|se| se.encode()).collect(),
            ),
            Request::MlsJoin { community, from } => {
                self.handle_mls_join(conn_from, community, from).await
            }
            Request::KeyPackages { .. } | Request::DmKeyPackage => match self.with_mls(|mls| mls.take_key_packages(1)) {
                Ok(packages) => Response::KeyPackages(packages),
                Err(e) => Response::Error(format!("no key packages: {e}")),
            },
            Request::MlsCommits { community, after } => {
                Response::MlsCommits(self.commits_after(community, after))
            }
            Request::Secret { community } => self.serve_secret(conn_from, community),
            Request::SecretHistory { community } => self.serve_secret_history(conn_from, community),
            Request::EpochHistory { community } => self.serve_epoch_history(conn_from, community),
            Request::WriteKeys { community } => self.serve_write_keys(conn_from, community),
            Request::MemberSealedSupport { .. } => Response::MemberSealedSupport(true),
            Request::InvitePreview {
                community,
                token,
                invite_id,
            } => self.answer_invite_preview(conn_from, community, token, invite_id),
            Request::InvitePreviewFull {
                community,
                token,
                invite_id,
            } => self.answer_invite_preview_full(conn_from, community, token, invite_id),
            Request::Friends => self.serve_friends(conn_from),
            Request::HistoryKeys { community } => self.serve_history_keys(conn_from, community),
            Request::HistorySupport => Response::HistorySupport(
                self.inner.history_support.load(std::sync::atomic::Ordering::Relaxed),
            ),
            Request::MlsCommit { community, epoch } => {
                Response::MlsCommit(self.commit_at(community, epoch))
            }
            Request::Push { community, events } => match self.ingest(community, events) {
                Ok(_) => Response::Ack,
                Err(e) => Response::Error(format!("could not store events: {e}")),
            },
            Request::Addr(sa) => match self.accept_addr(*sa) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid address: {e}")),
            },
            Request::Dm { dm, from } => self.receive_dm(conn_from, *dm, from),
            Request::Contact(signed) => self.receive_contact(*signed).await,
            Request::Conv(packet) => self.receive_conv(*packet).await,
            Request::Deposit(env) => self.receive_deposit(*env),
            Request::Collect(req) => self.receive_collect(*req),
            Request::AckEnvelopes(ack) => self.receive_ack(*ack),
            Request::DepositKeyPackages(body) => self.receive_key_packages(*body),
            Request::TakeKeyPackage { root } => self.take_key_package(root),
            Request::Call(msg) => Response::Call(self.handle_call(conn_from, *msg).await),
            Request::Guardians(set) => self.accept_guardians(*set),
            Request::Presence(sp) => match self.accept_presence(None, *sp) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid presence: {e}")),
            },
            Request::AddrSync { digest } => self.serve_addr_sync(conn_from, digest),
            Request::AddrPush { records } => self.accept_addr_push(conn_from, records),
            Request::AddrLookup { device } => self.serve_addr_lookup(conn_from, device),
            Request::Profile(sp) => match self.accept_profile(*sp) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid profile: {e}")),
            },
            Request::ProfileSync(sp) => self.answer_profile_sync(conn_from, *sp),
            Request::ProfileBanner(sb) => match self.accept_profile_banner(*sb) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid profile banner: {e}")),
            },
            Request::Typing(st) => match self.accept_typing(None, *st) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid typing: {e}")),
            },
        }
    }

    pub(crate) fn learn_peer(&self, conn_from: DeviceId, from: PeerAddr) {
        if from.id == conn_from {
            let mut guard = self.write();
            self.learn_address(&mut guard, from);
        }
    }

    /// Records an unsigned peer address and tells the node to dial it by id. Persists only on change.
    pub(crate) fn learn_address(&self, w: &mut World, addr: PeerAddr) {
        if let Some(node) = self.inner.node.get() {
            if let Err(e) = node.remember(&addr) {
                debug!(error = %e, "could not remember peer address");
            }
        }
        if w.addresses.learn(addr) {
            if let Err(e) = w.save_addresses() {
                warn!(error = %e, "could not persist peer addresses");
            }
        }
    }

    fn receive_dm(&self, conn_from: DeviceId, sd: SignedDm, from: PeerAddr) -> Response {
        if sd.verify().is_err() {
            return Response::Error("invalid direct message".into());
        }
        let peer: RootId = sd.dm.body.from;
        let device_notice: Option<EngineEvent>;
        let is_new = {
            let mut guard = self.write();
            let w = &mut *guard;
            if sd.dm.body.to != w.me {
                return Response::Error("message is not addressed to this user".into());
            }
            if w.contact_book.blocked.contains(&peer) {
                return Response::Ack;
            }
            if !crate::contacts::dm_allowed(w, &peer) {
                return crate::contacts::refusal_for(w, peer, "this user does not accept direct messages from you".into());
            }
            device_notice = identity_watch::learn_device(
                &mut w.directory,
                &w.dms,
                &w.conv_book,
                &mut w.identity_changes,
                &w.paths.identity_changes(),
                &sd.cert,
                now_ms(),
            );
            if from.id == conn_from {
                self.learn_address(w, from);
            }
            self.observe(sd.dm.body.hlc);
            let seen = w
                .dms
                .get(&peer)
                .is_some_and(|log| log.iter().any(|d| d.dm.id == sd.dm.id));
            if seen {
                false
            } else {
                if let Err(e) = append_records(&w.paths.dm_log(&peer), std::slice::from_ref(&sd)) {
                    return Response::Error(format!("could not store message: {e}"));
                }
                w.dms.entry(peer).or_default().push(sd);
                true
            }
        };
        self.clear_typer(TypingTarget::Dm(peer), peer);
        if let Some(ev) = device_notice {
            self.emit(ev);
        }
        if is_new {
            self.emit(EngineEvent::Dm {
                peer: peer.to_string(),
            });
        }
        Response::Ack
    }

    /// Tries each known device of `peer` until one acknowledges. Returns whether one did.
    pub(crate) async fn deliver(&self, peer: RootId, sd: &SignedDm) -> bool {
        let (devices, from) = {
            let w = self.read();
            (w.directory.devices_of(&peer), w.my_addr.clone())
        };
        let Some(from) = from else {
            return false;
        };
        let bytes = match self.request_bytes(&Request::Dm {
            dm: Box::new(sd.clone()),
            from,
        }) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        let node = self.node();
        for device in devices {
            let reply = match node.request(device, bytes.clone()).await {
                Ok(reply) => reply,
                Err(e) => {
                    debug!(device = %device, error = %e, "direct message not delivered to device");
                    continue;
                }
            };
            if matches!(decode::<Response>(&reply), Ok(Response::Ack)) {
                return true;
            }
        }
        false
    }

    /// Re-sends every outbox entry. Acknowledged entries are dropped.
    pub(crate) async fn retry_outbox(&self) {
        let pending: Vec<SignedDm> = self.read().outbox.clone();
        for sd in pending {
            let peer = sd.dm.body.to;
            if !self.deliver(peer, &sd).await {
                continue;
            }
            {
                let mut guard = self.write();
                let id = sd.dm.id;
                guard.outbox.retain(|d| d.dm.id != id);
                if let Err(e) = guard.save_outbox() {
                    warn!(error = %e, "could not persist outbox");
                }
            }
            self.emit(EngineEvent::Dm {
                peer: peer.to_string(),
            });
        }
    }

    /// Syncs each community with one member, so a missed gossip message still arrives.
    ///
    /// The member tried first rotates from round to round, so one offline device cannot hold up
    /// the others. A round makes at most `ANTI_ENTROPY_ATTEMPTS` tries. A device that just failed
    /// is skipped for `FAILED_PEER_BACKOFF`.
    pub(crate) async fn anti_entropy(&self) {
        let targets: Vec<(CommunityId, Vec<DeviceId>)> = {
            let w = self.read();
            w.communities
                .values()
                .map(|c| (c.id(), w.member_devices(c)))
                .collect()
        };
        for (community, devices) in targets {
            if devices.is_empty() {
                continue;
            }
            let start = self.next_sync_start(community, devices.len());
            let mut attempts = 0;
            for offset in 0..devices.len() {
                if attempts == ANTI_ENTROPY_ATTEMPTS {
                    break;
                }
                let device = devices[(start + offset) % devices.len()];
                if self.is_backing_off(device) {
                    continue;
                }
                attempts += 1;
                let outcome = self.sync_with(community, device).await;
                match outcome {
                    Ok(()) => {
                        self.clear_backoff(device);
                        break;
                    }
                    Err(e) => {
                        debug!(community = %community, device = %device, error = %e, "anti-entropy sync failed");
                        self.back_off(device);
                    }
                }
            }
        }
    }

    /// Returns where this community's rotation starts, and moves it on by one.
    fn next_sync_start(&self, community: CommunityId, members: usize) -> usize {
        let mut cursor = self
            .inner
            .sync_cursor
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let next = cursor.entry(community).or_insert(0);
        let start = *next % members;
        *next = next.wrapping_add(1);
        start
    }

    fn is_backing_off(&self, device: DeviceId) -> bool {
        self.inner
            .sync_backoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&device)
            .is_some_and(|until| Instant::now() < *until)
    }

    fn back_off(&self, device: DeviceId) {
        self.inner
            .sync_backoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device, Instant::now() + FAILED_PEER_BACKOFF);
    }

    fn clear_backoff(&self, device: DeviceId) {
        self.inner
            .sync_backoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&device);
    }
}

/// How often a receive loop refreshes its neighbour list while the topic is quiet.
const NEIGHBOUR_TICK: Duration = Duration::from_secs(2);
/// How often a community asks a neighbour for MLS commits it may have missed.
const COMMIT_CHECK_EVERY: Duration = Duration::from_secs(5);
/// Members tried in one anti-entropy round.
const ANTI_ENTROPY_ATTEMPTS: usize = 3;
/// How long a device that failed a sync is skipped.
const FAILED_PEER_BACKOFF: Duration = Duration::from_secs(20);

/// Feeds one community's gossip into the engine until the topic closes, the engine is dropped,
/// or `stop` fires. Returns the receiver, so the caller can leave the topic with it. Keeps the
/// engine's neighbour cache for this community up to date.
pub(crate) async fn receive_loop(
    weak: Weak<Inner>,
    community: CommunityId,
    mut rx: TopicReceiver,
    mut stop: oneshot::Receiver<()>,
) -> TopicReceiver {
    let mut tick = tokio::time::interval(NEIGHBOUR_TICK);
    let mut known: BTreeSet<DeviceId> = BTreeSet::new();
    let mut last_commit_check = Instant::now();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = tick.tick() => {
                // A commit announcement lost in transit would otherwise wait for the next message.
                // So ask one neighbour for commits we lack, every few seconds.
                let neighbour = known.iter().next().copied();
                if let (Some(neighbour), true) = (neighbour, last_commit_check.elapsed() >= COMMIT_CHECK_EVERY) {
                    last_commit_check = Instant::now();
                    if let Some(inner) = weak.upgrade() {
                        let engine = Engine { inner };
                        let task = engine.clone();
                        engine.spawn_task(async move {
                            task.mls_commit_check(community, neighbour).await;
                        });
                    }
                }
            }
            msg = rx.next() => {
                let Some(msg) = msg else {
                    break;
                };
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                Engine { inner }
                    .on_gossip(community, msg.from, &msg.bytes)
                    .await;
            }
        }
        // A neighbour that just appeared has not seen our address yet. Send it.
        let fresh = note_neighbours(&weak, community, &rx, &mut known);
        if !fresh.is_empty() {
            if let Some(inner) = weak.upgrade() {
                let engine = Engine { inner };
                engine.broadcast_own_addr(community).await;
                engine.broadcast_presence(community).await;
                // Gossip sent just before a neighbour joined does not reach it. Sync with it now.
                for device in fresh {
                    let task = engine.clone();
                    engine.spawn_task(async move {
                        let _ = task.sync_with(community, device).await;
                        task.peer_exchange(device).await;
                    });
                }
            }
        }
    }
    rx
}

/// Copies the topic's current direct neighbours into the engine's cache. Returns the ones that
/// were not there last time.
fn note_neighbours(
    weak: &Weak<Inner>,
    community: CommunityId,
    rx: &TopicReceiver,
    known: &mut BTreeSet<DeviceId>,
) -> Vec<DeviceId> {
    let now: BTreeSet<DeviceId> = rx.neighbors().into_iter().collect();
    let fresh: Vec<DeviceId> = now.difference(known).copied().collect();
    *known = now.clone();
    if let Some(inner) = weak.upgrade() {
        inner
            .neighbours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(community, now);
    }
    fresh
}

/// Periodic work: retry the outbox and anti-entropy sync.
pub(crate) async fn maintenance(weak: Weak<Inner>) {
    let mut tick = tokio::time::interval(RETRY_EVERY);
    // The first tick fires at once. Skip it so the first pass waits a full period.
    tick.tick().await;
    // Republish DHT records every 30 minutes, so they never age out of the DHT.
    const REPUBLISH_EVERY_ROUNDS: u32 = 60;
    // The user record: an hour at one pass per 30 s.
    const USER_REPUBLISH_ROUNDS: u32 = 120;
    // Ten minutes at one pass per 30 s.
    const ADDR_REBROADCAST_ROUNDS: u32 = 20;
    let mut rounds: u32 = 0;
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else {
            break;
        };
        let engine = Engine { inner };
        engine.retry_outbox().await;
        engine.retry_contact_outbox().await;
        engine.flush_conv_queues().await;
        if let Err(e) = engine.collect_inboxes().await {
            warn!(error = %e, "could not collect from inboxes");
        }
        if rounds.is_multiple_of(20) {
            if let Err(e) = engine.deposit_key_packages().await {
                warn!(error = %e, "could not deposit key packages");
            }
        }
        engine.refresh_lan_keys();
        engine.anti_entropy().await;
        engine.mls_maintenance().await;
        engine.flush_mls(false);
        rounds += 1;
        // Re-announce our address on every community topic every 10 minutes, so late joiners
        // learn it from gossip too.
        if rounds.is_multiple_of(ADDR_REBROADCAST_ROUNDS) {
            engine.rebroadcast_own_addr().await;
            // Re-sign our record too, so it never reaches its expiry while we run.
            engine.refresh_own_record_now().await;
        }
        // The first publish can run before the DHT node has bootstrapped. Pkarr then stores the
        // record on too few nodes, yet still reports success. So publish once more on the first
        // pass, and retry while a publish fails.
        let retry = !engine
            .inner
            .records_current
            .load(std::sync::atomic::Ordering::Relaxed);
        if retry || rounds == 1 || rounds.is_multiple_of(REPUBLISH_EVERY_ROUNDS) {
            engine.publish_records().await;
        }
        // The user record is republished hourly, so it stays on the DHT and follows device changes.
        if rounds == 1 || rounds.is_multiple_of(USER_REPUBLISH_ROUNDS) {
            engine.publish_user_record().await;
        }
    }
}

#[cfg(test)]
mod tests;
