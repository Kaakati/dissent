//! Conversations sealed with MLS. A 1:1 direct message and a group DM are each one MLS group.
//!
//! The group id of a 1:1 conversation is `blake3(b"dissent/dm-group/v1" ++ sorted roots)`, so both
//! sides name it the same way. Whoever has something to say first creates the group. It takes a key
//! package from the other side's devices, or from one of the other side's inboxes when they are
//! offline, and welcomes them. If both sides create it at once, the lower root's group wins and the
//! other side drops its own.
//!
//! Group DMs have a random id and a creator. Only the creator removes people. Any member can add,
//! leave or rename.
//!
//! Every recipient device has a FIFO queue of packets: welcomes, commits, messages and receipts. A
//! message is encrypted when it is queued, so a commit always reaches a device before the messages
//! sealed in its epoch. A packet goes directly when the device is reachable. Otherwise it is
//! deposited at the device's inboxes. See `docs/design/10-dm-delivery.md`.
//!
//! Each plaintext body is signed by the sending device and carries its own id. Receivers check
//! that the sender's cert root is a current member.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use std::fmt;
use std::str::FromStr;
use std::sync::PoisonError;

use dissent_events::Hlc;
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use dissent_mls::{CommitInfo, Processed, WelcomeInfo};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{clean_message, clean_name, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{decode, encode, Request, Response};
use crate::sealed::mls_err;
use crate::store::{append_record, read_bin, read_records, write_bin, Paths};
use crate::views::{ConversationSummary, DmItem, EngineEvent};
use crate::world::World;

/// Group DMs take at least this many members besides the creator.
pub const MIN_GROUP_MEMBERS: usize = 3;
/// Group DMs take at most this many members besides the creator.
pub const MAX_GROUP_MEMBERS: usize = 16;
/// Longest group name, in Unicode scalar values.
pub const MAX_GROUP_NAME_CHARS: usize = 64;

const DM_GROUP_DOMAIN: &[u8] = b"dissent/dm-group/v1";
const MSG_ID_DOMAIN: &[u8] = b"dissent/conv-msg/v1";
const MSG_SIG_DOMAIN: &[u8] = b"dissent/conv-msg-sig/v1";
const WELCOME_DOMAIN: &[u8] = b"dissent/conv-welcome/v1";
const RECEIPT_DOMAIN: &[u8] = b"dissent/conv-receipt/v1";

/// Conversation id: a 32-byte MLS group id, shown as lowercase base32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConvId(pub [u8; 32]);

impl fmt::Display for ConvId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&data_encoding::BASE32_NOPAD.encode(&self.0).to_lowercase())
    }
}

impl FromStr for ConvId {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_uppercase().as_bytes())
            .map_err(|_| CoreError::Invalid(format!("not a conversation id: {s}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::Invalid(format!("not a conversation id: {s}")))?;
        Ok(ConvId(arr))
    }
}

/// The id of the 1:1 conversation between two roots. The order of the arguments does not matter.
pub fn dm_conv_id(a: &RootId, b: &RootId) -> ConvId {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut buf = DM_GROUP_DOMAIN.to_vec();
    buf.extend_from_slice(&lo.0);
    buf.extend_from_slice(&hi.0);
    ConvId(*blake3::hash(&buf).as_bytes())
}

/// Whether a conversation is a 1:1 DM or a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvKind {
    Dm,
    Group,
}

impl ConvKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConvKind::Dm => "dm",
            ConvKind::Group => "group",
        }
    }
}

/// Where a message we sent has got to. Higher values are further along, and a status only moves up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Delivery {
    Failed,
    QueuedInbox,
    SentDirect,
    Delivered,
}

impl Delivery {
    fn as_str(self) -> &'static str {
        match self {
            Delivery::Failed => "failed",
            Delivery::QueuedInbox => "queued_inbox",
            Delivery::SentDirect => "sent_direct",
            Delivery::Delivered => "delivered",
        }
    }
}

/// What we know about a conversation, apart from its MLS state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConvMeta {
    pub id: ConvId,
    pub kind: ConvKind,
    pub creator: RootId,
    pub name: String,
    /// The HLC of the rename that set `name`. A later rename wins.
    pub name_hlc: Hlc,
    /// Every member root, us included, as of the latest commit we applied.
    pub members: Vec<RootId>,
    /// True once this device holds the MLS group.
    pub ready: bool,
    /// True once we left, or a commit removed us.
    pub left: bool,
    pub created_ms: u64,
}

/// Something a member says in the conversation. Signed, and sealed inside MLS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Action {
    Text(String),
    Rename(String),
    Leave,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Body {
    pub conv: ConvId,
    pub from: RootId,
    pub hlc: Hlc,
    pub action: Action,
}

/// A body with the sending device's cert and signature. This is the plaintext inside MLS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signed {
    pub body: Body,
    pub cert: DeviceCert,
    pub sig: Signature,
}

/// What travels between devices about a conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ConvPacket {
    /// An invitation: the MLS welcome, plus the metadata a joiner needs and checks.
    Welcome {
        conv: ConvId,
        kind: ConvKind,
        creator: RootId,
        name: String,
        members: Vec<RootId>,
        welcome: Vec<u8>,
        cert: DeviceCert,
        sig: Signature,
    },
    /// An MLS commit that changes the membership.
    Commit { conv: ConvId, commit: Vec<u8> },
    /// An application message, encrypted in the epoch that was current when it was queued.
    Message {
        conv: ConvId,
        id: [u8; 32],
        ciphertext: Vec<u8>,
    },
    /// A holder of a group invite asks to join. Signed by the holder's device, for one invite version.
    JoinRequest {
        conv: ConvId,
        version: u64,
        cert: DeviceCert,
        addr: String,
        sig: Signature,
    },
    /// The recipient read a message. Signed by the recipient's device.
    Receipt {
        conv: ConvId,
        id: [u8; 32],
        cert: DeviceCert,
        sig: Signature,
    },
}

impl ConvPacket {
    fn conv(&self) -> ConvId {
        match self {
            ConvPacket::Welcome { conv, .. }
            | ConvPacket::Commit { conv, .. }
            | ConvPacket::Message { conv, .. }
            | ConvPacket::Receipt { conv, .. }
            | ConvPacket::JoinRequest { conv, .. } => *conv,
        }
    }
}

/// A packet waiting for one device, with the root it belongs to. The root locates its inboxes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Queued {
    pub root: RootId,
    pub packet: ConvPacket,
}

/// A message stored for display. Only text messages are stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConvMsg {
    pub id: [u8; 32],
    pub conv: ConvId,
    pub from: RootId,
    pub hlc: Hlc,
    pub text: String,
}

/// Conversation state that survives restarts. The messages live in one log per conversation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ConvBook {
    pub convs: BTreeMap<ConvId, ConvMeta>,
    /// Packets per recipient device, oldest first. Each is removed once it is sent or deposited.
    pub queues: BTreeMap<DeviceId, Vec<Queued>>,
    /// Bodies written before their conversation had an MLS group. Sealed once it does.
    pub unsent: Vec<Body>,
    /// Delivery state of messages we sent, by message id.
    pub status: BTreeMap<[u8; 32], Delivery>,
    /// Why a message we sent was refused, by message id.
    pub failures: BTreeMap<[u8; 32], String>,
    /// Conversations a blocked person started. Their packets are acknowledged and dropped.
    pub silenced: BTreeSet<ConvId>,
    /// The creator's invite version per group. Revoking bumps it.
    pub invite_version: BTreeMap<ConvId, u64>,
    /// Join requests waiting for the creator, per group.
    pub join_requests: BTreeMap<ConvId, Vec<crate::invites::JoinRequest>>,
    /// Groups we asked to join with an invite. A welcome for one of them is accepted from anyone.
    pub invite_joins: BTreeMap<ConvId, crate::invites::InviteJoin>,
    /// The sender of each conversation the recipient's policy refused, so later packets for it are
    /// answered the same way.
    pub refused_convs: BTreeMap<ConvId, dissent_identity::RootId>,
    /// Conversations whose group is being created right now. Not stored.
    #[serde(skip)]
    pub creating: BTreeSet<ConvId>,
}

impl ConvBook {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(read_bin(&paths.conv_book())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_bin(&paths.conv_book(), self)
    }
}

/// Reads the stored messages of every conversation in the book.
pub(crate) fn load_messages(paths: &Paths, book: &ConvBook) -> BTreeMap<ConvId, Vec<ConvMsg>> {
    book.convs
        .keys()
        .map(|id| {
            let msgs = read_records::<ConvMsg>(&paths.conv_log(id)).unwrap_or_default();
            (*id, msgs)
        })
        .collect()
}

fn domain_bytes<T: Serialize>(domain: &[u8], value: &T) -> Result<Vec<u8>> {
    let mut out = domain.to_vec();
    out.extend(encode(value)?);
    Ok(out)
}

/// The id of a body. Two copies of one message share it, so duplicates are easy to spot.
fn msg_id(body: &Body) -> Result<[u8; 32]> {
    Ok(*blake3::hash(&domain_bytes(MSG_ID_DOMAIN, body)?).as_bytes())
}

fn verify_cert(cert: &DeviceCert) -> Result<()> {
    cert.verify()
        .map_err(|e| CoreError::Invalid(format!("cert: {e}")))
}

fn verify_sig<T: Serialize>(cert: &DeviceCert, domain: &[u8], value: &T, sig: &Signature) -> Result<()> {
    cert.device
        .verify(&domain_bytes(domain, value)?, sig)
        .map_err(|e| CoreError::Invalid(format!("signature: {e}")))
}

impl Signed {
    fn sign(body: Body, device: &DeviceKey, cert: &DeviceCert) -> Result<Self> {
        let sig = device.sign(&domain_bytes(MSG_SIG_DOMAIN, &body)?);
        Ok(Self {
            body,
            cert: cert.clone(),
            sig,
        })
    }

    /// Checks the sender's cert and the signature over the body.
    fn verify(&self) -> Result<()> {
        verify_cert(&self.cert)?;
        verify_sig(&self.cert, MSG_SIG_DOMAIN, &self.body, &self.sig)
    }
}

/// Whether a commit may take effect. The committer must be a current member. Only the creator may
/// remove anyone except its own devices. Nothing changes in a 1:1 conversation.
fn commit_allowed(info: &CommitInfo, meta: &ConvMeta) -> bool {
    let root = info.committer.root;
    if !meta.members.contains(&root) {
        return false;
    }
    if meta.kind == ConvKind::Dm && (!info.adds.is_empty() || !info.removes.is_empty()) {
        return false;
    }
    let removes_own_only = info.removes.iter().all(|c| c.root == root);
    removes_own_only || root == meta.creator
}

/// Whether a joiner accepts a welcome. The group must be the conversation, and its members must be
/// exactly the roots the invitation names, with us among them.
fn welcome_allowed(info: &WelcomeInfo, conv: ConvId, members: &[RootId], me: RootId) -> bool {
    if info.group != conv.0 {
        return false;
    }
    let roots: BTreeSet<RootId> = info.members.iter().map(|c| c.root).collect();
    let named: BTreeSet<RootId> = members.iter().copied().collect();
    roots == named && roots.contains(&me)
}

fn roots_of(certs: &[DeviceCert]) -> Vec<RootId> {
    let set: BTreeSet<RootId> = certs.iter().map(|c| c.root).collect();
    set.into_iter().collect()
}

fn packet_conv(packet: &ConvPacket) -> ConvId {
    packet.conv()
}

/// How long a direct request may take before we fall back to an inbox. Node requests wait up to 15 s
/// for an offline device, which is too long to hold a send or a conversation start.
pub(crate) const DIRECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Runs a direct request under `DIRECT_TIMEOUT`. A timeout reads as an error.
pub(crate) async fn direct<T, E>(fut: impl std::future::Future<Output = std::result::Result<T, E>>) -> std::result::Result<T, ()> {
    match tokio::time::timeout(DIRECT_TIMEOUT, fut).await {
        Ok(Ok(v)) => Ok(v),
        _ => Err(()),
    }
}

/// What happened to one direct send attempt.
enum Direct {
    Acked,
    Refused(String),
    Unreachable,
}

impl Engine {
    /// The MLS members of `conv`, as certified devices. Empty when we hold no group.
    fn mls_members(&self, conv: ConvId) -> Vec<DeviceCert> {
        self.inner
            .mls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .members(&conv.0)
            .unwrap_or_default()
    }

    /// Every other member device of `conv`, with its root.
    fn member_targets(&self, conv: ConvId) -> Vec<(DeviceId, RootId)> {
        let me = self.inner.identity.device_id();
        self.mls_members(conv)
            .into_iter()
            .filter(|c| c.device != me)
            .map(|c| (c.device, c.root))
            .collect()
    }

    fn meta(&self, conv: &ConvId) -> Result<ConvMeta> {
        self.read()
            .conv_book
            .convs
            .get(conv)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))
    }

    /// The meta of a conversation we can write to: not left.
    fn active_meta(&self, conv: &ConvId) -> Result<ConvMeta> {
        let meta = self.meta(conv)?;
        if meta.left {
            return Err(CoreError::Invalid("you have left this conversation".into()));
        }
        Ok(meta)
    }

    /// Adds `packet` to the queue of each target. Caller holds the write guard.
    fn enqueue(w: &mut World, targets: &[(DeviceId, RootId)], packet: &ConvPacket) {
        for (device, root) in targets {
            w.conv_book
                .queues
                .entry(*device)
                .or_default()
                .push(Queued {
                    root: *root,
                    packet: packet.clone(),
                });
        }
    }

    fn save_book(w: &World) -> Result<()> {
        w.conv_book.save(&w.paths)
    }

    /// Seals `body` under the conversation's current epoch and queues it for every other member.
    /// The conversation must hold an MLS group.
    fn seal_and_queue(&self, body: Body) -> Result<()> {
        let conv = body.conv;
        let signed = Signed::sign(body, &self.inner.identity.device, &self.inner.identity.cert)?;
        let plaintext = encode(&signed)?;
        let ciphertext = self
            .with_mls_ratchet(|mls| mls.encrypt(&conv.0, &plaintext))
            .map_err(mls_err)?;
        let id = msg_id(&signed.body)?;
        let targets = self.member_targets(conv);
        let packet = ConvPacket::Message {
            conv,
            id,
            ciphertext,
        };
        let mut guard = self.write();
        Self::enqueue(&mut guard, &targets, &packet);
        Self::save_book(&guard)
    }

    /// Key packages for `root`. Tries the root's devices first, then its inboxes. The first source
    /// that answers wins.
    async fn key_packages_for(&self, root: RootId) -> Result<Vec<Vec<u8>>> {
        let node = self.node();
        // Inbox devices are skipped here. Answering with one would hand them the welcome, and an inbox
        // is not a member that reads messages. Their pool is reached through `TakeKeyPackage`.
        let inboxes_of_root = self
            .read()
            .contact_book
            .known_inboxes
            .get(&root)
            .cloned()
            .unwrap_or_default();
        let devices: Vec<DeviceId> = self
            .read()
            .directory
            .devices_of(&root)
            .into_iter()
            .filter(|d| !inboxes_of_root.contains(d))
            .collect();
        let req = self.request_bytes(&Request::DmKeyPackage)?;
        for device in devices {
            if let Ok(reply) = direct(node.request(device, req.clone())).await {
                if let Ok(Response::KeyPackages(pkgs)) = decode::<Response>(&reply) {
                    if !pkgs.is_empty() {
                        return Ok(pkgs);
                    }
                }
            }
        }
        let inboxes = self
            .read()
            .contact_book
            .known_inboxes
            .get(&root)
            .cloned()
            .unwrap_or_default();
        let take = self.request_bytes(&Request::TakeKeyPackage { root })?;
        for inbox in inboxes {
            if let Ok(reply) = direct(node.request(inbox, take.clone())).await {
                if let Ok(Response::KeyPackages(pkgs)) = decode::<Response>(&reply) {
                    if !pkgs.is_empty() {
                        return Ok(pkgs);
                    }
                }
            }
        }
        Err(CoreError::NotFound(format!("no key package for {root}")))
    }

    /// Creates the MLS group and adds `key_packages`. Returns the welcome for the new devices.
    fn create_mls_group(&self, conv: ConvId, key_packages: &[Vec<u8>]) -> Result<Vec<u8>> {
        self.with_mls(|mls| mls.create_group(conv.0)).map_err(mls_err)?;
        let commit = self
            .with_mls(|mls| mls.add_members(&conv.0, key_packages))
            .map_err(mls_err)?;
        Ok(commit.welcome)
    }

    /// Queues a welcome for `targets`, signed by this device.
    fn queue_welcome(
        &self,
        w: &mut World,
        meta: &ConvMeta,
        welcome: Vec<u8>,
        targets: &[(DeviceId, RootId)],
    ) -> Result<()> {
        let sig = self.inner.identity.device.sign(&domain_bytes(
            WELCOME_DOMAIN,
            &(
                meta.id,
                meta.kind,
                meta.creator,
                &meta.name,
                &meta.members,
                &welcome,
            ),
        )?);
        let packet = ConvPacket::Welcome {
            conv: meta.id,
            kind: meta.kind,
            creator: meta.creator,
            name: meta.name.clone(),
            members: meta.members.clone(),
            welcome,
            cert: self.inner.identity.cert.clone(),
            sig,
        };
        Self::enqueue(w, targets, &packet);
        Ok(())
    }

    /// Stores the conversation meta and saves the book.
    fn set_meta(&self, w: &mut World, meta: ConvMeta) -> Result<()> {
        w.conv_book.convs.insert(meta.id, meta);
        Self::save_book(w)
    }

    /// Tells listeners that a conversation changed. A 1:1 DM also emits the peer-keyed event.
    fn notify_conv(&self, meta: &ConvMeta) {
        self.emit(EngineEvent::ConversationChanged {
            id: meta.id.to_string(),
        });
        if meta.kind == ConvKind::Dm {
            if let Some(peer) = meta.members.iter().copied().find(|m| *m != self.inner.me) {
                self.emit(EngineEvent::Dm {
                    peer: peer.to_string(),
                });
            }
        }
    }

    /// Seals and queues the bodies written before this conversation had a group.
    fn release_unsent(&self, conv: ConvId) -> Result<()> {
        let bodies: Vec<Body> = {
            let mut guard = self.write();
            let (mine, rest): (Vec<Body>, Vec<Body>) = guard
                .conv_book
                .unsent
                .drain(..)
                .partition(|b| b.conv == conv);
            guard.conv_book.unsent = rest;
            Self::save_book(&guard)?;
            mine
        };
        for body in bodies {
            self.seal_and_queue(body)?;
        }
        Ok(())
    }

    /// Moves a message's delivery state up to `to`. A state never moves down.
    fn raise_status(&self, id: [u8; 32], to: Delivery) {
        let mut guard = self.write();
        let entry = guard.conv_book.status.entry(id).or_insert(to);
        if to > *entry {
            *entry = to;
        }
        if let Err(e) = Self::save_book(&guard) {
            warn!(error = %e, "could not save delivery state");
        }
    }

    /// Creates a group DM with `members`. Each member needs a key package, from a device or an inbox.
    pub async fn create_group(&self, name: String, members: Vec<RootId>) -> Result<ConversationSummary> {
        let name = clean_name(&name)?;
        if name.chars().count() > MAX_GROUP_NAME_CHARS {
            return Err(CoreError::Invalid("group name is too long".into()));
        }
        let me = self.inner.me;
        let others: BTreeSet<RootId> = members.into_iter().filter(|m| *m != me).collect();
        if others.len() < MIN_GROUP_MEMBERS || others.len() > MAX_GROUP_MEMBERS {
            return Err(CoreError::Invalid(format!(
                "a group needs {MIN_GROUP_MEMBERS} to {MAX_GROUP_MEMBERS} other members"
            )));
        }
        let others: Vec<RootId> = others.into_iter().collect();
        let mut key_packages = Vec::new();
        for root in &others {
            key_packages.extend(self.key_packages_for(*root).await?);
        }

        let conv = ConvId(DeviceKey::generate().0.seed());
        let welcome = self.create_mls_group(conv, &key_packages)?;
        let mut all = vec![me];
        all.extend(others.iter().copied());
        all.sort();
        let meta = ConvMeta {
            id: conv,
            kind: ConvKind::Group,
            creator: me,
            name,
            name_hlc: self.stamp(),
            members: all,
            ready: true,
            left: false,
            created_ms: now_ms(),
        };
        let targets = self.member_targets(conv);
        {
            let mut guard = self.write();
            self.queue_welcome(&mut guard, &meta, welcome, &targets)?;
            self.set_meta(&mut guard, meta.clone())?;
        }
        self.flush_conv_queues().await;
        self.notify_conv(&meta);
        self.summary_of(conv)
    }

    /// Creates the 1:1 group for `conv` if it has none yet. Takes the other side's key package from
    /// its devices or inboxes. Single-flight per conversation.
    pub(crate) async fn ensure_group(&self, conv: ConvId) -> Result<()> {
        let (meta, peer) = {
            let mut guard = self.write();
            let Some(meta) = guard.conv_book.convs.get(&conv).cloned() else {
                return Ok(());
            };
            if meta.ready || meta.left || meta.kind != ConvKind::Dm {
                return Ok(());
            }
            if !guard.conv_book.creating.insert(conv) {
                return Ok(());
            }
            let peer = meta.members.iter().copied().find(|m| *m != self.inner.me);
            (meta, peer)
        };
        let result = self.create_dm_group(meta, peer).await;
        self.write().conv_book.creating.remove(&conv);
        result
    }

    async fn create_dm_group(&self, meta: ConvMeta, peer: Option<RootId>) -> Result<()> {
        let Some(peer) = peer else {
            return Err(CoreError::Invalid("conversation has no other party".into()));
        };
        let key_packages = self.key_packages_for(peer).await?;
        let welcome = self.create_mls_group(meta.id, &key_packages)?;
        let me = self.inner.me;
        let mut members = vec![me, peer];
        members.sort();
        let updated = ConvMeta {
            creator: me,
            members,
            ready: true,
            ..meta
        };
        let targets = self.member_targets(updated.id);
        {
            let mut guard = self.write();
            self.queue_welcome(&mut guard, &updated, welcome, &targets)?;
            self.set_meta(&mut guard, updated.clone())?;
        }
        self.release_unsent(updated.id)
    }

    /// Adds members to a group. Any member may add. The others get a commit, and the new devices
    /// get a welcome.
    pub async fn group_add(&self, conv: ConvId, members: Vec<RootId>) -> Result<ConversationSummary> {
        let meta = self.active_meta(&conv)?;
        if meta.kind != ConvKind::Group || !meta.ready {
            return Err(CoreError::Invalid("not a group conversation".into()));
        }
        let fresh: Vec<RootId> = members
            .into_iter()
            .filter(|m| !meta.members.contains(m))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if meta.members.len() + fresh.len() > MAX_GROUP_MEMBERS + 1 {
            return Err(CoreError::Invalid("the group is full".into()));
        }
        if fresh.is_empty() {
            return self.summary_of(conv);
        }
        let mut key_packages = Vec::new();
        for root in &fresh {
            key_packages.extend(self.key_packages_for(*root).await?);
        }
        let before: BTreeSet<DeviceId> = self.mls_members(conv).iter().map(|c| c.device).collect();
        let commit = self
            .with_mls(|mls| mls.add_members(&conv.0, &key_packages))
            .map_err(mls_err)?;
        let after = self.mls_members(conv);
        let me = self.inner.identity.device_id();
        let existing: Vec<(DeviceId, RootId)> = after
            .iter()
            .filter(|c| before.contains(&c.device) && c.device != me)
            .map(|c| (c.device, c.root))
            .collect();
        let added: Vec<(DeviceId, RootId)> = after
            .iter()
            .filter(|c| !before.contains(&c.device))
            .map(|c| (c.device, c.root))
            .collect();
        let mut updated = meta.clone();
        updated.members = roots_of(&after);
        {
            let mut guard = self.write();
            Self::enqueue(
                &mut guard,
                &existing,
                &ConvPacket::Commit {
                    conv,
                    commit: commit.commit,
                },
            );
            self.queue_welcome(&mut guard, &updated, commit.welcome, &added)?;
            self.set_meta(&mut guard, updated.clone())?;
        }
        self.flush_conv_queues().await;
        self.notify_conv(&updated);
        self.summary_of(conv)
    }

    /// Removes a member from a group. Only the creator may do this.
    pub async fn group_remove(&self, conv: ConvId, member: RootId) -> Result<ConversationSummary> {
        let meta = self.active_meta(&conv)?;
        if meta.kind != ConvKind::Group {
            return Err(CoreError::Invalid("not a group conversation".into()));
        }
        if meta.creator != self.inner.me {
            return Err(CoreError::Rejected("NoPermission".into()));
        }
        if member == self.inner.me {
            return Err(CoreError::Invalid("use group_leave to leave".into()));
        }
        let devices: Vec<DeviceId> = self
            .mls_members(conv)
            .iter()
            .filter(|c| c.root == member)
            .map(|c| c.device)
            .collect();
        if devices.is_empty() {
            return Err(CoreError::NotFound(format!("member {member}")));
        }
        self.remove_devices(conv, &meta, &devices).await
    }

    /// Removes `devices` with one commit. The rest of the group and the removed devices get it.
    async fn remove_devices(
        &self,
        conv: ConvId,
        meta: &ConvMeta,
        devices: &[DeviceId],
    ) -> Result<ConversationSummary> {
        let targets = self.member_targets(conv);
        let commit = self
            .with_mls(|mls| mls.remove_members(&conv.0, devices))
            .map_err(mls_err)?;
        let mut updated = meta.clone();
        updated.members = roots_of(&self.mls_members(conv));
        {
            let mut guard = self.write();
            Self::enqueue(&mut guard, &targets, &ConvPacket::Commit { conv, commit });
            self.set_meta(&mut guard, updated.clone())?;
        }
        self.flush_conv_queues().await;
        self.notify_conv(&updated);
        self.summary_of(conv)
    }

    /// Leaves a group. The leaver says so in the group, and the creator then removes the leaver.
    pub async fn group_leave(&self, conv: ConvId) -> Result<()> {
        let meta = self.active_meta(&conv)?;
        if meta.kind != ConvKind::Group {
            return Err(CoreError::Invalid("a one-to-one conversation cannot be left".into()));
        }
        self.post(conv, Action::Leave).await?;
        let mut guard = self.write();
        let mut updated = meta;
        updated.left = true;
        self.set_meta(&mut guard, updated.clone())?;
        drop(guard);
        self.notify_conv(&updated);
        Ok(())
    }

    /// Renames a group. Every member applies the newest rename.
    pub async fn group_rename(&self, conv: ConvId, name: String) -> Result<ConversationSummary> {
        let name = clean_name(&name)?;
        if name.chars().count() > MAX_GROUP_NAME_CHARS {
            return Err(CoreError::Invalid("group name is too long".into()));
        }
        let meta = self.active_meta(&conv)?;
        if meta.kind != ConvKind::Group {
            return Err(CoreError::Invalid("not a group conversation".into()));
        }
        let hlc = self.stamp();
        self.post_with_hlc(conv, Action::Rename(name.clone()), hlc).await?;
        let mut guard = self.write();
        let mut updated = meta;
        updated.name = name;
        updated.name_hlc = hlc;
        self.set_meta(&mut guard, updated.clone())?;
        drop(guard);
        self.notify_conv(&updated);
        self.summary_of(conv)
    }

    /// Writes a control action (leave, rename) to the conversation, sealed like a message.
    async fn post(&self, conv: ConvId, action: Action) -> Result<()> {
        let hlc = self.stamp();
        self.post_with_hlc(conv, action, hlc).await
    }

    async fn post_with_hlc(&self, conv: ConvId, action: Action, hlc: Hlc) -> Result<()> {
        let body = Body {
            conv,
            from: self.inner.me,
            hlc,
            action,
        };
        self.seal_and_queue(body)?;
        self.flush_conv_queues().await;
        Ok(())
    }

    /// Sends a text message to a conversation. Before the conversation has a group, the text waits
    /// locally and is sealed once it does.
    pub async fn send_to_conversation(&self, conv: ConvId, text: String) -> Result<DmItem> {
        let text = clean_message(&text)?;
        let embeds = self.build_embeds(&text).await;
        let plain = if embeds.is_empty() {
            text
        } else {
            crate::attachments::pack_with_embeds(&text, &[], &embeds)?
        };
        self.send_conversation_plain(conv, plain).await
    }

    /// Seals a payload into a conversation and sends it. `plain` is text, or packed attachments.
    pub(crate) async fn send_conversation_plain(&self, conv: ConvId, text: String) -> Result<DmItem> {
        let meta = self.active_meta(&conv)?;
        let body = Body {
            conv,
            from: self.inner.me,
            hlc: self.stamp(),
            action: Action::Text(text.clone()),
        };
        let id = msg_id(&body)?;
        let stored = ConvMsg {
            id,
            conv,
            from: self.inner.me,
            hlc: body.hlc,
            text,
        };
        {
            let mut guard = self.write();
            append_record(&guard.paths.conv_log(&conv), &stored)?;
            guard.conv_msgs.entry(conv).or_default().push(stored.clone());
            if !meta.ready {
                guard.conv_book.unsent.push(body.clone());
                Self::save_book(&guard)?;
            }
        }
        if meta.ready {
            self.seal_and_queue(body)?;
        } else if let Err(e) = self.ensure_group(conv).await {
            debug!(error = %e, "conversation has no group yet");
        }
        self.flush_conv_queues().await;
        let item = {
            let w = self.read();
            self.item(&w, &stored)
        };
        self.notify_conv(&meta);
        Ok(item)
    }

    /// The newest `limit` messages of a conversation, oldest first.
    pub fn conversation_messages(&self, conv: ConvId, limit: usize) -> Vec<DmItem> {
        let w = self.read();
        let Some(log) = w.conv_msgs.get(&conv) else {
            return Vec::new();
        };
        let mut sorted: Vec<&ConvMsg> = log.iter().collect();
        sorted.sort_by_key(|m| m.hlc);
        let start = sorted.len().saturating_sub(limit);
        sorted[start..].iter().map(|m| self.item(&w, m)).collect()
    }

    pub(crate) fn item(&self, w: &World, m: &ConvMsg) -> DmItem {
        let delivery = if m.from == self.inner.me {
            w.conv_book
                .status
                .get(&m.id)
                .map_or("sending", |s| s.as_str())
        } else {
            "delivered"
        };
        DmItem {
            id: data_encoding::BASE32_NOPAD.encode(&m.id).to_lowercase(),
            from: m.from.to_string(),
            text: crate::attachments::text_of(&m.text),
            attachments: w.attachment_views(&m.text),
            embeds: crate::attachments::embeds_of(&m.text),
            ms: m.hlc.wall_ms,
            pending: delivery == "sending",
            delivery: delivery.to_string(),
            queued: delivery == "queued_inbox",
            reason: w.conv_book.failures.get(&m.id).cloned(),
        }
    }

    fn summary_of(&self, conv: ConvId) -> Result<ConversationSummary> {
        let w = self.read();
        let meta = w
            .conv_book
            .convs
            .get(&conv)
            .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))?;
        Ok(crate::dm::summary(&w, meta))
    }

    /// Sends queued packets, oldest first, to each device. A device that cannot be reached gets its
    /// packets deposited at its inboxes, and those leave the queue. A packet the device refuses is
    /// dropped, so one bad packet cannot block the queue. A 1:1 conversation still waiting for its
    /// group gets a creation attempt first.
    pub(crate) async fn flush_conv_queues(&self) {
        let waiting: Vec<ConvId> = {
            let w = self.read();
            w.conv_book
                .convs
                .values()
                .filter(|m| !m.ready && !m.left && m.kind == ConvKind::Dm)
                .filter(|m| w.conv_book.unsent.iter().any(|b| b.conv == m.id))
                .map(|m| m.id)
                .collect()
        };
        for conv in waiting {
            if let Err(e) = self.ensure_group(conv).await {
                debug!(error = %e, "conversation still has no group");
            }
        }
        let devices: Vec<DeviceId> = self
            .read()
            .conv_book
            .queues
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(d, _)| *d)
            .collect();
        for device in devices {
            loop {
                let head = self
                    .read()
                    .conv_book
                    .queues
                    .get(&device)
                    .and_then(|q| q.first().cloned());
                let Some(item) = head else {
                    break;
                };
                match self.send_packet(device, &item.packet).await {
                    Direct::Acked => {
                        self.pop_if_head(device, &item);
                        if let ConvPacket::Message { id, .. } = item.packet {
                            self.raise_status(id, Delivery::SentDirect);
                        }
                    }
                    Direct::Refused(reason) => {
                        self.pop_if_head(device, &item);
                        if let ConvPacket::Welcome { conv, kind: ConvKind::Dm, .. } = item.packet {
                            self.abandon_dm_group(conv, reason.clone());
                        }
                        if let ConvPacket::Message { id, .. } = item.packet {
                            self.write().conv_book.failures.insert(id, reason);
                            self.raise_status(id, Delivery::Failed);
                        }
                    }
                    Direct::Unreachable => {
                        if self.deposit_packet(device, item.root, &item.packet).await {
                            self.pop_if_head(device, &item);
                            if let ConvPacket::Message { id, .. } = item.packet {
                                self.raise_status(id, Delivery::QueuedInbox);
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn send_packet(&self, device: DeviceId, packet: &ConvPacket) -> Direct {
        let Ok(bytes) = self.request_bytes(&Request::Conv(Box::new(packet.clone()))) else {
            return Direct::Refused("could not encode the packet".into());
        };
        match direct(self.node().request(device, bytes)).await {
            Ok(reply) => match decode::<Response>(&reply) {
                Ok(Response::Ack) => Direct::Acked,
                Ok(Response::Refused(reason)) => Direct::Refused(reason),
                Ok(other) => {
                    warn!(device = %device, reply = ?other, "conversation packet refused");
                    Direct::Refused("the device could not process it".into())
                }
                Err(_) => Direct::Refused("unreadable reply".into()),
            },
            Err(()) => {
                debug!(device = %device, "conversation packet not delivered");
                Direct::Unreachable
            }
        }
    }

    /// Removes the head of a device's queue, only if it is still the packet we handled. Two flushes
    /// may run at once, and a second one must not remove a packet it never sent.
    fn pop_if_head(&self, device: DeviceId, item: &Queued) {
        let mut guard = self.write();
        if let Some(q) = guard.conv_book.queues.get_mut(&device) {
            if q.first() == Some(item) {
                q.remove(0);
            }
            if q.is_empty() {
                guard.conv_book.queues.remove(&device);
            }
        }
        if let Err(e) = Self::save_book(&guard) {
            warn!(error = %e, "could not save conversation queues");
        }
    }

    /// Handles a conversation packet from another device. Replies `Ack`, or an error for a refusal.
    pub(crate) async fn receive_conv(&self, packet: ConvPacket) -> Response {
        let conv = packet_conv(&packet);
        let result = match packet {
            ConvPacket::Welcome {
                conv,
                kind,
                creator,
                name,
                members,
                welcome,
                cert,
                sig,
            } => {
                self.on_welcome(conv, kind, creator, name, members, welcome, cert, sig)
                    .await
            }
            ConvPacket::Commit { conv, commit } => self.on_commit(conv, commit),
            ConvPacket::Message { conv, id, ciphertext } => self.on_message(conv, id, ciphertext),
            ConvPacket::Receipt { conv, id, cert, sig } => self.on_receipt(conv, id, cert, sig),
            ConvPacket::JoinRequest {
                conv,
                version,
                cert,
                addr,
                sig,
            } => self.on_join_request(conv, version, cert, addr, sig),
        };
        self.kick_conv_flush();
        match result {
            Ok(()) => Response::Ack,
            Err(CoreError::Rejected(reason)) => Response::Refused(reason),
            Err(CoreError::PolicyRefused { from, reason }) => {
                let mut guard = self.write();
                guard.conv_book.refused_convs.insert(conv, from);
                let _ = Self::save_book(&guard);
                crate::contacts::refusal_for(&mut guard, from, reason)
            }
            Err(e) => Response::Error(e.to_string()),
        }
    }

    /// Sends queued packets in the background. A handler must not wait on its own replies: the
    /// peer may be waiting on this handler, and the two would stall each other.
    pub(crate) fn kick_conv_flush(&self) {
        let engine = Engine {
            inner: self.inner.clone(),
        };
        self.spawn_task(async move { engine.flush_conv_queues().await });
    }

    #[allow(clippy::too_many_arguments)]
    async fn on_welcome(
        &self,
        conv: ConvId,
        kind: ConvKind,
        creator: RootId,
        name: String,
        members: Vec<RootId>,
        welcome: Vec<u8>,
        cert: DeviceCert,
        sig: Signature,
    ) -> Result<()> {
        verify_cert(&cert)?;
        verify_sig(
            &cert,
            WELCOME_DOMAIN,
            &(conv, kind, creator, &name, &members, &welcome),
            &sig,
        )?;
        let me = self.inner.me;
        if !members.contains(&me) || !members.contains(&creator) || !members.contains(&cert.root) {
            return Err(CoreError::Invalid("welcome does not name its members".into()));
        }
        if self.read().contact_book.blocked.contains(&cert.root) {
            let mut guard = self.write();
            guard.conv_book.silenced.insert(conv);
            Self::save_book(&guard)?;
            return Ok(());
        }
        let now = now_ms();
        let asked = self.read().conv_book.invite_joins.get(&conv).is_some_and(|j| {
            now < j.expires_ms && crate::invites::join_allowed(j, &cert)
        });
        if !asked && !crate::contacts::dm_allowed(&self.read(), &cert.root) {
            return Err(CoreError::PolicyRefused {
                from: cert.root,
                reason: "DM policy refuses this user".into(),
            });
        }
        let existing = self.read().conv_book.convs.get(&conv).cloned();
        if let Some(meta) = existing.filter(|m| m.ready) {
            if meta.creator != me {
                return Ok(());
            }
            if creator > me {
                return Err(CoreError::Rejected("conflicting group: keeping ours".into()));
            }
            // Both sides created a group at once. The lower root's group wins, so ours goes.
            self.drop_own_group(conv)?;
        }
        let joined = self
            .with_mls(|mls| {
                mls.join_from_welcome_with_policy(&welcome, |info| welcome_allowed(info, conv, &members, me))
            })
            .map_err(mls_err)?;
        if joined != conv.0 {
            return Err(CoreError::Invalid("welcome joined another group".into()));
        }
        let meta = ConvMeta {
            id: conv,
            kind,
            creator,
            name,
            name_hlc: Hlc::default(),
            members: roots_of(&self.mls_members(conv)),
            ready: true,
            left: false,
            created_ms: now_ms(),
        };
        {
            let mut guard = self.write();
            self.set_meta(&mut guard, meta.clone())?;
            guard.conv_book.refused_convs.remove(&conv);
            // The join is complete, so the invite allowance is spent.
            guard.conv_book.invite_joins.remove(&conv);
        }
        self.notify_conv(&meta);
        self.release_unsent(conv)
    }

    /// Forgets a 1:1 group whose welcome the other side refused. It was never joined there, so it
    /// must not stand in the way of a new group. Nothing is re-sent.
    fn abandon_dm_group(&self, conv: ConvId, reason: String) {
        if let Err(e) = self.with_mls(|mls| mls.drop_group(&conv.0)) {
            warn!(error = %e, "could not drop an unjoined group");
        }
        let mut guard = self.write();
        let w = &mut *guard;
        let mut failed = Vec::new();
        for q in w.conv_book.queues.values_mut() {
            for x in q.iter() {
                if let ConvPacket::Message { id, .. } = x.packet {
                    if packet_conv(&x.packet) == conv {
                        failed.push(id);
                    }
                }
            }
            q.retain(|x| packet_conv(&x.packet) != conv);
        }
        for id in failed {
            w.conv_book.failures.insert(id, reason.clone());
            w.conv_book.status.insert(id, Delivery::Failed);
        }
        w.conv_book.queues.retain(|_, q| !q.is_empty());
        if let Some(meta) = w.conv_book.convs.get_mut(&conv) {
            meta.ready = false;
        }
        if let Err(e) = Self::save_book(w) {
            warn!(error = %e, "could not save conversation state");
        }
    }

    /// Gives up our own group for `conv` after losing a tie-break. Our own messages go back to the
    /// unsent list, to be sealed in the winning group. Packets already queued for it are dropped.
    fn drop_own_group(&self, conv: ConvId) -> Result<()> {
        self.with_mls(|mls| mls.drop_group(&conv.0)).map_err(mls_err)?;
        let me = self.inner.me;
        let mut guard = self.write();
        let w = &mut *guard;
        let mine: Vec<Body> = w
            .conv_msgs
            .get(&conv)
            .into_iter()
            .flatten()
            .filter(|m| m.from == me)
            .map(|m| Body {
                conv,
                from: m.from,
                hlc: m.hlc,
                action: Action::Text(m.text.clone()),
            })
            .collect();
        w.conv_book.unsent.extend(mine);
        for q in w.conv_book.queues.values_mut() {
            q.retain(|x| packet_conv(&x.packet) != conv);
        }
        w.conv_book.queues.retain(|_, q| !q.is_empty());
        if let Some(meta) = w.conv_book.convs.get_mut(&conv) {
            meta.ready = false;
        }
        Self::save_book(w)
    }

    fn on_commit(&self, conv: ConvId, commit: Vec<u8>) -> Result<()> {
        let meta = self.active_meta(&conv)?;
        if !meta.ready {
            return Err(CoreError::Invalid("commit for a group we have not joined".into()));
        }
        let processed = self
            .with_mls(|mls| mls.process_commit_with_policy(&conv.0, &commit, |info| commit_allowed(info, &meta)))
            .map_err(mls_err)?;
        if processed != Processed::Commit {
            return Err(CoreError::Invalid("expected a commit".into()));
        }
        let mut updated = meta;
        let still_in = self
            .inner
            .mls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .has_group(&conv.0);
        if still_in {
            updated.members = roots_of(&self.mls_members(conv));
        } else {
            updated.left = true;
        }
        {
            let mut guard = self.write();
            self.set_meta(&mut guard, updated.clone())?;
        }
        self.notify_conv(&updated);
        Ok(())
    }

    fn on_message(&self, conv: ConvId, id: [u8; 32], ciphertext: Vec<u8>) -> Result<()> {
        if self.read().conv_book.silenced.contains(&conv) {
            return Ok(());
        }
        let meta = self
            .active_meta(&conv)
            .map_err(|_| match self.read().conv_book.refused_convs.get(&conv).copied() {
                Some(from) => CoreError::PolicyRefused {
                    from,
                    reason: "DM policy refuses this user".into(),
                },
                None => CoreError::Rejected("conversation not accepted".into()),
            })?;
        if !meta.ready {
            return Err(CoreError::Invalid("message for a group we have not joined".into()));
        }
        if self
            .read()
            .conv_msgs
            .get(&conv)
            .is_some_and(|log| log.iter().any(|m| m.id == id))
        {
            return Ok(());
        }
        let (plaintext, device) = self
            .with_mls_ratchet(|mls| mls.decrypt_application(&conv.0, &ciphertext))
            .map_err(mls_err)?;
        let signed: Signed = decode(&plaintext)?;
        signed.verify()?;
        if signed.cert.device != device || signed.body.conv != conv {
            return Err(CoreError::Invalid("message sender does not match".into()));
        }
        if signed.body.from != signed.cert.root || !meta.members.contains(&signed.cert.root) {
            return Err(CoreError::Invalid("message sender is not a member".into()));
        }
        if self.read().contact_book.blocked.contains(&signed.cert.root) {
            return Ok(());
        }
        self.clear_typer(crate::typing::conv_target(&self.inner.me, conv, signed.cert.root), signed.cert.root);
        if meta.kind == ConvKind::Dm && !crate::contacts::dm_allowed(&self.read(), &signed.cert.root) {
            return Err(CoreError::PolicyRefused {
                from: signed.cert.root,
                reason: "DM policy refuses this user".into(),
            });
        }
        if msg_id(&signed.body)? != id {
            return Err(CoreError::Invalid("message id does not match".into()));
        }
        let body = signed.body;
        match body.action {
            Action::Text(text) => {
                self.maybe_mirror(&text);
                self.auto_download(None, data_encoding::BASE32_NOPAD.encode(&id).to_lowercase(), &text);
                let stored = ConvMsg {
                    id,
                    conv,
                    from: body.from,
                    hlc: body.hlc,
                    text,
                };
                {
                    let mut guard = self.write();
                    append_record(&guard.paths.conv_log(&conv), &stored)?;
                    guard.conv_msgs.entry(conv).or_default().push(stored);
                }
                self.notify_conv(&meta);
                self.queue_receipt(conv, id, device, signed.cert.root)?;
            }
            Action::Rename(name) => {
                if body.hlc > meta.name_hlc {
                    let mut updated = meta;
                    updated.name = name;
                    updated.name_hlc = body.hlc;
                    let mut guard = self.write();
                    self.set_meta(&mut guard, updated.clone())?;
                    drop(guard);
                    self.notify_conv(&updated);
                }
            }
            Action::Leave => {
                // Only the creator removes people. Other members wait for the commit.
                if meta.creator == self.inner.me {
                    let leaving: Vec<DeviceId> = self
                        .mls_members(conv)
                        .iter()
                        .filter(|c| c.root == body.from)
                        .map(|c| c.device)
                        .collect();
                    if !leaving.is_empty() {
                        let targets = self.member_targets(conv);
                        let commit = self
                            .with_mls(|mls| mls.remove_members(&conv.0, &leaving))
                            .map_err(mls_err)?;
                        let mut updated = meta;
                        updated.members = roots_of(&self.mls_members(conv));
                        let mut guard = self.write();
                        Self::enqueue(&mut guard, &targets, &ConvPacket::Commit { conv, commit });
                        self.set_meta(&mut guard, updated.clone())?;
                        drop(guard);
                        self.notify_conv(&updated);
                    }
                }
            }
        }
        Ok(())
    }

    /// Queues a signed receipt for the device that sent message `id`.
    fn queue_receipt(&self, conv: ConvId, id: [u8; 32], device: DeviceId, root: RootId) -> Result<()> {
        let sig = self
            .inner
            .identity
            .device
            .sign(&domain_bytes(RECEIPT_DOMAIN, &(conv, id))?);
        let packet = ConvPacket::Receipt {
            conv,
            id,
            cert: self.inner.identity.cert.clone(),
            sig,
        };
        let mut guard = self.write();
        Self::enqueue(&mut guard, &[(device, root)], &packet);
        Self::save_book(&guard)
    }

    /// A receipt from a member's device. Marks one of our messages as delivered.
    fn on_receipt(&self, conv: ConvId, id: [u8; 32], cert: DeviceCert, sig: Signature) -> Result<()> {
        verify_cert(&cert)?;
        verify_sig(&cert, RECEIPT_DOMAIN, &(conv, id), &sig)?;
        let meta = self.meta(&conv)?;
        if !meta.members.contains(&cert.root) {
            return Err(CoreError::Invalid("receipt from a non-member".into()));
        }
        let ours = self
            .read()
            .conv_msgs
            .get(&conv)
            .is_some_and(|log| log.iter().any(|m| m.id == id && m.from == self.inner.me));
        if ours {
            self.raise_status(id, Delivery::Delivered);
            self.notify_conv(&meta);
        }
        Ok(())
    }
}
