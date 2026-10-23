//! Identity primitives for Dissent.
//!
//! * A user is an Ed25519 **root key**. Its public half is the [`RootId`].
//! * Each installed app instance holds a **device key**. The root key signs a
//!   [`DeviceCert`] binding the device to the root. Devices sign day-to-day
//!   traffic (events, transport); the root key is only needed to add/revoke
//!   devices and is normally kept offline (seed phrase).
//! * A community is also an Ed25519 key ([`CommunityId`]); its secret lives
//!   with the founder and is used for discovery records.
//!
//! Public IDs serialise as lowercase base32 (RFC 4648, no padding) and the
//! first 8 chars (40 bits) form the `name·keysuffix` suffix.

pub mod keystore;
pub mod mnemonic;

use ed25519_dalek::{Signature as DalekSig, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Opaque 64-byte Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(#[serde(with = "serde_bytes64")] pub [u8; 64]);

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Signature({}…)",
            data_encoding::BASE32_NOPAD.encode(&self.0[..6]).to_lowercase()
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("invalid public key")]
    InvalidKey,
    #[error("invalid signature")]
    BadSignature,
    #[error("device certificate is not signed by the claimed root")]
    BadCert,
    #[error("invalid encoding: {0}")]
    Encoding(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("serialisation: {0}")]
    Serde(String),
}

pub type Result<T> = std::result::Result<T, IdentityError>;

macro_rules! public_key_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub [u8; 32]);

        impl $name {
            pub fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
            pub fn verifying_key(&self) -> Result<VerifyingKey> {
                VerifyingKey::from_bytes(&self.0).map_err(|_| IdentityError::InvalidKey)
            }
            pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<()> {
                self.verifying_key()?
                    .verify(msg, &DalekSig::from_bytes(&sig.0))
                    .map_err(|_| IdentityError::BadSignature)
            }
            /// First 8 base32 chars = 40 bits; used as `name·suffix`.
            pub fn suffix(&self) -> String {
                self.to_string()[..8].to_string()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&data_encoding::BASE32_NOPAD.encode(&self.0).to_lowercase())
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.suffix())
            }
        }
        impl FromStr for $name {
            type Err = IdentityError;
            fn from_str(s: &str) -> Result<Self> {
                let bytes = data_encoding::BASE32_NOPAD
                    .decode(s.trim().to_ascii_uppercase().as_bytes())
                    .map_err(|e| IdentityError::Encoding(e.to_string()))?;
                let arr: [u8; 32] = bytes.try_into().map_err(|_| IdentityError::InvalidKey)?;
                VerifyingKey::from_bytes(&arr).map_err(|_| IdentityError::InvalidKey)?;
                Ok(Self(arr))
            }
        }
    };
}

public_key_newtype!(RootId, "Public half of a user's root key. The user's identity.");
public_key_newtype!(DeviceId, "Public half of a device key. Also the iroh node id of that device.");
public_key_newtype!(CommunityId, "Public half of a community key. The community's address.");

/// Shared secret-key wrapper. `Debug` hides the seed.
#[derive(Clone)]
pub struct SecretKey(SigningKey);

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl SecretKey {
    pub fn generate() -> Self {
        let seed: [u8; 32] = rand::random();
        Self::from_seed(seed)
    }
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }
    pub fn seed(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
    pub fn public_bytes(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    pub fn sign(&self, msg: &[u8]) -> Signature {
        Signature(self.0.sign(msg).to_bytes())
    }
}

/// A user's root key.
#[derive(Clone, Debug)]
pub struct RootKey(pub SecretKey);
/// A device key. Its seed doubles as the iroh transport secret.
#[derive(Clone, Debug)]
pub struct DeviceKey(pub SecretKey);
/// A community key.
#[derive(Clone, Debug)]
pub struct CommunityKey(pub SecretKey);

impl RootKey {
    pub fn generate() -> Self {
        Self(SecretKey::generate())
    }
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SecretKey::from_seed(seed))
    }
    pub fn id(&self) -> RootId {
        RootId(self.0.public_bytes())
    }
    /// Issue a certificate binding `device` to this root.
    pub fn certify(&self, device: DeviceId, issued_at_ms: u64) -> DeviceCert {
        let mut cert = DeviceCert { root: self.id(), device, issued_at_ms, sig: Signature([0; 64]) };
        cert.sig = self.0.sign(&cert.signing_bytes());
        cert
    }
}
impl DeviceKey {
    pub fn generate() -> Self {
        Self(SecretKey::generate())
    }
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SecretKey::from_seed(seed))
    }
    pub fn id(&self) -> DeviceId {
        DeviceId(self.0.public_bytes())
    }
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.0.sign(msg)
    }
}
impl CommunityKey {
    pub fn generate() -> Self {
        Self(SecretKey::generate())
    }
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SecretKey::from_seed(seed))
    }
    pub fn id(&self) -> CommunityId {
        CommunityId(self.0.public_bytes())
    }
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.0.sign(msg)
    }
}

/// Root-signed statement "this device belongs to this user".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCert {
    pub root: RootId,
    pub device: DeviceId,
    pub issued_at_ms: u64,
    pub sig: Signature,
}

impl DeviceCert {
    const DOMAIN: &'static [u8] = b"dissent/device-cert/v1";
    fn signing_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(Self::DOMAIN.len() + 72);
        v.extend_from_slice(Self::DOMAIN);
        v.extend_from_slice(&self.root.0);
        v.extend_from_slice(&self.device.0);
        v.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        v
    }
    pub fn verify(&self) -> Result<()> {
        self.root.verify(&self.signing_bytes(), &self.sig).map_err(|_| IdentityError::BadCert)
    }
}

/// Community-key-signed statement "this community is owned by this root and named so".
/// Carried in invites and sync so joiners learn the owner without trusting a peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    pub community: CommunityId,
    pub owner: RootId,
    pub name: String,
    pub created_ms: u64,
    pub sig: Signature,
}

impl Genesis {
    const DOMAIN: &'static [u8] = b"dissent/genesis/v1";
    fn signing_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(Self::DOMAIN.len() + 80 + self.name.len());
        v.extend_from_slice(Self::DOMAIN);
        v.extend_from_slice(&self.community.0);
        v.extend_from_slice(&self.owner.0);
        v.extend_from_slice(&self.created_ms.to_le_bytes());
        v.extend_from_slice(self.name.as_bytes());
        v
    }
    pub fn create(key: &CommunityKey, owner: RootId, name: String, created_ms: u64) -> Self {
        let mut g = Genesis { community: key.id(), owner, name, created_ms, sig: Signature([0; 64]) };
        g.sig = key.sign(&g.signing_bytes());
        g
    }
    pub fn verify(&self) -> Result<()> {
        self.community.verify(&self.signing_bytes(), &self.sig).map_err(|_| IdentityError::BadSignature)
    }
}

/// Everything one app install needs: the device key, its cert, and
/// (optionally, while unlocked) the root key.
#[derive(Clone, Debug)]
pub struct Identity {
    pub root: Option<RootKey>,
    pub device: DeviceKey,
    pub cert: DeviceCert,
}

impl Identity {
    /// Fresh user on a fresh device.
    pub fn create(now_ms: u64) -> Self {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), now_ms);
        Self { root: Some(root), device, cert }
    }
    pub fn root_id(&self) -> RootId {
        self.cert.root
    }
    pub fn device_id(&self) -> DeviceId {
        self.device.id()
    }
}

mod serde_bytes64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(b: &[u8; 64], s: S) -> std::result::Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            data_encoding::BASE64URL_NOPAD.encode(b).serialize(s)
        } else {
            serde::Serialize::serialize(&b[..], s)
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<[u8; 64], D::Error> {
        let v: Vec<u8> = if d.is_human_readable() {
            let s = String::deserialize(d)?;
            data_encoding::BASE64URL_NOPAD.decode(s.as_bytes()).map_err(serde::de::Error::custom)?
        } else {
            Vec::<u8>::deserialize(d)?
        };
        v.try_into().map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_roundtrip_and_tamper() {
        let key = CommunityKey::generate();
        let g = Genesis::create(&key, RootKey::generate().id(), "rust".into(), 5);
        g.verify().unwrap();
        let mut bad = g.clone();
        bad.name = "evil".into();
        assert!(bad.verify().is_err());
    }

    #[test]
    fn cert_roundtrip_and_tamper() {
        let id = Identity::create(1);
        id.cert.verify().unwrap();
        let mut bad = id.cert.clone();
        bad.issued_at_ms = 2;
        assert!(bad.verify().is_err());
    }

    #[test]
    fn id_display_parse() {
        let r = RootKey::generate().id();
        let s = r.to_string();
        assert_eq!(s.len(), 52);
        assert_eq!(r, s.parse::<RootId>().unwrap());
        assert_eq!(r.suffix().len(), 8);
    }

    #[test]
    fn signature_serde_json_and_postcard() {
        let k = DeviceKey::generate();
        let sig = k.sign(b"hi");
        let j = serde_json::to_string(&sig).unwrap();
        assert_eq!(sig, serde_json::from_str::<Signature>(&j).unwrap());
        let p = postcard::to_allocvec(&sig).unwrap();
        assert_eq!(sig, postcard::from_bytes::<Signature>(&p).unwrap());
    }
}
