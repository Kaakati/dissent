//! Social recovery. A lost root key is replaced by rotating to a new one.
//!
//! No secret is reconstructed or moved. The new root key signs to prove it
//! is held by the person doing the recovery. Authority to rotate comes either
//! from the old root key (voluntary rotation) or from `threshold` distinct
//! guardians who each sign the same [`RotationRequest`].

use crate::link;
use crate::{IdentityError, Result, RootId, RootKey, Signature};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_GUARDIANS: usize = 16;

const SET_DOMAIN: &[u8] = b"dissent/guardians/v1";
const REQUEST_DOMAIN: &[u8] = b"dissent/rotation-request/v1";
const APPROVAL_DOMAIN: &[u8] = b"dissent/rotation-approval/v1";
const OWNER_DOMAIN: &[u8] = b"dissent/rotation-owner/v1";
const NEW_ROOT_DOMAIN: &[u8] = b"dissent/rotation-new-root/v1";

fn recovery_err(msg: impl Into<String>) -> IdentityError {
    IdentityError::Recovery(msg.into())
}

/// Bytes a signer over `request` covers: a domain tag followed by the request body.
fn over_request(domain: &[u8], request: &RotationRequest) -> Vec<u8> {
    [domain, request.bytes().as_slice()].concat()
}

/// Owner's signed list of guardians and the threshold needed to rotate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardianSet {
    pub owner: RootId,
    /// Sorted, unique, 1..=[`MAX_GUARDIANS`] entries.
    pub guardians: Vec<RootId>,
    /// 1..=guardians.len().
    pub threshold: u8,
    pub created_ms: u64,
    pub sig: Signature,
}

fn check_policy(owner: &RootId, guardians: &[RootId], threshold: u8) -> Result<()> {
    if guardians.is_empty() || guardians.len() > MAX_GUARDIANS {
        return Err(recovery_err(format!("guardian count must be 1..={MAX_GUARDIANS}")));
    }
    if guardians.windows(2).any(|w| w[0] >= w[1]) {
        return Err(recovery_err("guardians must be sorted and unique"));
    }
    if guardians.contains(owner) {
        return Err(recovery_err("owner cannot be its own guardian"));
    }
    if threshold == 0 || threshold as usize > guardians.len() {
        return Err(recovery_err("threshold must be 1..=guardian count"));
    }
    Ok(())
}

impl GuardianSet {
    /// Sorts and dedups `guardians`, validates the policy, and signs it with `root`.
    pub fn create(root: &RootKey, mut guardians: Vec<RootId>, threshold: u8, now_ms: u64) -> Result<Self> {
        guardians.sort();
        guardians.dedup();
        let owner = root.id();
        check_policy(&owner, &guardians, threshold)?;
        let mut set = Self { owner, guardians, threshold, created_ms: now_ms, sig: Signature([0; 64]) };
        set.sig = root.0.sign(&set.signing_bytes());
        Ok(set)
    }

    fn signing_bytes(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(32 * (self.guardians.len() + 1) + 9);
        body.extend_from_slice(&self.owner.0);
        for g in &self.guardians {
            body.extend_from_slice(&g.0);
        }
        body.push(self.threshold);
        body.extend_from_slice(&self.created_ms.to_le_bytes());
        [SET_DOMAIN, body.as_slice()].concat()
    }

    pub fn verify(&self) -> Result<()> {
        check_policy(&self.owner, &self.guardians, self.threshold)?;
        self.owner.verify(&self.signing_bytes(), &self.sig)
    }

    /// `dissent://guardians/...`. Shared with the recovering device, which needs it to prove the
    /// threshold. The list is public and signed, so the link carries no secret.
    pub fn to_link(&self) -> Result<String> {
        link::encode("guardians", self)
    }

    pub fn parse(s: &str) -> Result<Self> {
        link::decode("guardians", s)
    }
}

/// A request to move from `old_root` to `new_root`. Unsigned: the new device
/// shows it to guardians (as a link or QR), and each signs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationRequest {
    pub old_root: RootId,
    pub new_root: RootId,
    pub requested_ms: u64,
}

impl RotationRequest {
    /// Request made from a new device that holds `new_root`.
    pub fn new_for(new_root: &RootKey, old_root: RootId, now_ms: u64) -> Self {
        Self { old_root, new_root: new_root.id(), requested_ms: now_ms }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(REQUEST_DOMAIN.len() + 72);
        v.extend_from_slice(REQUEST_DOMAIN);
        v.extend_from_slice(&self.old_root.0);
        v.extend_from_slice(&self.new_root.0);
        v.extend_from_slice(&self.requested_ms.to_le_bytes());
        v
    }

    /// `dissent://recover/...`
    pub fn to_link(&self) -> Result<String> {
        link::encode("recover", self)
    }

    pub fn parse(s: &str) -> Result<Self> {
        link::decode("recover", s)
    }
}

/// One guardian's signature over a [`RotationRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardianApproval {
    pub request: RotationRequest,
    pub guardian: RootId,
    pub sig: Signature,
}

impl GuardianApproval {
    pub fn sign(guardian: &RootKey, request: RotationRequest) -> Self {
        let sig = guardian.0.sign(&over_request(APPROVAL_DOMAIN, &request));
        Self { request, guardian: guardian.id(), sig }
    }

    /// Checks that this approval is for `request` and signed by `self.guardian`.
    pub fn verify_for(&self, request: &RotationRequest) -> Result<()> {
        if &self.request != request {
            return Err(recovery_err("approval is for a different request"));
        }
        self.guardian.verify(&over_request(APPROVAL_DOMAIN, request), &self.sig)
    }

    /// `dissent://approve/...`
    pub fn to_link(&self) -> Result<String> {
        link::encode("approve", self)
    }

    pub fn parse(s: &str) -> Result<Self> {
        link::decode("approve", s)
    }
}

/// Who authorised a rotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RotationProof {
    /// Signed directly by the old root key.
    Owner { sig: Signature },
    /// At least `set.threshold` distinct guardians approved.
    Guardians { set: GuardianSet, approvals: Vec<GuardianApproval> },
}

/// A complete, self-verifying rotation from `request.old_root` to `request.new_root`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRotation {
    pub request: RotationRequest,
    pub proof: RotationProof,
    /// Signed by the new root key, proving possession of it.
    pub new_root_sig: Signature,
}

impl KeyRotation {
    /// Voluntary rotation. `old` signs the request directly.
    pub fn by_owner(old: &RootKey, new: &RootKey, now_ms: u64) -> Self {
        let request = RotationRequest::new_for(new, old.id(), now_ms);
        let sig = old.0.sign(&over_request(OWNER_DOMAIN, &request));
        let new_root_sig = new.0.sign(&over_request(NEW_ROOT_DOMAIN, &request));
        Self { request, proof: RotationProof::Owner { sig }, new_root_sig }
    }

    /// Guardian-approved rotation, assembled on the new device. `new` must be
    /// the root key named in `request`. Call [`KeyRotation::verify`] before trusting it.
    pub fn by_guardians(
        new: &RootKey,
        set: GuardianSet,
        request: RotationRequest,
        approvals: Vec<GuardianApproval>,
    ) -> Result<Self> {
        if new.id() != request.new_root {
            return Err(recovery_err("new root key does not match the request"));
        }
        let new_root_sig = new.0.sign(&over_request(NEW_ROOT_DOMAIN, &request));
        Ok(Self { request, proof: RotationProof::Guardians { set, approvals }, new_root_sig })
    }

    pub fn verify(&self) -> Result<()> {
        let req = &self.request;
        if req.old_root == req.new_root {
            return Err(recovery_err("new root equals old root"));
        }
        req.new_root.verify(&over_request(NEW_ROOT_DOMAIN, req), &self.new_root_sig)?;
        match &self.proof {
            RotationProof::Owner { sig } => req.old_root.verify(&over_request(OWNER_DOMAIN, req), sig),
            RotationProof::Guardians { set, approvals } => verify_guardians(set, approvals, req),
        }
    }
}

fn verify_guardians(set: &GuardianSet, approvals: &[GuardianApproval], req: &RotationRequest) -> Result<()> {
    set.verify()?;
    if set.owner != req.old_root {
        return Err(recovery_err("guardian set belongs to a different root"));
    }
    // Count distinct guardians. Non-guardians, duplicates and bad signatures are ignored.
    let mut signers = BTreeSet::new();
    for a in approvals {
        if set.guardians.binary_search(&a.guardian).is_ok() && a.verify_for(req).is_ok() {
            signers.insert(a.guardian);
        }
    }
    if signers.len() < set.threshold as usize {
        return Err(recovery_err(format!(
            "{} valid approvals, {} required",
            signers.len(),
            set.threshold
        )));
    }
    Ok(())
}
