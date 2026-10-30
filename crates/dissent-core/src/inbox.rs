//! Offline delivery through inboxes. An inbox is an always-on device that holds sealed packets for a
//! person until that person's devices collect them, and a few MLS key packages so that a conversation
//! can start while the person is offline. See `docs/design/10-dm-delivery.md`.
//!
//! An inbox serves its own root when `serve` is on, and each root its owner chose to host
//! (`host_inbox_for`): a seeder run for a friend, or a friend's always-on device. It accepts deposits
//! and key packages only for the roots it serves, and it caps storage, lifetime and the deposit rate.
//! A person picks inboxes with `set_inboxes`, from the devices that answer `InboxProbe` as serving them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::groups::ConvPacket;
use crate::rpc::{decode, encode, Request, Response};
use crate::store::Paths;

/// Bytes an inbox keeps for one person it serves.
pub const MAX_STORED_BYTES: u64 = 50 * 1024 * 1024;
/// Bytes an inbox keeps for everyone it serves together.
pub const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
/// People a device may host an inbox for, besides its own root.
pub const MAX_HOSTED: usize = 16;
/// Devices `inbox_candidates` asks at most, so opening the picker stays quick.
const MAX_PROBES: usize = 32;
/// How long one probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
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
const PROBE_DOMAIN: &[u8] = b"dissent/inbox-probe/v1";

/// The one refusal for a deposit or key packages for a person this device does not hold mail for. It is
/// the same whether this device is no inbox at all or an inbox for other people, so a refusal does not show
/// which devices are inboxes (privacy audit L1).
pub(crate) const NOT_SERVED: &str = "this device does not hold mail for that person";

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

/// A device asks an inbox whether it holds mail for the device's root. Signed by that device, with a
/// recent timestamp, and carrying its cert so the inbox learns the root from the signature, not the sender's word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub cert: DeviceCert,
    pub ts_ms: u64,
    pub sig: Signature,
}

/// An inbox's answer to a probe. A device that does not serve the asker, or a probe that does not
/// verify, gets the default: not serving, nothing held. So the answer shows a stranger nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeAnswer {
    pub serving: bool,
    /// Envelopes held for any device of the asker's root.
    pub envelopes: u32,
    /// Key packages held for the asker's root.
    pub key_packages: u32,
    /// Bytes held for the asker's root, against `MAX_STORED_BYTES`.
    pub bytes: u64,
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
    /// Other people this device hosts an inbox for. Kept in its own file, `inbox_hosts.bin`, so the
    /// layout of `inbox.bin` from earlier versions still loads.
    #[serde(skip)]
    pub hosted: BTreeSet<RootId>,
    /// Notices from devices that started or stopped holding our mail, newest last, at most `MAX_NOTICES`.
    /// Kept in `inbox_notices.bin`.
    #[serde(skip)]
    pub notices: Vec<HostNotice>,
}

/// Notices kept for Settings > Offline delivery.
const MAX_NOTICES: usize = 20;

/// A device told us it started (`on`) or stopped holding our mail. See `ContactMsg::InboxHost`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HostNotice {
    pub root: RootId,
    pub device: DeviceId,
    pub on: bool,
    pub ts_ms: u64,
}

/// A host notice as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostNoticeView {
    pub root: String,
    pub name: String,
    pub device: String,
    pub on: bool,
    /// Whether the device is in our inbox list.
    pub chosen: bool,
    pub ts_ms: u64,
}

impl InboxBook {
    pub fn load(paths: &Paths) -> Result<Self> {
        let mut book: Self = paths.read_bin(&paths.inbox())?.unwrap_or_default();
        book.hosted = paths.read_bin(&paths.inbox_hosts())?.unwrap_or_default();
        book.notices = paths.read_bin(&paths.inbox_notices())?.unwrap_or_default();
        Ok(book)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.write_bin(&paths.inbox(), self)?;
        paths.write_bin(&paths.inbox_hosts(), &self.hosted)?;
        paths.write_bin(&paths.inbox_notices(), &self.notices)
    }

    /// Records a host notice. A device's newer notice replaces its older one; an older or repeated one is
    /// ignored. Returns whether anything changed.
    pub(crate) fn record_notice(&mut self, root: RootId, device: DeviceId, on: bool, ts_ms: u64) -> bool {
        if let Some(old) = self.notices.iter().find(|n| n.device == device) {
            if old.ts_ms >= ts_ms || old.on == on {
                return false;
            }
        }
        self.notices.retain(|n| n.device != device);
        self.notices.push(HostNotice { root, device, on, ts_ms });
        if self.notices.len() > MAX_NOTICES {
            self.notices.remove(0);
        }
        true
    }

    /// Whether this device holds mail for `root`, given that `me` is its own root.
    fn serves(&self, root: &RootId, me: &RootId) -> bool {
        (self.serve && root == me) || self.hosted.contains(root)
    }

    /// Whether this device holds mail for anyone.
    fn serving_any(&self) -> bool {
        self.serve || !self.hosted.is_empty()
    }

    fn stored_bytes(&self) -> u64 {
        self.envelopes.iter().map(|e| e.packet.len() as u64).sum()
    }

    fn stored_bytes_for(&self, root: &RootId) -> u64 {
        self.envelopes
            .iter()
            .filter(|e| e.recipient_root == *root)
            .map(|e| e.packet.len() as u64)
            .sum()
    }

    /// Drops everything held for `root`.
    fn forget(&mut self, root: &RootId) {
        self.envelopes.retain(|e| e.recipient_root != *root);
        self.keys.retain(|k| k.root != *root);
    }

    fn prune(&mut self, now: u64) {
        self.envelopes.retain(|e| e.expires_ms > now);
    }
}

/// What became of a deposit at the inboxes of a recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DepositOutcome {
    /// An inbox stored it.
    Accepted,
    /// An inbox answered and refused it. The reason is the inbox's.
    Refused(String),
    /// No inbox answered, so the packet stays queued.
    Unreachable,
}

/// Counts for the status views and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InboxStats {
    /// Whether this device serves as an inbox for its own root.
    pub serving: bool,
    /// Envelopes held, for every root served.
    pub envelopes: usize,
    /// Key packages held, for every root served.
    pub key_packages: usize,
    /// People besides us this device hosts an inbox for.
    pub hosted: usize,
    /// Bytes held, for every root served, against `MAX_TOTAL_BYTES`.
    pub bytes: u64,
}

/// One inbox device as Settings shows it: one we chose, or one that offers to hold our mail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InboxView {
    pub device: String,
    /// The root of the device's owner, when the directory knows it.
    pub owner: Option<String>,
    /// The owner's display name, or the device name for one of our own devices.
    pub name: String,
    /// One of our own devices.
    pub own_device: bool,
    /// Whether it is in our inbox list.
    pub chosen: bool,
    /// Whether it answered the probe.
    pub reachable: bool,
    /// Whether it holds mail for us.
    pub serving: bool,
    pub envelopes: u32,
    pub key_packages: u32,
    pub bytes: u64,
}

/// A person this device hosts an inbox for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostedView {
    pub root: String,
    pub name: String,
    pub envelopes: u32,
    pub bytes: u64,
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

    /// Counts what this device holds for its inbox service.
    pub fn inbox_stats(&self) -> InboxStats {
        let w = self.read();
        InboxStats {
            serving: w.inbox.serve,
            envelopes: w.inbox.envelopes.len(),
            key_packages: w.inbox.keys.len(),
            hosted: w.inbox.hosted.len(),
            bytes: w.inbox.stored_bytes(),
        }
    }

    /// Starts or stops holding mail for `root`, another person. Stopping drops what is held for them.
    /// For our own root, use `set_serve_inbox`. A change sends `root` a signed notice through the contact
    /// outbox, so they learn of it; setting what is already set sends nothing.
    pub fn host_inbox_for(&self, root: RootId, on: bool) -> Result<()> {
        let changed = {
            let mut guard = self.write();
            let w = &mut *guard;
            if root == w.me {
                return Err(CoreError::Invalid("this is your own identity: use the inbox switch for it".into()));
            }
            let changed = if on {
                if !w.inbox.hosted.contains(&root) && w.inbox.hosted.len() >= MAX_HOSTED {
                    return Err(CoreError::Invalid(format!("an inbox holds mail for at most {MAX_HOSTED} people")));
                }
                w.inbox.hosted.insert(root)
            } else if w.inbox.hosted.remove(&root) {
                w.inbox.forget(&root);
                true
            } else {
                false
            };
            w.inbox.save(&w.paths)?;
            if changed {
                self.queue_inbox_host_notice(w, root, on);
            }
            changed
        };
        if changed {
            self.kick_contact_outbox();
        }
        Ok(())
    }

    /// Notices from devices that started or stopped holding our mail, newest first.
    pub fn inbox_notices(&self) -> Vec<HostNoticeView> {
        let w = self.read();
        w.inbox
            .notices
            .iter()
            .rev()
            .map(|n| HostNoticeView {
                root: n.root.to_string(),
                name: w.display_name(&n.root),
                device: n.device.to_string(),
                on: n.on,
                chosen: w.contact_book.my_inboxes.contains(&n.device),
                ts_ms: n.ts_ms,
            })
            .collect()
    }

    /// The people this device holds mail for, besides us, with what it holds for each.
    pub fn hosted_inboxes(&self) -> Vec<HostedView> {
        let w = self.read();
        w.inbox
            .hosted
            .iter()
            .map(|root| HostedView {
                root: root.to_string(),
                name: w.display_name(root),
                envelopes: w.inbox.envelopes.iter().filter(|e| e.recipient_root == *root).count() as u32,
                bytes: w.inbox.stored_bytes_for(root),
            })
            .collect()
    }

    /// Our chosen inboxes, each asked now whether it still holds mail for us and how much.
    pub async fn inbox_status(&self) -> Vec<InboxView> {
        let chosen = self.inboxes();
        self.probe_all(chosen).await
    }

    /// Devices that offer to hold our mail and are not chosen yet: our own other devices, our contacts'
    /// devices and the seeders of our communities, each asked with a probe. Only those that serve us are
    /// returned. Asking tells each of them that we looked.
    pub async fn inbox_candidates(&self) -> Vec<InboxView> {
        let me = self.inner.identity.device_id();
        let devices: Vec<DeviceId> = {
            let w = self.read();
            let chosen: BTreeSet<DeviceId> = w.contact_book.my_inboxes.iter().copied().collect();
            let mut roots: Vec<RootId> = vec![w.me];
            roots.extend(w.contacts.iter().map(|c| c.cert.root));
            for c in w.communities.values() {
                roots.extend(c.state().members.iter().filter(|(_, m)| m.storage && !m.left).map(|(r, _)| *r));
            }
            let mut seen = BTreeSet::new();
            roots
                .into_iter()
                .flat_map(|r| w.directory.devices_of(&r))
                .filter(|d| *d != me && !chosen.contains(d) && seen.insert(*d))
                .take(MAX_PROBES)
                .collect()
        };
        self.probe_all(devices).await.into_iter().filter(|v| v.serving).collect()
    }

    /// Probes `devices` together, each under `PROBE_TIMEOUT`, in the order given.
    async fn probe_all(&self, devices: Vec<DeviceId>) -> Vec<InboxView> {
        let Ok(bytes) = self.probe_bytes() else {
            return Vec::new();
        };
        let node = self.node();
        let mut tasks = tokio::task::JoinSet::new();
        for (i, device) in devices.iter().copied().enumerate() {
            let node = node.clone();
            let bytes = bytes.clone();
            tasks.spawn(async move {
                let answer = match tokio::time::timeout(PROBE_TIMEOUT, node.request(device, bytes)).await {
                    Ok(Ok(reply)) => match decode::<Response>(&reply) {
                        Ok(Response::InboxProbe(a)) => Some(*a),
                        _ => None,
                    },
                    _ => None,
                };
                (i, answer)
            });
        }
        let mut answers: Vec<Option<ProbeAnswer>> = vec![None; devices.len()];
        while let Some(done) = tasks.join_next().await {
            if let Ok((i, answer)) = done {
                answers[i] = answer;
            }
        }
        let w = self.read();
        let chosen: BTreeSet<DeviceId> = w.contact_book.my_inboxes.iter().copied().collect();
        devices
            .into_iter()
            .zip(answers)
            .map(|(device, answer)| {
                let owner = w.directory.root_of(&device);
                let own_device = owner == Some(w.me);
                let name = match owner {
                    Some(_) if own_device => "Your device".to_owned(),
                    Some(root) => w.display_name(&root),
                    None => format!("device·{}", device.suffix()),
                };
                let a = answer.unwrap_or_default();
                InboxView {
                    device: device.to_string(),
                    owner: owner.map(|r| r.to_string()),
                    name,
                    own_device,
                    chosen: chosen.contains(&device),
                    reachable: answer.is_some(),
                    serving: a.serving,
                    envelopes: a.envelopes,
                    key_packages: a.key_packages,
                    bytes: a.bytes,
                }
            })
            .collect()
    }

    /// Whether `bytes` is an inbox request for a person this device holds mail for: a deposit for them, or a
    /// collect, ack, key-package deposit or take, or probe on their behalf. Hosting is consent, so such a request
    /// is answered even while this person is invisible. Senders to a hosted person can then tell the inbox is up.
    pub(crate) fn hosted_inbox_request(&self, bytes: &[u8]) -> bool {
        let Ok(env) = decode::<crate::access::InEnvelope>(bytes) else {
            return false;
        };
        let w = self.read();
        let held_for = |device: &DeviceId| w.inbox.envelopes.iter().any(|e| e.recipient == *device);
        let served_device = |device: &DeviceId| {
            held_for(device) || w.directory.root_of(device).is_some_and(|r| w.inbox.serves(&r, &w.me))
        };
        match &env.req {
            Request::Deposit(e) => w.inbox.serves(&e.recipient_root, &w.me),
            Request::Collect(c) => served_device(&c.device),
            Request::AckEnvelopes(a) => served_device(&a.device),
            Request::DepositKeyPackages(b) => w.inbox.serves(&b.cert.root, &w.me),
            Request::TakeKeyPackage { root } => w.inbox.serves(root, &w.me),
            Request::InboxProbe(p) => w.inbox.serves(&p.cert.root, &w.me),
            _ => false,
        }
    }

    /// A signed probe from this device, ready to send.
    fn probe_bytes(&self) -> Result<Vec<u8>> {
        let cert = self.inner.identity.cert.clone();
        let ts_ms = now_ms();
        let sig = sign_over(&self.inner.identity.device, PROBE_DOMAIN, &(cert.root, cert.device, ts_ms))?;
        self.request_bytes(&Request::InboxProbe(Box::new(Probe { cert, ts_ms, sig })))
    }

    /// Answers a probe: what this device holds for the asker's root, or the default when it does not
    /// serve that root or the probe does not verify.
    pub(crate) fn receive_probe(&self, probe: Probe) -> Response {
        let now = now_ms();
        let valid = now.abs_diff(probe.ts_ms) <= COLLECT_WINDOW_MS
            && probe.cert.verify().is_ok()
            && verify_device(
                &probe.cert.device,
                PROBE_DOMAIN,
                &(probe.cert.root, probe.cert.device, probe.ts_ms),
                &probe.sig,
            )
            .is_ok();
        let w = self.read();
        let root = probe.cert.root;
        if !valid || !w.inbox.serves(&root, &w.me) {
            return Response::InboxProbe(Box::default());
        }
        Response::InboxProbe(Box::new(ProbeAnswer {
            serving: true,
            envelopes: w.inbox.envelopes.iter().filter(|e| e.recipient_root == root && e.expires_ms > now).count() as u32,
            key_packages: w.inbox.keys.iter().filter(|k| k.root == root).count() as u32,
            bytes: w.inbox.stored_bytes_for(&root),
        }))
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

    /// Deposits one packet for `device`, of `root`, at each inbox that root lists. Says whether one of
    /// them accepted it, or refused it with a reason, or none answered.
    pub(crate) async fn deposit_packet(&self, device: DeviceId, root: RootId, packet: &ConvPacket) -> DepositOutcome {
        let inboxes = self
            .read()
            .contact_book
            .known_inboxes
            .get(&root)
            .cloned()
            .unwrap_or_default();
        if inboxes.is_empty() {
            return DepositOutcome::Unreachable;
        }
        let Ok(bytes) = encode(packet) else {
            return DepositOutcome::Unreachable;
        };
        let id = *blake3::hash(&bytes).as_bytes();
        let expires_ms = now_ms() + MAX_AGE_MS;
        let Ok(sig) = sign_over(
            &self.inner.identity.device,
            DEPOSIT_DOMAIN,
            &(device, root, id, expires_ms, &bytes),
        ) else {
            return DepositOutcome::Unreachable;
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
            return DepositOutcome::Unreachable;
        };
        let node = self.node();
        let mut refused = None;
        for inbox in inboxes {
            let Ok(reply) = crate::groups::direct(node.request(inbox, req.clone())).await else {
                continue;
            };
            match decode::<Response>(&reply) {
                Ok(Response::Ack) => return DepositOutcome::Accepted,
                Ok(Response::Error(reason)) => refused = Some(reason),
                _ => refused = Some("the inbox could not process it".to_owned()),
            }
        }
        refused.map_or(DepositOutcome::Unreachable, DepositOutcome::Refused)
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
        if !w.inbox.serves(&env.recipient_root, &w.me) {
            return Response::Error(NOT_SERVED.into());
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
        let size = env.packet.len() as u64;
        if w.inbox.stored_bytes_for(&env.recipient_root) + size > MAX_STORED_BYTES
            || w.inbox.stored_bytes() + size > MAX_TOTAL_BYTES
        {
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
        // A device that holds no mail answers as an inbox with nothing for this device would.
        if !w.inbox.serving_any() {
            return Response::Envelopes(Vec::new());
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
        if !w.inbox.serves(&body.cert.root, &w.me) {
            return Response::Error(NOT_SERVED.into());
        }
        // The device is now known, so a later notice that this device stopped holding the mail reaches it.
        w.directory.learn(&body.cert);
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
        // No package left looks the same as no inbox for that person, so a take does not show whether this
        // device holds mail for them.
        let Some(pos) = w
            .inbox
            .keys
            .iter()
            .position(|k| k.root == root)
            .filter(|_| w.inbox.serves(&root, &w.me))
        else {
            return Response::Error("no key package for that user".into());
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
