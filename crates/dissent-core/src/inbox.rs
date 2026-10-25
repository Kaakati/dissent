//! Offline delivery through inboxes. An inbox is an always-on device of the same user, typically the
//! seeder. It holds sealed packets for its owner until the owner's devices collect them, and it
//! holds a few MLS key packages so that a conversation can start while the owner is offline. See
//! `docs/design/10-dm-delivery.md`.
//!
//! An inbox serves exactly one root, its owner's. It accepts deposits and key packages only for
//! that root, and it caps storage, lifetime and the deposit rate.

use std::collections::BTreeMap;

use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::groups::ConvPacket;
use crate::rpc::{decode, encode, Request, Response};
use crate::store::{read_bin, write_bin, Paths};

/// Bytes an inbox keeps for its owner, in total.
pub const MAX_STORED_BYTES: u64 = 50 * 1024 * 1024;
/// Longest time an envelope stays, in milliseconds.
pub const MAX_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Deposits one sender device may make in an hour.
pub const DEPOSITS_PER_HOUR: usize = 60;
/// Key packages an inbox keeps per device.
pub const KEY_PACKAGES_PER_DEVICE: usize = 5;
/// Inbox devices a user may list.
pub const MAX_INBOXES: usize = 4;
/// How far a collect request's timestamp may be from the inbox clock, in milliseconds.
const COLLECT_WINDOW_MS: u64 = 5 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;

const DEPOSIT_DOMAIN: &[u8] = b"dissent/inbox-deposit/v1";
const COLLECT_DOMAIN: &[u8] = b"dissent/inbox-collect/v1";
const ACK_DOMAIN: &[u8] = b"dissent/inbox-ack/v1";
const KEYS_DOMAIN: &[u8] = b"dissent/inbox-keys/v1";

/// A sealed packet waiting for one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub recipient: DeviceId,
    pub recipient_root: RootId,
    pub id: [u8; 32],
    pub expires_ms: u64,
    /// A postcard `ConvPacket`. The inbox cannot read it.
    pub packet: Vec<u8>,
    /// The sending device's cert.
    pub from: DeviceCert,
    pub sig: Signature,
}

/// A device asks for its envelopes. Signed by that device, with a recent timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collect {
    pub device: DeviceId,
    pub ts_ms: u64,
    pub sig: Signature,
}

/// A device acknowledges envelopes it has processed. The inbox deletes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckIds {
    pub device: DeviceId,
    pub ids: Vec<[u8; 32]>,
    pub sig: Signature,
}

/// A device deposits key packages with its inbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositKeys {
    pub cert: DeviceCert,
    pub packages: Vec<Vec<u8>>,
    pub sig: Signature,
}

/// One stored key package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredKey {
    pub root: RootId,
    pub device: DeviceId,
    pub package: Vec<u8>,
}

/// Inbox state. Stored only on a device that serves as an inbox, but always present so that a
/// device can switch the service on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct InboxBook {
    /// Whether this device accepts deposits for its own root.
    pub serve: bool,
    pub envelopes: Vec<Envelope>,
    pub keys: Vec<StoredKey>,
    /// Deposit times per sender device, for the hourly limit. Not stored: a restart resets it.
    #[serde(skip)]
    pub recent: BTreeMap<DeviceId, Vec<u64>>,
}

impl InboxBook {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(read_bin(&paths.inbox())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_bin(&paths.inbox(), self)
    }

    fn stored_bytes(&self) -> u64 {
        self.envelopes.iter().map(|e| e.packet.len() as u64).sum()
    }

    fn prune(&mut self, now: u64) {
        self.envelopes.retain(|e| e.expires_ms > now);
    }
}

/// Counts for the status views and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InboxStats {
    /// Whether this device serves as an inbox.
    pub serving: bool,
    /// Envelopes held for our root.
    pub envelopes: usize,
    /// Key packages held for our root.
    pub key_packages: usize,
}

/// Checks a signature over `domain` and a value by the device that signed it.
fn verify_device<T: Serialize>(device: &DeviceId, domain: &[u8], value: &T, sig: &Signature) -> Result<()> {
    let mut bytes = domain.to_vec();
    bytes.extend(encode(value)?);
    device
        .verify(&bytes, sig)
        .map_err(|e| CoreError::Invalid(format!("signature: {e}")))
}

fn sign_over<T: Serialize>(key: &DeviceKey, domain: &[u8], value: &T) -> Result<Signature> {
    let mut bytes = domain.to_vec();
    bytes.extend(encode(value)?);
    Ok(key.sign(&bytes))
}

impl Engine {
    /// Whether this device serves as an inbox for its own root.
    pub fn set_serve_inbox(&self, on: bool) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        w.inbox.serve = on;
        w.inbox.save(&w.paths)
    }

    /// Our inbox devices, which hold our envelopes and key packages while we are offline.
    pub fn inboxes(&self) -> Vec<DeviceId> {
        self.read().contact_book.my_inboxes.clone()
    }

    /// Lists the devices that hold our envelopes. Contacts learn the list, and a change is sent
    /// to them. Also deposits fresh key packages with the new inboxes.
    pub async fn set_inboxes(&self, devices: Vec<DeviceId>) -> Result<()> {
        let mut devices: Vec<DeviceId> = devices
            .into_iter()
            .filter(|d| *d != self.inner.identity.device_id())
            .collect();
        devices.dedup();
        if devices.len() > MAX_INBOXES {
            return Err(CoreError::Invalid(format!(
                "at most {MAX_INBOXES} inboxes"
            )));
        }
        {
            let mut guard = self.write();
            let w = &mut *guard;
            w.contact_book.my_inboxes = devices.clone();
            w.contact_book.save(&w.paths)?;
        }
        self.announce_inboxes().await;
        self.deposit_key_packages().await?;
        Ok(())
    }

    /// Counts what this device holds for its own inbox service.
    pub fn inbox_stats(&self) -> InboxStats {
        let w = self.read();
        InboxStats {
            serving: w.inbox.serve,
            envelopes: w.inbox.envelopes.len(),
            key_packages: w.inbox.keys.len(),
        }
    }

    /// Deposits up to `KEY_PACKAGES_PER_DEVICE` fresh key packages with each inbox. A conversation
    /// can then start with us while we are offline.
    pub async fn deposit_key_packages(&self) -> Result<()> {
        let inboxes = self.inboxes();
        if inboxes.is_empty() {
            return Ok(());
        }
        let packages = self
            .with_mls(|mls| mls.take_key_packages(KEY_PACKAGES_PER_DEVICE))
            .map_err(crate::sealed::mls_err)?;
        let cert = self.inner.identity.cert.clone();
        let sig = sign_over(&self.inner.identity.device, KEYS_DOMAIN, &packages)?;
        let body = DepositKeys { cert, packages, sig };
        let bytes = self.request_bytes(&Request::DepositKeyPackages(Box::new(body)))?;
        let node = self.node();
        for inbox in inboxes {
            if let Ok(reply) = crate::groups::direct(node.request(inbox, bytes.clone())).await {
                if !matches!(decode::<Response>(&reply), Ok(Response::Ack)) {
                    debug!(inbox = %inbox, "inbox refused key packages");
                }
            }
        }
        Ok(())
    }

    /// Asks every inbox for our envelopes, handles each packet as if it had arrived directly, and
    /// acknowledges the ones processed. Returns how many envelopes were handled.
    pub async fn collect_inboxes(&self) -> Result<usize> {
        let me = self.inner.identity.device_id();
        let inboxes: Vec<DeviceId> = self
            .inboxes()
            .into_iter()
            .filter(|d| *d != me)
            .collect();
        let node = self.node();
        let mut handled = 0usize;
        for inbox in inboxes {
            let ts_ms = now_ms();
            let sig = sign_over(&self.inner.identity.device, COLLECT_DOMAIN, &(me, ts_ms))?;
            let req = self.request_bytes(&Request::Collect(Box::new(Collect {
                device: me,
                ts_ms,
                sig,
            })))?;
            let envelopes = match node.request(inbox, req).await {
                Ok(reply) => match decode::<Response>(&reply) {
                    Ok(Response::Envelopes(list)) => list,
                    _ => continue,
                },
                Err(e) => {
                    debug!(inbox = %inbox, error = %e, "inbox unreachable");
                    continue;
                }
            };
            let mut ids = Vec::new();
            let mut packets = Vec::new();
            for env in envelopes {
                match decode::<ConvPacket>(&env.packet) {
                    Ok(p) => packets.push((env.id, p)),
                    Err(e) => {
                        warn!(error = %e, "dropping an undecodable envelope");
                        ids.push(env.id);
                    }
                }
            }
            // A welcome or a commit must land before the messages it makes readable, whatever order
            // the inbox returned them in. A refused message is acknowledged and gone for good.
            packets.sort_by_key(|(_, p)| !matches!(p, ConvPacket::Welcome { .. } | ConvPacket::Commit { .. }));
            for (id, packet) in packets {
                // Refusals are acknowledged too. A refused packet would be refused again.
                let _ = self.receive_conv(packet).await;
                ids.push(id);
                handled += 1;
            }
            if !ids.is_empty() {
                let sig = sign_over(&self.inner.identity.device, ACK_DOMAIN, &(me, &ids))?;
                let ack = self.request_bytes(&Request::AckEnvelopes(Box::new(AckIds {
                    device: me,
                    ids,
                    sig,
                })))?;
                let _ = node.request(inbox, ack).await;
            }
        }
        Ok(handled)
    }

    /// Deposits one packet for `device`, of `root`, at each inbox that root lists. True when one
    /// of them accepted it.
    pub(crate) async fn deposit_packet(&self, device: DeviceId, root: RootId, packet: &ConvPacket) -> bool {
        let inboxes = self
            .read()
            .contact_book
            .known_inboxes
            .get(&root)
            .cloned()
            .unwrap_or_default();
        if inboxes.is_empty() {
            return false;
        }
        let Ok(bytes) = encode(packet) else {
            return false;
        };
        let id = *blake3::hash(&bytes).as_bytes();
        let expires_ms = now_ms() + MAX_AGE_MS;
        let Ok(sig) = sign_over(
            &self.inner.identity.device,
            DEPOSIT_DOMAIN,
            &(device, root, id, expires_ms, &bytes),
        ) else {
            return false;
        };
        let envelope = Envelope {
            recipient: device,
            recipient_root: root,
            id,
            expires_ms,
            packet: bytes,
            from: self.inner.identity.cert.clone(),
            sig,
        };
        let Ok(req) = self.request_bytes(&Request::Deposit(Box::new(envelope))) else {
            return false;
        };
        let node = self.node();
        for inbox in inboxes {
            if let Ok(reply) = crate::groups::direct(node.request(inbox, req.clone())).await {
                if matches!(decode::<Response>(&reply), Ok(Response::Ack)) {
                    return true;
                }
            }
        }
        false
    }

    /// Sends the signed inbox list to every contact, through their outbox.
    pub(crate) async fn announce_inboxes(&self) {
        self.queue_inbox_announcements();
        self.retry_contact_outbox().await;
    }

    /// Handles a deposit. Returns `Ack`, or an error that says why.
    pub(crate) fn receive_deposit(&self, env: Envelope) -> Response {
        let mut guard = self.write();
        let w = &mut *guard;
        if !w.inbox.serve {
            return Response::Error("this device is not an inbox".into());
        }
        if env.recipient_root != w.me {
            return Response::Error("this inbox does not serve that user".into());
        }
        if env.from.verify().is_err()
            || verify_device(&env.from.device, DEPOSIT_DOMAIN, &envelope_fields(&env), &env.sig).is_err()
        {
            return Response::Error("invalid deposit".into());
        }
        let now = now_ms();
        if env.expires_ms <= now || env.expires_ms > now + MAX_AGE_MS {
            return Response::Error("deposit expiry is out of range".into());
        }
        if w.inbox.envelopes.iter().any(|e| e.recipient == env.recipient && e.id == env.id) {
            return Response::Ack;
        }
        let used = {
            let times = w.inbox.recent.entry(env.from.device).or_default();
            times.retain(|t| now.saturating_sub(*t) < HOUR_MS);
            times.len()
        };
        if used >= DEPOSITS_PER_HOUR {
            return Response::Error("deposit rate limit reached".into());
        }
        w.inbox.prune(now);
        if w.inbox.stored_bytes() + env.packet.len() as u64 > MAX_STORED_BYTES {
            return Response::Error("inbox is full".into());
        }
        w.inbox.recent.entry(env.from.device).or_default().push(now);
        w.inbox.envelopes.push(env);
        match w.inbox.save(&w.paths) {
            Ok(()) => Response::Ack,
            Err(e) => Response::Error(format!("could not store deposit: {e}")),
        }
    }

    /// Returns the envelopes for a device that signed a recent collect request.
    pub(crate) fn receive_collect(&self, req: Collect) -> Response {
        let now = now_ms();
        if now.abs_diff(req.ts_ms) > COLLECT_WINDOW_MS {
            return Response::Error("collect request is out of date".into());
        }
        if verify_device(&req.device, COLLECT_DOMAIN, &(req.device, req.ts_ms), &req.sig).is_err() {
            return Response::Error("invalid collect request".into());
        }
        let mut guard = self.write();
        let w = &mut *guard;
        if !w.inbox.serve {
            return Response::Error("this device is not an inbox".into());
        }
        w.inbox.prune(now);
        let mine: Vec<Envelope> = w
            .inbox
            .envelopes
            .iter()
            .filter(|e| e.recipient == req.device)
            .cloned()
            .collect();
        Response::Envelopes(mine)
    }

    /// Deletes envelopes a device has acknowledged.
    pub(crate) fn receive_ack(&self, ack: AckIds) -> Response {
        if verify_device(&ack.device, ACK_DOMAIN, &(ack.device, &ack.ids), &ack.sig).is_err() {
            return Response::Error("invalid acknowledgement".into());
        }
        let mut guard = self.write();
        let w = &mut *guard;
        w.inbox
            .envelopes
            .retain(|e| !(e.recipient == ack.device && ack.ids.contains(&e.id)));
        match w.inbox.save(&w.paths) {
            Ok(()) => Response::Ack,
            Err(e) => Response::Error(format!("could not save acknowledgement: {e}")),
        }
    }

    /// Stores a device's key packages. Only the owner's devices may deposit.
    pub(crate) fn receive_key_packages(&self, body: DepositKeys) -> Response {
        if body.cert.verify().is_err()
            || verify_device(&body.cert.device, KEYS_DOMAIN, &body.packages, &body.sig).is_err()
        {
            return Response::Error("invalid key packages".into());
        }
        let mut guard = self.write();
        let w = &mut *guard;
        if !w.inbox.serve {
            return Response::Error("this device is not an inbox".into());
        }
        if body.cert.root != w.me {
            return Response::Error("key packages must come from the owner's devices".into());
        }
        let held = w
            .inbox
            .keys
            .iter()
            .filter(|k| k.device == body.cert.device)
            .count();
        let room = KEY_PACKAGES_PER_DEVICE.saturating_sub(held);
        for package in body.packages.into_iter().take(room) {
            w.inbox.keys.push(StoredKey {
                root: body.cert.root,
                device: body.cert.device,
                package,
            });
        }
        match w.inbox.save(&w.paths) {
            Ok(()) => Response::Ack,
            Err(e) => Response::Error(format!("could not store key packages: {e}")),
        }
    }

    /// Hands out one key package for `root`. Each one is removed as it is handed out.
    pub(crate) fn take_key_package(&self, root: RootId) -> Response {
        let mut guard = self.write();
        let w = &mut *guard;
        if !w.inbox.serve || root != w.me {
            return Response::Error("no key package for that user".into());
        }
        let Some(pos) = w.inbox.keys.iter().position(|k| k.root == root) else {
            return Response::Error("no key package left".into());
        };
        let key = w.inbox.keys.remove(pos);
        if let Err(e) = w.inbox.save(&w.paths) {
            warn!(error = %e, "could not save key package state");
        }
        Response::KeyPackages(vec![key.package])
    }
}

/// The fields an envelope signature covers, in order.
fn envelope_fields(e: &Envelope) -> (DeviceId, RootId, [u8; 32], u64, &Vec<u8>) {
    (e.recipient, e.recipient_root, e.id, e.expires_ms, &e.packet)
}

#[cfg(test)]
mod tests;
