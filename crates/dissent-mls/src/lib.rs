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
//! Group ids are the 32-byte community ids. Group state lives in memory for now;
//! see the `// chisle:` note on [`MlsClient`].

use std::collections::HashMap;
use std::fmt::Display;

use dissent_identity::{DeviceCert, DeviceId, DeviceKey};
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

/// The single ciphersuite used by Dissent groups.
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

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

/// One device's MLS state: the OpenMLS provider, its signing key and its groups.
///
/// // chisle: in-memory storage only. Groups and key material are lost on
/// restart. Persisting needs a serialisable or SQLite-backed OpenMLS storage
/// provider and a store for `MlsGroup::load`.
pub struct MlsClient {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    groups: HashMap<[u8; 32], MlsGroup>,
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
        })
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

    /// Creates an empty group whose id is the community id.
    pub fn create_group(&mut self, group: [u8; 32]) -> Result<()> {
        if self.groups.contains_key(&group) {
            return Err(MlsError::GroupExists);
        }
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
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

    /// Joins the group named by a welcome. Every member credential in the group must verify.
    pub fn join_from_welcome(&mut self, welcome: &[u8]) -> Result<[u8; 32]> {
        let message = MlsMessageIn::tls_deserialize_exact(welcome).map_err(om)?;
        let welcome = match message.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => return Err(MlsError::Rejected("message is not a welcome")),
        };
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build();
        let staged =
            StagedWelcome::new_from_welcome(&self.provider, &config, welcome, None).map_err(om)?;

        let group_id = fixed_group_id(staged.group_context().group_id().as_slice())?;
        if self.groups.contains_key(&group_id) {
            return Err(MlsError::GroupExists);
        }
        for member in staged.members() {
            verify_member(&member.credential, &member.signature_key)?;
        }
        let mls_group = staged.into_group(&self.provider).map_err(om)?;
        self.groups.insert(group_id, mls_group);
        Ok(group_id)
    }

    /// Processes an incoming MLS message for `group`.
    ///
    /// Commits are checked before they are merged, so a rejected commit leaves
    /// the group unchanged.
    pub fn process(&mut self, group: &[u8; 32], message: &[u8]) -> Result<Processed> {
        let mls_group = self.groups.get_mut(group).ok_or(MlsError::UnknownGroup)?;
        let (processed, evicted) = process_in_group(&self.provider, mls_group, message)?;
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

/// Processes one message against a group, without touching the group map.
/// Returns the outcome and whether this device was evicted.
fn process_in_group(
    provider: &OpenMlsRustCrypto,
    mls_group: &mut MlsGroup,
    message: &[u8],
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
            check_commit(&staged)?;
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
    verify_leaf(key_package.leaf_node())?;
    Ok(key_package)
}

/// Verifies a leaf node's credential and that its signature key is the certified device.
fn verify_leaf(leaf: &LeafNode) -> Result<()> {
    verify_member(leaf.credential(), leaf.signature_key().as_slice()).map(|_| ())
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

/// Checks every new credential a staged commit introduces, before it is merged.
fn check_commit(staged: &StagedCommit) -> Result<()> {
    if let Some(leaf) = staged.update_path_leaf_node() {
        verify_leaf(leaf)?;
    }
    for proposal in staged.queued_proposals() {
        check_proposal(proposal)?;
    }
    Ok(())
}

fn member_device(mls_group: &MlsGroup, leaf: LeafNodeIndex) -> Result<DeviceId> {
    let member = mls_group
        .members()
        .find(|m| m.index == leaf)
        .ok_or(MlsError::NotAMember)?;
    let cert = verify_member(&member.credential, &member.signature_key)?;
    Ok(cert.device)
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
