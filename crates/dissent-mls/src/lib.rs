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

use dissent_identity::{DeviceCert, DeviceId, DeviceKey, Vault};
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use serde::{Deserialize, Serialize};

/// The single ciphersuite used by Dissent groups.
///
/// `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`: a hybrid X25519 and ML-KEM-768 (X-Wing)
/// key exchange, AES-256-GCM, SHA-384 and Ed25519 signatures. It comes from the IETF draft
/// `draft-ietf-mls-pq-ciphersuites`, so the wire format may change before the RFC is published.
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

/// Label bound to the sealed MLS state. A blob sealed for another file does not open here.
const SNAPSHOT_AAD: &[u8] = b"dissent/mls-state.bin";
/// Purpose of the per-file subkey that seals the MLS state.
const SNAPSHOT_PURPOSE: &[u8] = b"dissent/mls-state/v1";

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
    #[error("snapshot is not from this format or ciphersuite and cannot be restored")]
    IncompatibleSnapshot,
    #[error("sealed state could not be opened: {0}")]
    Sealed(String),
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

    /// Rebuilds a client from a sealed snapshot made by [`snapshot_sealed`](Self::snapshot_sealed).
    /// Groups listed in `groups` are loaded from the restored storage. A listed group with no stored
    /// state is skipped. A wrong vault is refused with [`MlsError::Sealed`]. A plaintext file is
    /// refused too, and [`migrate_legacy`](Self::migrate_legacy) converts one.
    pub fn restore_sealed(
        device: &DeviceKey,
        cert: &DeviceCert,
        vault: &Vault,
        snapshot: &[u8],
        groups: &[[u8; 32]],
    ) -> Result<Self> {
        let plain = vault
            .subkey(SNAPSHOT_PURPOSE)
            .open_sealed(SNAPSHOT_AAD, snapshot)
            .map_err(|e| MlsError::Sealed(e.to_string()))?;
        Self::restore_body(device, cert, &plain, groups)
    }

    fn restore_body(
        device: &DeviceKey,
        cert: &DeviceCert,
        plain: &[u8],
        groups: &[[u8; 32]],
    ) -> Result<Self> {
        let body = plain
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

    /// Serialised state, sealed under `vault` for [`restore_sealed`](Self::restore_sealed). Includes the key
    /// package pool. The sealing key is a subkey of `vault`, so the file reveals no group or key
    /// material.
    pub fn snapshot_sealed(&self, vault: &Vault) -> Result<Vec<u8>> {
        let plain = self.plain_snapshot()?;
        Ok(vault.subkey(SNAPSHOT_PURPOSE).seal(SNAPSHOT_AAD, &plain))
    }

    /// Whether `bytes` is a plaintext snapshot written before sealing was added.
    pub fn is_legacy_snapshot(bytes: &[u8]) -> bool {
        bytes.starts_with(&SNAPSHOT_HEADER[..4])
    }

    /// Seals a plaintext snapshot from before sealing was added, so it can be restored under
    /// `vault`. Checks that the snapshot decodes under the current ciphersuite. Save the result in
    /// place of the old file.
    pub fn migrate_legacy(legacy: &[u8], vault: &Vault) -> Result<Vec<u8>> {
        let body = legacy
            .strip_prefix(&SNAPSHOT_HEADER[..])
            .ok_or(MlsError::IncompatibleSnapshot)?;
        let _: Snapshot = postcard::from_bytes(body).map_err(encoding)?;
        Ok(vault.subkey(SNAPSHOT_PURPOSE).seal(SNAPSHOT_AAD, legacy))
    }

    /// The plaintext snapshot: header, then the postcard body. Sealed by [`snapshot_sealed`](Self::snapshot_sealed).
    fn plain_snapshot(&self) -> Result<Vec<u8>> {
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

    /// Removes every trace of `group` from this client: its memory and its stored MLS state. A
    /// group that is not held does nothing and returns `Ok`, so this is safe to repeat.
    pub fn drop_group(&mut self, group: &[u8; 32]) -> Result<()> {
        let mut dropped = match self.groups.remove(group) {
            Some(held) => held,
            None => match MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group))
                .map_err(om)?
            {
                Some(stored) => stored,
                None => return Ok(()),
            },
        };
        dropped.delete(self.provider.storage()).map_err(om)
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
    /// credential must verify. Storage is rolled back afterwards, so the welcome's key package is
    /// still there to join with.
    pub fn inspect_welcome(&mut self, welcome: &[u8]) -> Result<WelcomeInfo> {
        let before = storage_values(&self.provider)?;
        let info = stage_welcome(&self.provider, welcome).and_then(|staged| welcome_info(&staged));
        restore_values(&self.provider, before)?;
        info
    }

    /// Joins the group named by a welcome if `policy` accepts its [`WelcomeInfo`]. A refused welcome
    /// leaves no group behind, and its key package stays available for a later welcome.
    pub fn join_from_welcome_with_policy(
        &mut self,
        welcome: &[u8],
        policy: impl FnOnce(&WelcomeInfo) -> bool,
    ) -> Result<[u8; 32]> {
        let before = storage_values(&self.provider)?;
        let result = self.join_staged(welcome, policy);
        if result.is_err() {
            restore_values(&self.provider, before)?;
        }
        result
    }

    fn join_staged(
        &mut self,
        welcome: &[u8],
        policy: impl FnOnce(&WelcomeInfo) -> bool,
    ) -> Result<[u8; 32]> {
        let staged = stage_welcome(&self.provider, welcome)?;
        let info = welcome_info(&staged)?;
        if self.groups.contains_key(&info.group) {
            return Err(MlsError::GroupExists);
        }
        if !policy(&info) {
            return Err(MlsError::Rejected("policy"));
        }
        let mls_group = staged.into_group(&self.provider).map_err(om)?;
        self.groups.insert(info.group, mls_group);
        Ok(info.group)
    }

    /// Processes an incoming MLS message, accepting any valid commit from a member.
    ///
    /// Deprecated: a malicious member could add an outsider through this path. Use
    /// [`process_with_policy`](Self::process_with_policy).
    #[deprecated(note = "accepts any commit from a member; use process_with_policy")]
    pub fn process(&mut self, group: &[u8; 32], message: &[u8]) -> Result<Processed> {
        self.process_checked(group, parse_protocol(message)?, Expect::Any, |_| true)
    }

    /// Processes a commit for `group`. `policy` decides whether it may take effect. It sees the
    /// [`CommitInfo`] that merging would produce, before the merge.
    ///
    /// If `policy` returns false, the commit is discarded and the group keeps its epoch. Any error
    /// leaves the group and storage as they were, so the same bytes can be processed again later
    /// once the policy accepts them. A message that is not a commit is rejected. A commit that
    /// evicts this device drops the group.
    ///
    /// Only commits pay for the storage snapshot, because OpenMLS consumes a commit's decryption
    /// secret while staging it.
    pub fn process_commit_with_policy(
        &mut self,
        group: &[u8; 32],
        commit: &[u8],
        policy: impl FnOnce(&CommitInfo) -> bool,
    ) -> Result<Processed> {
        let protocol = parse_protocol(commit)?;
        if protocol.content_type() != ContentType::Commit {
            return Err(MlsError::Rejected("not a commit"));
        }
        self.process_checked(group, protocol, Expect::Commit, policy)
    }

    /// Decrypts an application message for `group` and returns its plaintext and sender. A message
    /// that is not an application message is rejected.
    ///
    /// The envelope's content type is checked before anything is decrypted, so a message that is not
    /// an application message is rejected with no state change. The content type is authenticated
    /// as part of the AEAD additional data, so relabelling a message only makes it fail to decrypt.
    /// No storage snapshot is taken, because a message that fails signature or decryption changes
    /// nothing. A replay fails with `SecretReuseError`, or with `TooDistantInThePast` once it is
    /// older than the tolerance window.
    pub fn decrypt_application(
        &mut self,
        group: &[u8; 32],
        message: &[u8],
    ) -> Result<(Vec<u8>, DeviceId)> {
        let protocol = parse_protocol(message)?;
        if protocol.content_type() != ContentType::Application {
            return Err(MlsError::Rejected("not an application message"));
        }
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        match process_in_group(
            &self.provider,
            mls_group,
            protocol,
            Expect::Application,
            |_: &CommitInfo| true,
        )?
        .0
        {
            Processed::Application(plaintext, sender) => Ok((plaintext, sender)),
            _ => Err(MlsError::Rejected("not an application message")),
        }
    }

    /// Processes any message for `group`, with `policy` applied to commits.
    ///
    /// Deprecated: use [`process_commit_with_policy`](Self::process_commit_with_policy) for commits
    /// and [`decrypt_application`](Self::decrypt_application) for application messages.
    #[deprecated(
        note = "use process_commit_with_policy for commits and decrypt_application for messages"
    )]
    pub fn process_with_policy(
        &mut self,
        group: &[u8; 32],
        message: &[u8],
        policy: impl FnOnce(&CommitInfo) -> bool,
    ) -> Result<Processed> {
        self.process_checked(group, parse_protocol(message)?, Expect::Any, policy)
    }

    /// Shared body of the commit and deprecated paths. Takes a storage snapshot, and rolls back and
    /// reloads the group on any error.
    fn process_checked(
        &mut self,
        group: &[u8; 32],
        protocol: ProtocolMessage,
        expect: Expect,
        policy: impl FnOnce(&CommitInfo) -> bool,
    ) -> Result<Processed> {
        let before = storage_values(&self.provider)?;
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let outcome = process_in_group(&self.provider, mls_group, protocol, expect, policy);
        let (processed, evicted) = match outcome {
            Ok(done) => done,
            Err(err) => {
                restore_values(&self.provider, before)?;
                let reloaded = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group))
                    .map_err(om)?
                    .ok_or(MlsError::UnknownGroup)?;
                self.groups.insert(*group, reloaded);
                return Err(err);
            }
        };
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

    /// Every current member of `group` with its MLS leaf index. Indices are unique within an
    /// epoch, and a member keeps its index while it stays in the group.
    pub fn indexed_members(&self, group: &[u8; 32]) -> Result<Vec<(u32, DeviceCert)>> {
        self.group(group)?
            .members()
            .map(|member| {
                Ok((
                    member.index.u32(),
                    verify_member(&member.credential, &member.signature_key)?,
                ))
            })
            .collect()
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

/// The kind of message a caller accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Any,
    Commit,
    Application,
}

fn wrong_kind(expect: Expect) -> MlsError {
    match expect {
        Expect::Commit => MlsError::Rejected("not a commit"),
        _ => MlsError::Rejected("not an application message"),
    }
}

/// Processes one message against a group, without touching the group map.
/// Returns the outcome and whether this device was evicted.
///
/// OpenMLS checks the signature and decrypts before it applies the sender ratchet change, so a
/// message that fails those checks consumes nothing. Callers gate on the envelope's content type
/// first, so a message of the wrong kind is rejected before it reaches this function.
fn process_in_group(
    provider: &OpenMlsRustCrypto,
    mls_group: &mut MlsGroup,
    protocol: ProtocolMessage,
    expect: Expect,
    policy: impl FnOnce(&CommitInfo) -> bool,
) -> Result<(Processed, bool)> {
    let processed = mls_group.process_message(provider, protocol).map_err(om)?;

    let sender_leaf = match processed.sender() {
        Sender::Member(index) => Some(*index),
        _ => None,
    };

    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(app) => {
            if expect == Expect::Commit {
                return Err(wrong_kind(expect));
            }
            let leaf =
                sender_leaf.ok_or(MlsError::Rejected("application message from non-member"))?;
            let device = member_device(mls_group, leaf)?;
            Ok((Processed::Application(app.into_bytes(), device), false))
        }
        ProcessedMessageContent::ProposalMessage(proposal) => {
            if expect != Expect::Any {
                return Err(wrong_kind(expect));
            }
            check_proposal(&proposal)?;
            Ok((Processed::Proposal, false))
        }
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            if expect == Expect::Application {
                return Err(wrong_kind(expect));
            }
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

/// Parses the envelope of an incoming MLS message. Nothing is decrypted or consumed here.
fn parse_protocol(message: &[u8]) -> Result<ProtocolMessage> {
    MlsMessageIn::tls_deserialize_exact(message)
        .map_err(om)?
        .try_into_protocol_message()
        .map_err(|_| MlsError::Rejected("not a group message"))
}

/// Stages a welcome without joining. OpenMLS consumes the welcome's key package while staging, so
/// every caller rolls storage back when it does not keep the result.
fn stage_welcome(provider: &OpenMlsRustCrypto, bytes: &[u8]) -> Result<StagedWelcome> {
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(om)?;
    let welcome = match message.extract() {
        MlsMessageBodyIn::Welcome(welcome) => welcome,
        _ => return Err(MlsError::Rejected("message is not a welcome")),
    };
    check_ciphersuite(welcome.ciphersuite())?;
    let config = MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .max_past_epochs(PAST_EPOCHS)
        .sender_ratchet_configuration(sender_ratchet())
        .build();
    StagedWelcome::new_from_welcome(provider, &config, welcome, None).map_err(om)
}

/// A copy of every value in provider storage. Restoring it undoes all changes made since.
fn storage_values(provider: &OpenMlsRustCrypto) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
    provider
        .storage()
        .values
        .read()
        .map(|values| values.clone())
        .map_err(|_| MlsError::Rejected("storage lock poisoned"))
}

fn restore_values(provider: &OpenMlsRustCrypto, saved: HashMap<Vec<u8>, Vec<u8>>) -> Result<()> {
    let mut values = provider
        .storage()
        .values
        .write()
        .map_err(|_| MlsError::Rejected("storage lock poisoned"))?;
    *values = saved;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn legacy_plaintext_snapshot_is_refused_then_migrated() {
        let group = [9u8; 32];
        let device = DeviceKey::generate();
        let root = RootKey::generate();
        let cert = root.certify(device.id(), 1);
        let mut client = MlsClient::new(&device, &cert).unwrap();
        client.create_group(group).unwrap();

        // The plaintext form holds the device and group ids in the clear, as the old files did.
        let plain = client.plain_snapshot().unwrap();
        assert!(MlsClient::is_legacy_snapshot(&plain));
        // The field name of the group's init secret appears as text in the plaintext form.
        let marker = b"init_secret";
        assert!(contains(&plain, marker));

        let vault = Vault::ephemeral_for_tests();
        assert!(matches!(
            MlsClient::restore_sealed(&device, &cert, &vault, &plain, &[group]),
            Err(MlsError::Sealed(_))
        ));

        let sealed = MlsClient::migrate_legacy(&plain, &vault).unwrap();
        assert!(!MlsClient::is_legacy_snapshot(&sealed));
        assert!(!contains(&sealed, marker));
        let restored =
            MlsClient::restore_sealed(&device, &cert, &vault, &sealed, &[group]).unwrap();
        assert_eq!(restored.epoch(&group).unwrap(), 0);
    }

    #[test]
    fn migrate_legacy_rejects_a_sealed_blob() {
        let device = DeviceKey::generate();
        let cert = RootKey::generate().certify(device.id(), 1);
        let client = MlsClient::new(&device, &cert).unwrap();
        let vault = Vault::ephemeral_for_tests();
        let sealed = client.snapshot_sealed(&vault).unwrap();
        assert!(matches!(
            MlsClient::migrate_legacy(&sealed, &vault),
            Err(MlsError::IncompatibleSnapshot)
        ));
    }
}
