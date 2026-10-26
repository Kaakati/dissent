//! Set reconciliation with negentropy. The initiator learns which events each side lacks, then
//! fetches and pushes them in pages. Used for joins, gap fills and anti-entropy.

use crate::engine::{not_found, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use dissent_events::{EventId, SignedEvent};
use dissent_identity::{CommunityId, DeviceId};
use crate::wire::SignedAddr;
use dissent_net::PeerAddr;
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use std::collections::HashSet;

/// Largest negentropy message in one round, in bytes.
const FRAME_LIMIT: u64 = 256 * 1024;
/// Most ids in one fetch or one push.
pub(crate) const PAGE_ITEMS: usize = 500;
/// Most encoded event bytes in one fetch reply or push. Keeps replies inside the frame limit.
const PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Rounds allowed before a reconciliation is abandoned.
const MAX_ROUNDS: usize = 10_000;

fn neg_err(e: negentropy::Error) -> CoreError {
    CoreError::Invalid(format!("reconciliation failed: {e}"))
}

fn unexpected(resp: &Response) -> CoreError {
    CoreError::Invalid(format!("unexpected reply: {resp:?}"))
}

/// The outcome of a reconciliation: what each side lacks, and the members' signed addresses the peer passed on.
pub(crate) struct Reconciled {
    pub need: Vec<EventId>,
    pub have: Vec<EventId>,
    pub addrs: Vec<SignedAddr>,
}

/// Sealed negentropy storage over `(wall time, event id)` pairs.
pub(crate) fn index(items: &[(u64, EventId)]) -> Result<NegentropyStorageVector> {
    let mut storage = NegentropyStorageVector::with_capacity(items.len());
    for (wall_ms, id) in items {
        storage
            .insert(*wall_ms, Id::from_byte_array(id.0))
            .map_err(neg_err)?;
    }
    storage.seal().map_err(neg_err)?;
    Ok(storage)
}

/// Splits events into pages by count and encoded size. An event larger than a page gets a page.
fn pages(events: Vec<SignedEvent>) -> Vec<Vec<SignedEvent>> {
    let mut out: Vec<Vec<SignedEvent>> = Vec::new();
    let mut current: Vec<SignedEvent> = Vec::new();
    let mut bytes = 0usize;
    for se in events {
        let size = se.encode().len();
        if !current.is_empty() && (current.len() >= PAGE_ITEMS || bytes + size > PAGE_BYTES) {
            out.push(std::mem::take(&mut current));
            bytes = 0;
        }
        bytes += size;
        current.push(se);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

impl Engine {
    /// Brings `community` up to date with `peer` in both directions.
    pub(crate) async fn sync_with(&self, community: CommunityId, peer: DeviceId) -> Result<()> {
        let (items, from) = self.sync_inputs(community)?;
        let Reconciled { need, have, addrs } = self.reconcile(community, peer, items, from).await?;
        self.fetch_into(community, peer, need, |events| {
            self.ingest(community, events).map(|_| ())
        })
        .await?;
        self.learn_member_addrs(community, addrs);
        for chunk in have.chunks(PAGE_ITEMS) {
            let events = self
                .read()
                .communities
                .get(&community)
                .map(|c| c.signed_for(chunk))
                .unwrap_or_default();
            for page in pages(events) {
                self.push(community, peer, page).await?;
            }
        }
        self.mls_catch_up(community, peer).await;
        Ok(())
    }

    fn sync_inputs(&self, community: CommunityId) -> Result<(Vec<(u64, EventId)>, PeerAddr)> {
        let w = self.read();
        let c = w
            .communities
            .get(&community)
            .ok_or_else(|| not_found(&community))?;
        let from = w
            .my_addr
            .clone()
            .ok_or_else(|| CoreError::Invalid("endpoint address is not ready".into()))?;
        Ok((c.index_items(), from))
    }

    /// Runs reconciliation rounds with `peer`. Returns `(need, have)`: the ids the peer holds that
    /// we lack, and the ids we hold that the peer lacks.
    pub(crate) async fn reconcile(
        &self,
        community: CommunityId,
        peer: DeviceId,
        items: Vec<(u64, EventId)>,
        from: PeerAddr,
    ) -> Result<Reconciled> {
        let mut neg = Negentropy::owned(index(&items)?, FRAME_LIMIT).map_err(neg_err)?;
        let mut msg = neg.initiate().map_err(neg_err)?;
        let mut have_ids: Vec<Id> = Vec::new();
        let mut need_ids: Vec<Id> = Vec::new();
        let mut addrs: Vec<SignedAddr> = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let req = Request::Reconcile {
                community,
                msg,
                from: from.clone(),
            };
            let reply = match self.call(peer, &req).await? {
                Response::Reconcile {
                    msg: Some(reply),
                    addrs: more,
                } => {
                    addrs.extend(more);
                    reply
                }
                Response::Reconcile { msg: None, .. } => {
                    return Err(CoreError::Invalid("peer sent no reconciliation reply".into()))
                }
                Response::Error(reason) => {
                    return Err(CoreError::Invalid(format!("peer refused sync: {reason}")))
                }
                other => return Err(unexpected(&other)),
            };
            match neg
                .reconcile_with_ids(&reply, &mut have_ids, &mut need_ids)
                .map_err(neg_err)?
            {
                Some(next) => msg = next,
                None => {
                    return Ok(Reconciled {
                        need: to_event_ids(need_ids),
                        have: to_event_ids(have_ids),
                        addrs,
                    })
                }
            }
        }
        Err(CoreError::Invalid("reconciliation did not converge".into()))
    }

    /// Fetches `ids` from `peer` page by page, handing each batch to `sink`. An id the peer
    /// does not hold is dropped after one empty reply, so the loop always ends.
    pub(crate) async fn fetch_into(
        &self,
        community: CommunityId,
        peer: DeviceId,
        ids: Vec<EventId>,
        mut sink: impl FnMut(Vec<SignedEvent>) -> Result<()>,
    ) -> Result<()> {
        let mut pending = ids;
        while !pending.is_empty() {
            let chunk: Vec<EventId> = pending.iter().take(PAGE_ITEMS).copied().collect();
            let events = self.fetch_page(community, peer, &chunk).await?;
            if events.is_empty() {
                // The peer holds none of this chunk.
                pending.drain(..chunk.len());
                continue;
            }
            let got: HashSet<EventId> = events.iter().map(|se| se.event.id).collect();
            pending.retain(|id| !got.contains(id));
            sink(events)?;
        }
        Ok(())
    }

    /// One page of events. Asks with `FetchRaw`, so an event this build cannot decode is skipped rather
    /// than failing the page. A peer that predates `FetchRaw` gets `Fetch`.
    async fn fetch_page(&self, community: CommunityId, peer: DeviceId, ids: &[EventId]) -> Result<Vec<SignedEvent>> {
        let raw = Request::FetchRaw {
            community,
            ids: ids.to_vec(),
        };
        match self.call(peer, &raw).await {
            Ok(Response::EventsRaw(blobs)) => Ok(decode_events(&blobs)),
            Ok(Response::Error(reason)) => Err(CoreError::Invalid(format!("peer refused fetch: {reason}"))),
            _ => {
                let req = Request::Fetch {
                    community,
                    ids: ids.to_vec(),
                };
                match self.call(peer, &req).await? {
                    Response::Events(events) => Ok(events),
                    Response::Error(reason) => Err(CoreError::Invalid(format!("peer refused fetch: {reason}"))),
                    other => Err(unexpected(&other)),
                }
            }
        }
    }

    /// Sends events to `peer` for it to verify and store.
    pub(crate) async fn push(&self, community: CommunityId, peer: DeviceId, events: Vec<SignedEvent>) -> Result<()> {
        match self.call(peer, &Request::Push { community, events }).await? {
            Response::Ack => Ok(()),
            Response::Error(reason) => {
                Err(CoreError::Invalid(format!("peer refused push: {reason}")))
            }
            other => Err(unexpected(&other)),
        }
    }

    /// Answers one reconciliation round from this node's own events.
    pub(crate) fn reconcile_reply(&self, community: CommunityId, msg: &[u8]) -> Result<Vec<u8>> {
        let items = {
            let w = self.read();
            w.communities
                .get(&community)
                .ok_or_else(|| not_found(&community))?
                .index_items()
        };
        let storage = index(&items)?;
        let mut neg = Negentropy::borrowed(&storage, FRAME_LIMIT).map_err(neg_err)?;
        neg.reconcile(msg).map_err(neg_err)
    }

    /// The stored events for `ids`, in causal order, capped by count and size.
    pub(crate) fn events_for(&self, community: CommunityId, ids: &[EventId]) -> Vec<SignedEvent> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let wanted = &ids[..ids.len().min(PAGE_ITEMS)];
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for se in c.signed_for(wanted) {
            let size = se.encode().len();
            if !out.is_empty() && bytes + size > PAGE_BYTES {
                break;
            }
            bytes += size;
            out.push(se);
        }
        out
    }
}

fn to_event_ids(ids: Vec<Id>) -> Vec<EventId> {
    ids.into_iter().map(|id| EventId(id.to_bytes())).collect()
}

/// Decodes event bytes one at a time. An event this build cannot decode is logged and skipped, so an
/// event from a newer build does not fail the page that carries it.
pub(crate) fn decode_events(blobs: &[Vec<u8>]) -> Vec<SignedEvent> {
    blobs
        .iter()
        .filter_map(|bytes| match SignedEvent::decode(bytes) {
            Ok(se) => Some(se),
            Err(e) => {
                tracing::warn!(error = %e, "skipped an event this build cannot decode");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod decode_tests {
    use super::*;
    use dissent_events::{ChannelId, Event, EventBody, Hlc, Payload};
    use dissent_identity::{DeviceKey, RootKey};

    #[test]
    fn a_batch_with_an_event_this_build_cannot_decode_keeps_the_rest() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        let body = EventBody {
            community: CommunityId([7; 32]),
            channel: ChannelId::CONTROL,
            author: root.id(),
            device: device.id(),
            parents: vec![],
            hlc: Hlc { wall_ms: 1, counter: 0 },
            payload: Payload::Leave,
        };
        let good = SignedEvent {
            event: Event::sign(body, &device),
            cert,
        };
        // A blob from a newer build: not a SignedEvent this build can read.
        let blobs = vec![vec![0xff, 0xff, 0xff], good.encode()];
        let decoded = decode_events(&blobs);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].event.id, good.event.id);
        assert!(decode_events(&[vec![0xff]]).is_empty());
    }
}
