//! End-to-end group encryption for Dissent communities, built on MLS (RFC 9420)
//! via OpenMLS.
//!
//! Each device owns one [`MlsClient`]. The client's MLS signing key is the
//! device's Ed25519 key, and its MLS credential is the root-signed
//! [`DeviceCert`] serialised with postcard. Every credential that enters a
//! group (in a key package, a commit or a welcome) must decode to a valid cert
//! whose device matches the leaf's signature key. Otherwise the message is
//! rejected, so MLS membership stays bound to Dissent identities.
//!
//! Groups use [`CIPHERSUITE`], a hybrid X-Wing (X25519 with ML-KEM-768) key exchange, AES-256-GCM and
//! Ed25519 signatures. The suite is from the IETF draft `draft-ietf-mls-pq-ciphersuites`, so the wire
//! format may change before the RFC is published.
//!
//! Group ids are the 32-byte community ids. State survives restarts through
//! [`MlsClient::snapshot`] and [`MlsClient::restore`]. The snapshot holds key
//! material in clear, like the rest of the keystore.

use std::collections::HashMap;
use std::fmt::Display;

use dissent_identity::{DeviceCert, DeviceId, DeviceKey};
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::storage::StorageProvider as _;
use serde::{Deserialize, Serialize};

/// The single ciphersuite used by Dissent groups.
///
/// `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`: a hybrid X25519 and ML-KEM-768 (X-Wing)
/// key exchange, AES-256-GCM, SHA-384 and Ed25519 signatures. It comes from the IETF draft
/// `draft-ietf-mls-pq-ciphersuites`, so the wire format may change before the RFC is published.
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

/// Prefix on every snapshot: a format tag and the ciphersuite it was taken under.
/// Snapshots without it, or from another suite, cannot be restored.
const SNAPSHOT_HEADER: [u8; 6] = [
    b'D',
    b'S',
    b'N',
    b'P',
    (CIPHERSUITE as u16 >> 8) as u8,
    CIPHERSUITE as u16 as u8,
];

/// Key packages each device keeps ready. Topped up whenever they are handed out.
pub const KEY_PACKAGE_POOL: usize = 5;

/// Out-of-order tolerance of sender ratchets: a message may arrive this many generations late.
const RATCHET_TOLERANCE: u32 = 64;
/// How far ahead of the newest generation a sender ratchet will skip.
const RATCHET_FORWARD: u32 = 1000;
/// Past epochs kept, so messages sent just before a commit still decrypt after it.
const PAST_EPOCHS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum MlsError {
    #[error("unknown group")]
    UnknownGroup,
    #[error("group already exists")]
    GroupExists,
    #[error("device is not a member of the group")]
    NotAMember,
    #[error("credential rejected: {0}")]
    Credential(&'static str),
    #[error("ciphersuite {found:#06x} is not the required {expected:#06x}")]
    WrongCiphersuite { expected: u16, found: u16 },
    #[error("could not restore a welcome's key package: {0}")]
    KeyPackageRestore(String),
    #[error("snapshot is not from this format or ciphersuite and cannot be restored")]
    IncompatibleSnapshot,
    #[error("message rejected: {0}")]
    Rejected(&'static str),
    #[error("encoding: {0}")]
    Encoding(String),
    #[error("openmls: {0}")]
    OpenMls(String),
}

pub type Result<T> = std::result::Result<T, MlsError>;

/// Result of [`MlsClient::process`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// A decrypted application message and the device that sent it.
    Application(Vec<u8>, DeviceId),
    /// A commit was merged and the group epoch advanced. If the commit removed
    /// this device, the group has been dropped locally.
    Commit,
    /// A standalone proposal was checked and discarded. Dissent only sends
    /// commits built from by-value proposals, so nothing is stored.
    Proposal,
}

/// Output of [`MlsClient::add_members`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Serialised MLS commit, sent to existing members.
    pub commit: Vec<u8>,
    /// Serialised MLS welcome, sent to the added devices.
    pub welcome: Vec<u8>,
}

/// What merging a commit would change. Built from the staged commit before anything is merged,
/// so a policy sees exactly the effect of accepting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// The certified device that authored the commit.
    pub committer: DeviceCert,
    /// Devices the commit adds.
    pub adds: Vec<DeviceCert>,
    /// Devices the commit removes, taken from the roster before the commit.
    pub removes: Vec<DeviceCert>,
    /// The epoch the group enters once the commit merges.
    pub epoch_after: u64,
}

/// What joining from a welcome would give. Obtained without storing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeInfo {
    /// The group the welcome joins, which is the community id.
    pub group: [u8; 32],
    /// Every member of the group at the welcome's epoch, each certified.
    pub members: Vec<DeviceCert>,
    /// The epoch of the welcome.
    pub epoch: u64,
}

/// Serialised client state: the MLS storage values and the key package pool.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    storage: Vec<(Vec<u8>, Vec<u8>)>,
    pool: Vec<Vec<u8>>,
}

/// One device's MLS state: the OpenMLS provider, its signing key, its groups and its pool of
/// unused key packages.
///
/// State lives in OpenMLS's in-memory storage. [`MlsClient::snapshot`] serialises it, and
/// [`MlsClient::restore`] rebuilds the client from that snapshot. Callers must snapshot after
/// every change, since sender ratchets advance on each message.
pub struct MlsClient {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    groups: HashMap<[u8; 32], MlsGroup>,
    pool: Vec<Vec<u8>>,
}

impl MlsClient {
    /// Creates a client for `device`. `cert` must bind that device to a root and verify.
    pub fn new(device: &DeviceKey, cert: &DeviceCert) -> Result<Self> {
        if cert.device != device.id() {
            return Err(MlsError::Credential(
                "certificate is for a different device",
            ));
        }
        cert.verify()
            .map_err(|_| MlsError::Credential("device certificate does not verify"))?;

        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::from_raw(
            SignatureScheme::ED25519,
            device.0.seed().to_vec(),
            device.id().0.to_vec(),
        );
        signer.store(provider.storage()).map_err(om)?;

        let identity = postcard::to_allocvec(cert).map_err(encoding)?;
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: signer.to_public_vec().into(),
        };
        Ok(Self {
            provider,
            signer,
            credential,
            groups: HashMap::new(),
            pool: Vec::new(),
        })
    }

    /// Rebuilds a client from a [`snapshot`](Self::snapshot). Groups listed in `groups` are
    /// loaded from the restored storage. A listed group with no stored state is skipped.
    pub fn restore(
        device: &DeviceKey,
        cert: &DeviceCert,
        snapshot: &[u8],
        groups: &[[u8; 32]],
    ) -> Result<Self> {
        let body = snapshot
            .strip_prefix(&SNAPSHOT_HEADER[..])
            .ok_or(MlsError::IncompatibleSnapshot)?;
        let mut client = Self::new(device, cert)?;
        let saved: Snapshot = postcard::from_bytes(body).map_err(encoding)?;
        {
            let mut values = client
                .provider
                .storage()
                .values
                .write()
                .map_err(|_| MlsError::Rejected("storage lock poisoned"))?;
            values.extend(saved.storage);
        }
        client.pool = saved.pool;
        for group in groups {
            let loaded = MlsGroup::load(client.provider.storage(), &GroupId::from_slice(group))
                .map_err(om)?;
            if let Some(mls_group) = loaded {
                client.groups.insert(*group, mls_group);
            }
        }
        Ok(client)
    }

    /// Serialised state, for [`restore`](Self::restore). Includes the key package pool.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        let values = self
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| MlsError::Rejected("storage lock poisoned"))?;
        let saved = Snapshot {
            storage: values.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            pool: self.pool.clone(),
        };
        let mut out = SNAPSHOT_HEADER.to_vec();
        out.extend(postcard::to_allocvec(&saved).map_err(encoding)?);
        Ok(out)
    }

    /// Serialised, single-use KeyPackage to share so others can add this device.
    pub fn key_package(&self) -> Result<Vec<u8>> {
        let bundle = KeyPackage::builder()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .map_err(om)?;
        bundle.key_package().tls_serialize_detached().map_err(om)
    }

    /// Hands out up to `count` key packages from the pool, consuming them. The pool is topped up
    /// to [`KEY_PACKAGE_POOL`] first, so a device always has some ready.
    pub fn take_key_packages(&mut self, count: usize) -> Result<Vec<Vec<u8>>> {
        while self.pool.len() < KEY_PACKAGE_POOL {
            let fresh = self.key_package()?;
            self.pool.push(fresh);
        }
        let take = count.min(self.pool.len());
        Ok(self.pool.drain(..take).collect())
    }

    /// Whether this client holds state for `group`.
    pub fn has_group(&self, group: &[u8; 32]) -> bool {
        self.groups.contains_key(group)
    }

    /// Creates an empty group whose id is the community id.
    pub fn create_group(&mut self, group: [u8; 32]) -> Result<()> {
        if self.groups.contains_key(&group) {
            return Err(MlsError::GroupExists);
        }
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .max_past_epochs(PAST_EPOCHS)
            .sender_ratchet_configuration(sender_ratchet())
            .build();
        let mls_group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(&group),
            self.credential.clone(),
        )
        .map_err(om)?;
        self.groups.insert(group, mls_group);
        Ok(())
    }

    /// Adds devices from their KeyPackages. Each credential is verified before the commit is built.
    /// The commit is merged locally, so the epoch advances immediately.
    pub fn add_members(&mut self, group: &[u8; 32], key_packages: &[Vec<u8>]) -> Result<Commit> {
        let kps = key_packages
            .iter()
            .map(|bytes| decode_key_package(&self.provider, bytes))
            .collect::<Result<Vec<_>>>()?;
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let (commit, welcome, _group_info) = mls_group
            .add_members(&self.provider, &self.signer, &kps)
            .map_err(om)?;
        mls_group.merge_pending_commit(&self.provider).map_err(om)?;
        Ok(Commit {
            commit: commit.to_bytes().map_err(om)?,
            welcome: welcome.to_bytes().map_err(om)?,
        })
    }

    /// Removes the given devices from the group and returns the serialised commit. Used for bans.
    pub fn remove_members(&mut self, group: &[u8; 32], devices: &[DeviceId]) -> Result<Vec<u8>> {
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let leaves = devices
            .iter()
            .map(|device| leaf_of(mls_group, device))
            .collect::<Result<Vec<_>>>()?;
        let (commit, _welcome, _group_info) = mls_group
            .remove_members(&self.provider, &self.signer, &leaves)
            .map_err(om)?;
        mls_group.merge_pending_commit(&self.provider).map_err(om)?;
        commit.to_bytes().map_err(om)
    }

    /// Joins the group named by a welcome, accepting any group membership.
    ///
    /// Deprecated: use [`join_from_welcome_with_policy`](Self::join_from_welcome_with_policy).
    #[deprecated(note = "accepts any membership; use join_from_welcome_with_policy")]
    pub fn join_from_welcome(&mut self, welcome: &[u8]) -> Result<[u8; 32]> {
        self.join_from_welcome_with_policy(welcome, |_| true)
    }

    /// Describes the group a welcome would join, without joining or storing anything. Every member
    /// credential must verify. The welcome's key package is restored to storage afterwards.
    pub fn inspect_welcome(&self, welcome: &[u8]) -> Result<WelcomeInfo> {
        let (staged, restore) = stage_welcome(&self.provider, welcome)?;
        let info = welcome_info(&staged);
        restore.finish()?;
        info
    }

    /// Joins the group named by a welcome if `policy` accepts its [`WelcomeInfo`]. A refused
    /// welcome leaves no group behind, and its key package stays available for a later welcome.
    pub fn join_from_welcome_with_policy(
        &mut self,
        welcome: &[u8],
        policy: impl FnOnce(&WelcomeInfo) -> bool,
    ) -> Result<[u8; 32]> {
        let (staged, restore) = stage_welcome(&self.provider, welcome)?;
        let info = match welcome_info(&staged) {
            Ok(info) => info,
            Err(e) => return refuse(restore, e),
        };
        if self.groups.contains_key(&info.group) {
            return refuse(restore, MlsError::GroupExists);
        }
        if !policy(&info) {
            return refuse(restore, MlsError::Rejected("policy"));
        }
        let mls_group = match staged.into_group(&self.provider) {
            Ok(group) => group,
            Err(e) => return refuse(restore, om(e)),
        };
        restore.keep();
        self.groups.insert(info.group, mls_group);
        Ok(info.group)
    }

    /// Processes an incoming MLS message, accepting any valid commit from a member.
    ///
    /// Deprecated: a malicious member could add an outsider through this path. Use
    /// [`process_with_policy`](Self::process_with_policy).
    #[deprecated(note = "accepts any commit from a member; use process_with_policy")]
    pub fn process(&mut self, group: &[u8; 32], message: &[u8]) -> Result<Processed> {
        self.process_with_policy(group, message, |_| true)
    }

    /// Processes an incoming MLS message for `group`. `policy` decides whether a commit may take
    /// effect. It sees the [`CommitInfo`] that merging would produce, before the merge.
    ///
    /// If `policy` returns false, the staged commit is discarded and the group keeps its epoch.
    /// Application messages and proposals are handled without consulting `policy`. A commit that
    /// evicts this device drops the group.
    pub fn process_with_policy(
        &mut self,
        group: &[u8; 32],
        message: &[u8],
        policy: impl FnOnce(&CommitInfo) -> bool,
    ) -> Result<Processed> {
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let (processed, evicted) = process_in_group(&self.provider, mls_group, message, policy)?;
        if evicted {
            if let Some(mut dropped) = self.groups.remove(group) {
                dropped.delete(self.provider.storage()).map_err(om)?;
            }
        }
        Ok(processed)
    }

    /// Encrypts an application message for `group` in its current epoch.
    pub fn encrypt(&mut self, group: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let message = mls_group
            .create_message(&self.provider, &self.signer, plaintext)
            .map_err(om)?;
        message.to_bytes().map_err(om)
    }

    /// Current epoch of `group`.
    pub fn epoch(&self, group: &[u8; 32]) -> Result<u64> {
        Ok(self.group(group)?.epoch().as_u64())
    }

    /// Verified certificates of every current member of `group`.
    pub fn members(&self, group: &[u8; 32]) -> Result<Vec<DeviceCert>> {
        self.group(group)?
            .members()
            .map(|member| verify_member(&member.credential, &member.signature_key))
            .collect()
    }

    /// MLS exporter secret of `group` for `label`, `len` bytes long. Equal across members in one epoch.
    pub fn export_secret(&self, group: &[u8; 32], label: &str, len: usize) -> Result<Vec<u8>> {
        self.group(group)?
            .export_secret(self.provider.crypto(), label, &[], len)
            .map_err(om)
    }

    fn group(&self, group: &[u8; 32]) -> Result<&MlsGroup> {
        self.groups.get(group).ok_or(MlsError::UnknownGroup)
    }
}

/// Refuses any key package or welcome that does not use [`CIPHERSUITE`].
fn check_ciphersuite(found: Ciphersuite) -> Result<()> {
    if found == CIPHERSUITE {
        return Ok(());
    }
    Err(MlsError::WrongCiphersuite {
        expected: CIPHERSUITE as u16,
        found: found as u16,
    })
}

fn sender_ratchet() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(RATCHET_TOLERANCE, RATCHET_FORWARD)
}

/// Processes one message against a group, without touching the group map.
/// Returns the outcome and whether this device was evicted.
fn process_in_group(
    provider: &OpenMlsRustCrypto,
    mls_group: &mut MlsGroup,
    message: &[u8],
    policy: impl FnOnce(&CommitInfo) -> bool,
) -> Result<(Processed, bool)> {
    let mls_message = MlsMessageIn::tls_deserialize_exact(message).map_err(om)?;
    let protocol = mls_message
        .try_into_protocol_message()
        .map_err(|_| MlsError::Rejected("not a group message"))?;
    let processed = mls_group.process_message(provider, protocol).map_err(om)?;

    let sender_leaf = match processed.sender() {
        Sender::Member(index) => Some(*index),
        _ => None,
    };

    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(app) => {
            let leaf =
                sender_leaf.ok_or(MlsError::Rejected("application message from non-member"))?;
            let device = member_device(mls_group, leaf)?;
            Ok((Processed::Application(app.into_bytes(), device), false))
        }
        ProcessedMessageContent::ProposalMessage(proposal) => {
            check_proposal(&proposal)?;
            Ok((Processed::Proposal, false))
        }
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            let committer = sender_leaf.ok_or(MlsError::Rejected("commit from non-member"))?;
            let info = commit_info(mls_group, committer, &staged)?;
            if !policy(&info) {
                return Err(MlsError::Rejected("policy"));
            }
            mls_group
                .merge_staged_commit(provider, *staged)
                .map_err(om)?;
            Ok((Processed::Commit, !mls_group.is_active()))
        }
        ProcessedMessageContent::ExternalJoinProposalMessage(_) => Err(MlsError::Rejected(
            "external join proposals are not supported",
        )),
        ProcessedMessageContent::OwnPrivateMessage | ProcessedMessageContent::OwnPendingCommit => {
            Err(MlsError::Rejected("own message echoed back"))
        }
    }
}

/// Decodes and validates a serialised KeyPackage, including its credential.
fn decode_key_package(provider: &OpenMlsRustCrypto, bytes: &[u8]) -> Result<KeyPackage> {
    let key_package = KeyPackageIn::tls_deserialize_exact(bytes)
        .map_err(om)?
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .map_err(om)?;
    check_ciphersuite(key_package.ciphersuite())?;
    verify_leaf(key_package.leaf_node())?;
    Ok(key_package)
}

/// Verifies a leaf node's credential and that its signature key is the certified device.
fn leaf_cert(leaf: &LeafNode) -> Result<DeviceCert> {
    verify_member(leaf.credential(), leaf.signature_key().as_slice())
}

fn verify_leaf(leaf: &LeafNode) -> Result<()> {
    leaf_cert(leaf).map(|_| ())
}

/// Checks a Basic credential holds a valid [`DeviceCert`] for `signature_key`.
fn verify_member(credential: &Credential, signature_key: &[u8]) -> Result<DeviceCert> {
    if credential.credential_type() != CredentialType::Basic {
        return Err(MlsError::Credential(
            "credential is not a Dissent device cert",
        ));
    }
    let cert: DeviceCert = postcard::from_bytes(credential.serialized_content())
        .map_err(|_| MlsError::Credential("credential does not decode as a device cert"))?;
    cert.verify()
        .map_err(|_| MlsError::Credential("device certificate does not verify"))?;
    if cert.device.0.as_slice() != signature_key {
        return Err(MlsError::Credential(
            "signature key does not match certified device",
        ));
    }
    Ok(cert)
}

fn check_proposal(proposal: &QueuedProposal) -> Result<()> {
    match proposal.proposal() {
        Proposal::Add(add) => verify_leaf(add.key_package().leaf_node()),
        Proposal::Update(update) => verify_leaf(update.leaf_node()),
        _ => Ok(()),
    }
}

/// Describes what merging `staged` would do, checking every certificate it brings in.
///
/// Commits may only add, remove or update. External init, PSK, group context extension and every
/// other proposal type are refused, since Dissent never sends them. An update, whether a by-value
/// proposal or the committer's update path, must keep the same device as the leaf it replaces.
fn commit_info(
    mls_group: &MlsGroup,
    committer_leaf: LeafNodeIndex,
    staged: &StagedCommit,
) -> Result<CommitInfo> {
    let committer = current_cert(mls_group, committer_leaf)?;
    let mut adds = Vec::new();
    let mut removes = Vec::new();
    for proposal in staged.queued_proposals() {
        match proposal.proposal() {
            Proposal::Add(add) => adds.push(leaf_cert(add.key_package().leaf_node())?),
            Proposal::Remove(remove) => removes.push(current_cert(mls_group, remove.removed())?),
            Proposal::Update(update) => {
                let new = leaf_cert(update.leaf_node())?;
                let Sender::Member(leaf) = proposal.sender() else {
                    return Err(MlsError::Rejected("update from non-member"));
                };
                if current_cert(mls_group, *leaf)?.device != new.device {
                    return Err(MlsError::Rejected("update changes device"));
                }
            }
            _ => {
                return Err(MlsError::Rejected(
                    "commit carries a proposal type Dissent does not send",
                ))
            }
        }
    }
    if let Some(leaf) = staged.update_path_leaf_node() {
        if leaf_cert(leaf)?.device != committer.device {
            return Err(MlsError::Rejected("update path changes device"));
        }
    }
    Ok(CommitInfo {
        committer,
        adds,
        removes,
        epoch_after: staged.epoch().as_u64(),
    })
}

/// The verified certificate of the current member at `leaf`.
fn current_cert(mls_group: &MlsGroup, leaf: LeafNodeIndex) -> Result<DeviceCert> {
    let member = mls_group
        .members()
        .find(|m| m.index == leaf)
        .ok_or(MlsError::NotAMember)?;
    verify_member(&member.credential, &member.signature_key)
}

fn member_device(mls_group: &MlsGroup, leaf: LeafNodeIndex) -> Result<DeviceId> {
    current_cert(mls_group, leaf).map(|cert| cert.device)
}

/// A key package that a welcome consumed from storage. OpenMLS deletes it when it reads the
/// welcome, so it is written back on drop unless `keep` is called.
struct ConsumedKeyPackage<'a> {
    provider: &'a OpenMlsRustCrypto,
    saved: Option<(KeyPackageRef, KeyPackageBundle)>,
}

impl ConsumedKeyPackage<'_> {
    /// The welcome is accepted, so the key package stays spent.
    fn keep(mut self) {
        self.saved = None;
    }

    /// Writes the key package back now, and reports a storage failure to the caller.
    fn finish(mut self) -> Result<()> {
        match self.saved.take() {
            Some((hash_ref, bundle)) => self
                .provider
                .storage()
                .write_key_package(&hash_ref, &bundle)
                .map_err(|e| MlsError::KeyPackageRestore(e.to_string())),
            None => Ok(()),
        }
    }
}

impl Drop for ConsumedKeyPackage<'_> {
    /// Fallback for a guard dropped without `keep` or `finish`, such as on an early error return.
    /// A failure here cannot be returned, so it is logged at warn level. The key package then stays
    /// spent.
    fn drop(&mut self) {
        if let Some((hash_ref, bundle)) = self.saved.take() {
            if let Err(e) = self
                .provider
                .storage()
                .write_key_package(&hash_ref, &bundle)
            {
                tracing::warn!(
                    error = %e,
                    "could not restore a welcome's key package; it stays spent"
                );
            }
        }
    }
}

/// Ends a refused welcome: restores its key package, then returns the refusal. A restore failure
/// is returned in place of the refusal, since the caller must know the key package is spent.
fn refuse<T>(restore: ConsumedKeyPackage<'_>, err: MlsError) -> Result<T> {
    restore.finish()?;
    Err(err)
}

/// Stages a welcome without joining. The key package it consumes is saved so the caller can
/// restore it. Dropping the returned guard without `keep` restores it.
fn stage_welcome<'a>(
    provider: &'a OpenMlsRustCrypto,
    bytes: &[u8],
) -> Result<(StagedWelcome, ConsumedKeyPackage<'a>)> {
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(om)?;
    let welcome = match message.extract() {
        MlsMessageBodyIn::Welcome(welcome) => welcome,
        _ => return Err(MlsError::Rejected("message is not a welcome")),
    };
    check_ciphersuite(welcome.ciphersuite())?;
    let saved = first_key_package(provider, &welcome)?;
    let consumed = ConsumedKeyPackage { provider, saved };
    let config = MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .max_past_epochs(PAST_EPOCHS)
        .sender_ratchet_configuration(sender_ratchet())
        .build();
    match StagedWelcome::new_from_welcome(provider, &config, welcome, None) {
        Ok(staged) => Ok((staged, consumed)),
        Err(e) => refuse(consumed, om(e)),
    }
}

/// Finds the key package OpenMLS will consume for `welcome`, in the same order it searches.
/// Last-resort key packages are never consumed, so they give nothing to restore.
fn first_key_package(
    provider: &OpenMlsRustCrypto,
    welcome: &Welcome,
) -> Result<Option<(KeyPackageRef, KeyPackageBundle)>> {
    for secret in welcome.secrets() {
        let hash_ref = secret.new_member();
        let found: Option<KeyPackageBundle> =
            provider.storage().key_package(&hash_ref).map_err(om)?;
        if let Some(bundle) = found {
            if bundle.key_package().last_resort() {
                return Ok(None);
            }
            return Ok(Some((hash_ref, bundle)));
        }
    }
    Ok(None)
}

/// Describes a staged welcome: its group, verified members and epoch.
fn welcome_info(staged: &StagedWelcome) -> Result<WelcomeInfo> {
    let group = fixed_group_id(staged.group_context().group_id().as_slice())?;
    let members = staged
        .members()
        .map(|member| verify_member(&member.credential, &member.signature_key))
        .collect::<Result<Vec<_>>>()?;
    Ok(WelcomeInfo {
        group,
        members,
        epoch: staged.group_context().epoch().as_u64(),
    })
}

fn leaf_of(mls_group: &MlsGroup, device: &DeviceId) -> Result<LeafNodeIndex> {
    mls_group
        .members()
        .find(|m| m.signature_key.as_slice() == device.0.as_slice())
        .map(|m| m.index)
        .ok_or(MlsError::NotAMember)
}

fn fixed_group_id(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| MlsError::Rejected("group id is not a community id"))
}

fn om<E: Display>(err: E) -> MlsError {
    MlsError::OpenMls(err.to_string())
}

fn encoding<E: Display>(err: E) -> MlsError {
    MlsError::Encoding(err.to_string())
}
