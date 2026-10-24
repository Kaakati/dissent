//! Network side: inbound gossip and requests, direct message delivery, and background work.
//! No lock is held across a network await.

use crate::engine::{now_ms, not_found, Engine, Inner, MAX_CLOCK_SKEW_MS, RETRY_EVERY};
use crate::error::Result;
use crate::messages::event_for;
use crate::rpc::{decode, encode, Request, Response};
use crate::store::append_records;
use crate::views::EngineEvent;
use crate::wire::{decode_gossip, GossipMsg};
use crate::world::World;
use dissent_events::{SignedDm, SignedEvent};
use dissent_identity::{CommunityId, DeviceId, RootId};
use dissent_net::{PeerAddr, TopicReceiver};
use std::collections::BTreeSet;
use std::sync::{PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// The receive loop of one community's topic, and the means to stop it.
pub(crate) struct ReceiverTask {
    pub handle: JoinHandle<TopicReceiver>,
    pub stop: oneshot::Sender<()>,
}

impl Engine {
    pub(crate) async fn call(&self, peer: DeviceId, req: &Request) -> Result<Response> {
        let reply = self.node().request(peer, encode(req)?).await?;
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
            ..
        } = &mut *guard;
        let mut valid = Vec::with_capacity(items.len());
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
            directory.learn(&se.cert);
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
        let mut emitted: Vec<EngineEvent> = Vec::new();
        for se in &batch.stored {
            if c.is_rejected(&se.event.id) {
                continue;
            }
            let ev = event_for(&community, &se.event);
            if !emitted.contains(&ev) {
                emitted.push(ev);
            }
        }
        drop(guard);
        for ev in emitted {
            self.emit(ev);
        }
        Ok(batch.stalled)
    }

    /// Handles one gossip message. The sender is the neighbour that relayed it, not the author.
    pub(crate) async fn on_gossip(&self, community: CommunityId, neighbour: DeviceId, bytes: &[u8]) {
        match decode_gossip(bytes) {
            Ok(GossipMsg::Event(se)) => match self.ingest(community, vec![*se]) {
                Ok(true) => {
                    if let Err(e) = self.sync_with(community, neighbour).await {
                        debug!(community = %community, error = %e, "sync after gossip failed");
                    }
                }
                Ok(false) => {}
                Err(e) => debug!(community = %community, error = %e, "could not store gossip event"),
            },
            Ok(GossipMsg::Addr(sa)) => {
                if let Err(e) = self.accept_addr(*sa) {
                    debug!(error = %e, "rejected gossiped address");
                }
            }
            Err(e) => debug!(error = %e, "undecodable gossip message"),
        }
    }

    /// Serves one request from `conn_from`. Runs under the RPC handler.
    pub(crate) fn handle_request(&self, conn_from: DeviceId, bytes: &[u8]) -> Response {
        let req: Request = match decode(bytes) {
            Ok(req) => req,
            Err(e) => return Response::Error(format!("bad request: {e}")),
        };
        match req {
            Request::Genesis { community } => match self.read().communities.get(&community) {
                Some(c) => Response::Genesis(c.genesis.clone()),
                None => Response::Error("unknown community".into()),
            },
            Request::Reconcile {
                community,
                msg,
                from,
            } => {
                self.learn_peer(conn_from, from);
                match self.reconcile_reply(community, &msg) {
                    Ok(reply) => Response::Reconcile { msg: Some(reply) },
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Fetch { community, ids } => Response::Events(self.events_for(community, &ids)),
            Request::Push { community, events } => match self.ingest(community, events) {
                Ok(_) => Response::Ack,
                Err(e) => Response::Error(format!("could not store events: {e}")),
            },
            Request::Addr(sa) => match self.accept_addr(*sa) {
                Ok(()) => Response::Ack,
                Err(e) => Response::Error(format!("invalid address: {e}")),
            },
            Request::Dm { dm, from } => self.receive_dm(conn_from, *dm, from),
        }
    }

    fn learn_peer(&self, conn_from: DeviceId, from: PeerAddr) {
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
        let is_new = {
            let mut guard = self.write();
            let w = &mut *guard;
            if sd.dm.body.to != w.me {
                return Response::Error("message is not addressed to this user".into());
            }
            w.directory.learn(&sd.cert);
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
        let bytes = match encode(&Request::Dm {
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
    publish_neighbours(&weak, community, &rx);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = tick.tick() => publish_neighbours(&weak, community, &rx),
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
                publish_neighbours(&weak, community, &rx);
            }
        }
    }
    rx
}

/// Copies the topic's current direct neighbours into the engine's cache.
fn publish_neighbours(weak: &Weak<Inner>, community: CommunityId, rx: &TopicReceiver) {
    if let Some(inner) = weak.upgrade() {
        let devices: BTreeSet<DeviceId> = rx.neighbors().into_iter().collect();
        inner
            .neighbours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(community, devices);
    }
}

/// Periodic work: retry the outbox and anti-entropy sync.
pub(crate) async fn maintenance(weak: Weak<Inner>) {
    let mut tick = tokio::time::interval(RETRY_EVERY);
    // The first tick fires at once. Skip it so the first pass waits a full period.
    tick.tick().await;
    // Republish DHT records every 30 minutes, so they never age out of the DHT.
    const REPUBLISH_EVERY_ROUNDS: u32 = 60;
    let mut rounds: u32 = 0;
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else {
            break;
        };
        let engine = Engine { inner };
        engine.retry_outbox().await;
        engine.anti_entropy().await;
        rounds += 1;
        // The first publish can run before the DHT node has bootstrapped. Pkarr then stores the
        // record on too few nodes, yet still reports success. So publish once more on the first
        // pass, and retry while a publish fails.
        let retry = !engine.inner.records_current.load(std::sync::atomic::Ordering::Relaxed);
        if retry || rounds == 1 || rounds.is_multiple_of(REPUBLISH_EVERY_ROUNDS) {
            engine.publish_records().await;
        }
    }
}

#[cfg(test)]
mod tests;
