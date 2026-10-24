//! Linking extra devices to a root key, and revoking them.
//!
//! A new device signs a [`LinkRequest`] with its own key to prove possession.
//! A device holding the root key answers with a [`LinkGrant`] containing a
//! root-signed cert. Revocation is a root-signed [`DeviceRevocation`].
//! Only the root holder can grant or revoke. A linked device without the root
//! key can chat but cannot link or revoke other devices.

use crate::link;
use crate::{
    DeviceCert, DeviceId, DeviceKey, Identity, IdentityError, Result, RootId, RootKey, Signature,
};
use serde::{Deserialize, Serialize};

const LINK_DOMAIN: &[u8] = b"dissent/link-request/v1";
const REVOKE_DOMAIN: &[u8] = b"dissent/device-revoke/v1";
pub const MAX_DEVICE_NAME_CHARS: usize = 32;

// chisle: root_seed_wrapped stays None. Before enabling, seal the root seed to
// the new device's X25519 key (crypto_box) and document the trade-off.

fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.trim() != name || name.chars().count() > MAX_DEVICE_NAME_CHARS {
        return Err(IdentityError::Encoding(format!(
            "device name must be 1..={MAX_DEVICE_NAME_CHARS} characters, trimmed"
        )));
    }
    Ok(())
}

/// Sent by a new device to an existing root holder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequest {
    pub device: DeviceId,
    pub device_name: String,
    pub nonce: [u8; 16],
    pub requested_ms: u64,
    /// Signed by the new device key.
    pub sig: Signature,
}

impl LinkRequest {
    pub fn new(device: &DeviceKey, name: &str, now_ms: u64) -> Result<Self> {
        let device_name = name.trim().to_string();
        check_name(&device_name)?;
        let nonce = rand::random::<u128>().to_le_bytes();
        let mut req = Self {
            device: device.id(),
            device_name,
            nonce,
            requested_ms: now_ms,
            sig: Signature([0; 64]),
        };
        req.sig = device.sign(&req.signing_bytes());
        Ok(req)
    }

    fn signing_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(LINK_DOMAIN.len() + 64 + self.device_name.len());
        v.extend_from_slice(LINK_DOMAIN);
        v.extend_from_slice(&self.device.0);
        // Names are at most 32 chars, so at most 128 UTF-8 bytes: u16 is ample.
        v.extend_from_slice(&(self.device_name.len() as u16).to_le_bytes());
        v.extend_from_slice(self.device_name.as_bytes());
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.requested_ms.to_le_bytes());
        v
    }

    pub fn verify(&self) -> Result<()> {
        check_name(&self.device_name)?;
        self.device.verify(&self.signing_bytes(), &self.sig)
    }

    /// `dissent://link/...`
    pub fn to_link(&self) -> Result<String> {
        link::encode("link", self)
    }

    pub fn parse(s: &str) -> Result<Self> {
        link::decode("link", s)
    }
}

/// Root holder's answer to a [`LinkRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkGrant {
    pub cert: DeviceCert,
    /// Echoes the request nonce so the new device can match it.
    pub nonce: [u8; 16],
    /// Always `None` for now. See the `chisle` note above.
    pub root_seed_wrapped: Option<Vec<u8>>,
}

/// Root-signed statement that `device` must no longer be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRevocation {
    pub root: RootId,
    pub device: DeviceId,
    pub revoked_ms: u64,
    pub sig: Signature,
}

impl DeviceRevocation {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(REVOKE_DOMAIN.len() + 72);
        v.extend_from_slice(REVOKE_DOMAIN);
        v.extend_from_slice(&self.root.0);
        v.extend_from_slice(&self.device.0);
        v.extend_from_slice(&self.revoked_ms.to_le_bytes());
        v
    }

    pub fn verify(&self) -> Result<()> {
        self.root.verify(&self.signing_bytes(), &self.sig)
    }
}

impl RootKey {
    /// Certify the device named in `req`, after checking the request's signature.
    pub fn grant(&self, req: &LinkRequest, now_ms: u64) -> Result<LinkGrant> {
        req.verify()?;
        let cert = self.certify(req.device, now_ms);
        Ok(LinkGrant { cert, nonce: req.nonce, root_seed_wrapped: None })
    }

    pub fn revoke(&self, device: DeviceId, now_ms: u64) -> DeviceRevocation {
        let mut r = DeviceRevocation {
            root: self.id(),
            device,
            revoked_ms: now_ms,
            sig: Signature([0; 64]),
        };
        r.sig = self.0.sign(&r.signing_bytes());
        r
    }
}

impl Identity {
    /// Build the linked device's identity from its own pending request and the grant.
    /// The result has no root key.
    pub fn from_grant(pending: &LinkRequest, device: DeviceKey, grant: LinkGrant) -> Result<Self> {
        if device.id() != pending.device {
            return Err(IdentityError::BadCert);
        }
        if grant.nonce != pending.nonce {
            return Err(IdentityError::Encoding("grant nonce does not match request".into()));
        }
        if grant.root_seed_wrapped.is_some() {
            return Err(IdentityError::Encoding("root key transfer is not supported yet".into()));
        }
        if grant.cert.device != device.id() {
            return Err(IdentityError::BadCert);
        }
        grant.cert.verify()?;
        Ok(Self {
            root: None,
            device,
            cert: grant.cert,
            device_name: pending.device_name.clone(),
        })
    }
}
