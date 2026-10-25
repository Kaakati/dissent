//! Peer exchange: how devices share address records, so a reconnect rarely needs the Mainline DHT.
//!
//! Who may see what is decided here, and only here:
//!
//! - A device is in scope for us when it is us, a contact, or a member of a community we are in.
//!   We never exchange records with anyone else, and we never learn a record of anyone else.
//! - A record of device X may go to requester R only when X is us or X's root shares a community
//!   with R. A contact who shares no community with X does not get X's address from us.
//! - An address lookup for X is answered only to a scoped requester, and only when the record is
//!   visible to it. Any other request gets the same empty answer, so a stranger cannot tell
//!   whether X exists.
//!
//! Records reach a device three ways: pushed when our address changes, exchanged on connecting to a
//! new gossip neighbour (a digest diff sends only what is newer), and fetched by a live lookup
//! when a dial has no working address.

use std::collections::BTreeSet;

use dissent_identity::{DeviceId, RootId};
use dissent_net::{PEER, SEEDER};
use tracing::{debug, warn};

use crate::addr_record::{AddrRecord, MAX_RECORDS_PER_MESSAGE, RECORD_TTL_MS};
use crate::engine::{Engine, now_ms};
use crate::error::{CoreError, Result};
use crate::identity_watch;
use crate::rpc::{Request, Response};
use crate::world::World;

/// Most scoped devices we ask in one live lookup.
pub(crate) const LOOKUP_FANOUT: usize = 4;

/// Whether `root` may see us and we may see it: our own root, a contact, or a member of a
/// community we are in.
pub(crate) fn in_scope(w: &World, root: &RootId) -> bool {
    *root == w.me || w.is_contact(root) || w.communities.values().any(|c| c.is_member(root))
}

/// Whether `a` and `b` are both members of one community we are in.
pub(crate) fn co_members(w: &World, a: &RootId, b: &RootId) -> bool {
    w.communities
        .values()
        .any(|c| c.is_member(a) && c.is_member(b))
}

/// Whether `record` may be shared with `requester`: it is ours, or its root shares a community
/// with the requester.
pub(crate) fn visible_to(w: &World, requester: &RootId, record: &AddrRecord) -> bool {
    let root = record.root();
    root == w.me || co_members(w, requester, &root)
}

/// Every device we may exchange records with: members of our communities and devices of contacts.
pub(crate) fn scoped_devices(w: &World) -> BTreeSet<DeviceId> {
    let mut out = BTreeSet::new();
    for c in w.communities.values() {
        out.extend(w.member_devices(c));
    }
    for card in &w.contacts {
        out.extend(w.directory.devices_of(&card.cert.root));
    }
    out.remove(&w.device);
    out
}

/// The `(device, seq)` pairs we may share with `requester`, our own record included.
fn digest_for(w: &World, requester: &RootId, now: u64) -> Vec<(DeviceId, u64)> {
    let mut out = w
        .addr_cache
        .digest(now, |r| visible_to(w, requester, r));
    if let Some(own) = w.own_record.as_ref().filter(|r| r.expires_ms > now) {
        out.push((own.device(), own.seq));
    }
    out
}

/// The records we may share with `requester` that are newer than the ones in `theirs`, our own
/// record first.
fn records_for(w: &World, requester: &RootId, theirs: &[(DeviceId, u64)], now: u64) -> Vec<AddrRecord> {
    let mut out = w
        .addr_cache
        .newer_than(theirs, now, |r| visible_to(w, requester, r));
    if let Some(own) = w.own_record.as_ref().filter(|r| r.expires_ms > now) {
        let known = theirs
            .iter()
            .find(|(device, _)| *device == own.device())
            .map(|(_, seq)| *seq);
        if known.is_none_or(|seq| seq < own.seq) {
            out.insert(0, own.clone());
        }
    }
    out.truncate(MAX_RECORDS_PER_MESSAGE);
    out
}

/// A record a device answered a lookup with, and whether that device runs as a seeder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrAnswer {
    pub addr: dissent_net::PeerAddr,
    pub seeder: bool,
}

impl Engine {
    /// Our address as the engine holds it now. Peers learn it through peer exchange.
    pub fn my_address(&self) -> Option<dissent_net::PeerAddr> {
        self.read().my_addr.clone()
    }

    /// The address we hold for `device` from its record, if any is held and not expired.
    pub fn known_address(&self, device: DeviceId) -> Option<dissent_net::PeerAddr> {
        let now = now_ms();
        self.write().addr_cache.get(&device, now).map(|r| r.addr)
    }

    /// Asks `via` for the address record of `device`, dialling `via` at the address given. The
    /// answer is checked before it is returned. `Ok(None)` means `via` holds no record of `device`
    /// that it may share with us. A refusal, for example to a device outside our groups, is an error.
    pub async fn lookup_addr(&self, via: &dissent_net::PeerAddr, device: DeviceId) -> Result<Option<AddrAnswer>> {
        self.node().remember(via)?;
        match self.call(via.id, &Request::AddrLookup { device }).await? {
            Response::AddrLookup { record: Some(record), seeder } => {
                record.verify(now_ms())?;
                if record.device() != device {
                    return Err(CoreError::Invalid("answer is for another device".into()));
                }
                Ok(Some(AddrAnswer { addr: record.addr, seeder }))
            }
            Response::AddrLookup { record: None, .. } => Ok(None),
            Response::Refused(reason) => Err(CoreError::Invalid(format!("refused: {reason}"))),
            other => Err(CoreError::Invalid(format!("unexpected reply: {other:?}"))),
        }
    }

    /// Signs a record of `addr` for this device. A new sequence number each time, so the newest
    /// record always wins.
    pub(crate) fn sign_own_record(&self, addr: dissent_net::PeerAddr) -> Result<AddrRecord> {
        let now = now_ms();
        let mut guard = self.write();
        let seq = now.max(guard.own_record.as_ref().map_or(0, |r| r.seq).saturating_add(1));
        let record = AddrRecord::sign(
            addr,
            self.inner.identity.cert.clone(),
            seq,
            now.saturating_add(RECORD_TTL_MS),
            &self.inner.identity.device,
        )?;
        guard.own_record = Some(record.clone());
        Ok(record)
    }

    /// Re-signs our record for `addr` and pushes it to every scoped device. Called when our
    /// address changes, and again every maintenance cycle so the record never expires while we run.
    pub(crate) async fn refresh_own_record(&self, addr: dissent_net::PeerAddr) {
        if let Err(e) = self.sign_own_record(addr) {
            warn!(error = %e, "could not sign our address record");
            return;
        }
        self.push_own_record().await;
    }

    /// Re-signs the record for the address we currently hold.
    pub(crate) async fn refresh_own_record_now(&self) {
        let addr = self.read().my_addr.clone();
        if let Some(addr) = addr {
            self.refresh_own_record(addr).await;
        }
    }

    /// Sends our record to every scoped device, one request each. Delivery is best effort.
    pub(crate) async fn push_own_record(&self) {
        let (record, targets) = {
            let w = self.read();
            (w.own_record.clone(), scoped_devices(&w))
        };
        let Some(record) = record else {
            return;
        };
        for device in targets {
            let engine = self.clone();
            let record = record.clone();
            self.spawn_task(async move {
                let req = Request::AddrPush {
                    records: vec![record],
                };
                match engine.call(device, &req).await {
                    Ok(Response::Ack) => {}
                    Ok(other) => debug!(device = %device, reply = ?other, "record push not taken"),
                    Err(e) => debug!(device = %device, error = %e, "record not pushed"),
                }
            });
        }
    }

    /// Exchanges records with a device that just became a gossip neighbour. We send our digest,
    /// take the records it has that we lack, and send back the ones it lacks.
    pub(crate) async fn peer_exchange(&self, device: DeviceId) {
        let (root, digest) = {
            let w = self.read();
            let Some(root) = w.directory.root_of(&device) else {
                return;
            };
            if !in_scope(&w, &root) {
                return;
            }
            (root, digest_for(&w, &root, now_ms()))
        };
        let reply = match self.call(device, &Request::AddrSync { digest }).await {
            Ok(reply) => reply,
            Err(e) => {
                debug!(device = %device, error = %e, "record exchange failed");
                return;
            }
        };
        let Response::AddrSync { digest: theirs, records } = reply else {
            debug!(device = %device, "record exchange refused");
            return;
        };
        for record in records {
            if let Err(e) = self.learn_record(record) {
                debug!(device = %device, error = %e, "record not kept");
            }
        }
        let push = {
            let w = self.read();
            records_for(&w, &root, &theirs, now_ms())
        };
        if push.is_empty() {
            return;
        }
        if let Err(e) = self.call(device, &Request::AddrPush { records: push }).await {
            debug!(device = %device, error = %e, "our records not pushed back");
        }
    }

    /// Verifies a record and keeps it if its device is in scope and the record is newer than the
    /// one held. Also teaches the directory the device's cert, and remembers its address so the
    /// endpoint can dial it. Returns whether the record was kept.
    pub(crate) fn learn_record(&self, record: AddrRecord) -> Result<bool> {
        let now = now_ms();
        record.verify(now)?;
        if record.device() == self.inner.identity.device_id() {
            return Ok(false);
        }
        let notice = {
            let mut guard = self.write();
            let w = &mut *guard;
            if !in_scope(w, &record.root()) {
                return Err(CoreError::Invalid("record is from outside our scope".into()));
            }
            let notice = identity_watch::learn_device(
                &mut w.directory,
                &w.dms,
                &w.conv_book,
                &mut w.identity_changes,
                &w.paths.identity_changes(),
                &record.cert,
                now,
            );
            let addr = record.addr.clone();
            w.addr_cache.learn(record, now)?;
            if let Some(node) = self.inner.node.get() {
                if let Err(e) = node.remember(&addr) {
                    debug!(error = %e, "could not remember a peer record");
                }
            }
            notice
        };
        if let Some(ev) = notice {
            self.emit(ev);
        }
        Ok(true)
    }

    /// Answers `AddrSync`: our digest for the requester, and the records it lacks.
    pub(crate) fn serve_addr_sync(&self, from: DeviceId, digest: Vec<(DeviceId, u64)>) -> Response {
        let w = self.read();
        let Some(root) = w.directory.root_of(&from) else {
            return Response::Refused("unknown device".into());
        };
        if !in_scope(&w, &root) {
            return Response::Refused("not in scope".into());
        }
        let now = now_ms();
        Response::AddrSync {
            digest: digest_for(&w, &root, now),
            records: records_for(&w, &root, &digest, now),
        }
    }

    /// Answers `AddrPush`: keeps each record the sender may share, at most once per push gap.
    pub(crate) fn accept_addr_push(&self, from: DeviceId, records: Vec<AddrRecord>) -> Response {
        let now = now_ms();
        {
            let mut guard = self.write();
            let Some(root) = guard.directory.root_of(&from) else {
                return Response::Refused("unknown device".into());
            };
            if !in_scope(&guard, &root) {
                return Response::Refused("not in scope".into());
            }
            if records.len() > MAX_RECORDS_PER_MESSAGE {
                return Response::Refused("too many records".into());
            }
            if !guard.push_limiter.admit(from, now) {
                return Response::Refused("pushed too often".into());
            }
        }
        for record in records {
            if let Err(e) = self.learn_record(record) {
                debug!(device = %from, error = %e, "pushed record not kept");
            }
        }
        Response::Ack
    }

    /// Answers `AddrLookup`: the record of `device` if the requester may see it. Anything else
    /// gets the same empty answer, so the reply does not show whether the device exists.
    pub(crate) fn serve_addr_lookup(&self, from: DeviceId, device: DeviceId) -> Response {
        let now = now_ms();
        let mut w = self.write();
        let Some(root) = w.directory.root_of(&from) else {
            return Response::Refused("unknown device".into());
        };
        if !in_scope(&w, &root) {
            return Response::Refused("not in scope".into());
        }
        let found = if device == self.inner.identity.device_id() {
            w.own_record.clone()
        } else {
            w.addr_cache.get(&device, now)
        };
        let record = found.filter(|r| r.expires_ms > now && visible_to(&w, &root, r));
        let seeder = self.inner.node.get().is_some_and(|n| n.serves_lookups());
        Response::AddrLookup { record, seeder }
    }

    /// The devices a live lookup for `device` may ask: gossip neighbours we are connected to, that
    /// are in scope, and that share a community with `device`. Only people who already share a group
    /// with the device learn that someone is looking for it. A dial to a connected device uses its
    /// open path and triggers no address lookup, so the asks cannot loop back into the lookup.
    pub(crate) fn lookup_targets(&self, device: DeviceId) -> Vec<DeviceId> {
        let neighbours: BTreeSet<DeviceId> = self
            .inner
            .neighbours
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .flatten()
            .copied()
            .collect();
        let w = self.read();
        let Some(target) = w.directory.root_of(&device) else {
            return Vec::new();
        };
        let scope = scoped_devices(&w);
        neighbours
            .into_iter()
            .filter(|d| *d != device && scope.contains(d))
            .filter(|d| {
                w.directory
                    .root_of(d)
                    .is_some_and(|root| co_members(&w, &root, &target) || root == target)
            })
            .take(LOOKUP_FANOUT)
            .collect()
    }

    /// Asks one scoped device for the record of `device`, and offers a verified answer to the
    /// dial through `tx`. Labels it `seeder` when the answerer runs as one.
    pub(crate) async fn ask_for_record(
        &self,
        peer: DeviceId,
        device: DeviceId,
        tx: tokio::sync::mpsc::UnboundedSender<std::result::Result<iroh::address_lookup::Item, iroh::address_lookup::Error>>,
    ) {
        let reply = self.call(peer, &Request::AddrLookup { device }).await;
        let Ok(Response::AddrLookup {
            record: Some(record),
            seeder,
        }) = reply
        else {
            return;
        };
        if record.device() != device || record.verify(now_ms()).is_err() {
            return;
        }
        let provenance = if seeder { SEEDER } else { PEER };
        if let Ok(addr) = record.addr.to_iroh() {
            let info = iroh::address_lookup::EndpointInfo::from(addr);
            let _ = tx.send(Ok(iroh::address_lookup::Item::new(info, provenance, None)));
        }
        if let Err(e) = self.learn_record(record) {
            debug!(device = %device, error = %e, "looked-up record not kept");
        }
    }

    /// Re-reads the community secrets and gives the node their LAN keys. Called at start and on
    /// every maintenance cycle, so a new community or rotation takes effect within a cycle.
    pub(crate) fn refresh_lan_keys(&self) {
        let communities: Vec<dissent_identity::CommunityId> = self.read().communities.keys().copied().collect();
        let keys: Vec<[u8; 32]> = communities
            .iter()
            .filter_map(|c| self.access_secret(*c))
            .map(|secret| dissent_net::lan_key(&secret))
            .collect();
        if let Some(node) = self.inner.node.get() {
            node.set_lan_keys(keys);
        }
    }
}
