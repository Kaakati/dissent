//! Direct messages. 1:1, signed by the sending device, carried with the sender's cert.

use crate::event::digest_id;
use crate::{Error, Hlc, Result};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};

const ID_DOMAIN: &[u8] = b"dissent/dm/v1";
const SIG_DOMAIN: &[u8] = b"dissent/dm-sig/v1";

digest_id!(DmId, "Hash of a DM body.");

impl DmId {
    pub fn of(body: &DmBody) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(ID_DOMAIN);
        h.update(&postcard::to_allocvec(body).expect("DmBody always encodes"));
        Self(*h.finalize().as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmBody {
    pub from: RootId,
    pub to: RootId,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmMessage {
    pub body: DmBody,
    pub id: DmId,
    pub sig: Signature,
}

impl DmMessage {
    /// `body.device` must equal `device.id()`; otherwise [`DmMessage::verify`] fails.
    pub fn sign(body: DmBody, device: &DeviceKey) -> Self {
        debug_assert_eq!(body.device, device.id());
        let id = DmId::of(&body);
        let sig = device.sign(&sig_msg(&id));
        Self { body, id, sig }
    }

    /// Checks the id, the cert, that the cert binds `body.device` to `body.from`, and the signature.
    pub fn verify(&self, cert: &DeviceCert) -> Result<()> {
        if DmId::of(&self.body) != self.id {
            return Err(Error::IdMismatch);
        }
        cert.verify()?;
        if cert.device != self.body.device {
            return Err(Error::DeviceMismatch);
        }
        if cert.root != self.body.from {
            return Err(Error::AuthorMismatch);
        }
        self.body.device.verify(&sig_msg(&self.id), &self.sig)?;
        Ok(())
    }
}

fn sig_msg(id: &DmId) -> Vec<u8> {
    [SIG_DOMAIN, id.0.as_slice()].concat()
}

/// A DM plus the sender's device cert, as sent over the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDm {
    pub dm: DmMessage,
    pub cert: DeviceCert,
}

impl SignedDm {
    pub fn verify(&self) -> Result<()> {
        self.dm.verify(&self.cert)
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("SignedDm always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).map_err(|e| Error::Encoding(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::User;

    fn sample() -> (User, User, DmMessage) {
        let alice = User::new();
        let bob = User::new();
        let body = DmBody {
            from: alice.id(),
            to: bob.id(),
            device: alice.device.id(),
            hlc: Hlc {
                wall_ms: 3,
                counter: 0,
            },
            text: "hi bob".into(),
        };
        let dm = DmMessage::sign(body, &alice.device);
        (alice, bob, dm)
    }

    #[test]
    fn sign_verify_and_id_roundtrip() {
        let (alice, _, dm) = sample();
        dm.verify(&alice.cert).unwrap();
        let s = dm.id.to_string();
        assert_eq!(s.len(), 52);
        assert_eq!(s.parse::<DmId>().unwrap(), dm.id);
    }

    #[test]
    fn tampering_is_detected() {
        let (alice, bob, dm) = sample();

        let mut text = dm.clone();
        text.body.text = "hi eve".into();
        assert!(matches!(text.verify(&alice.cert), Err(Error::IdMismatch)));

        let mut sig = dm.clone();
        sig.sig.0[0] ^= 1;
        assert!(matches!(sig.verify(&alice.cert), Err(Error::Identity(_))));

        // Cert for a different root than `from`.
        let rogue = bob.root.certify(alice.device.id(), 0);
        assert!(matches!(dm.verify(&rogue), Err(Error::AuthorMismatch)));

        // Valid cert, but for another device.
        assert!(matches!(dm.verify(&bob.cert), Err(Error::DeviceMismatch)));
    }

    #[test]
    fn signed_dm_roundtrip_and_tamper() {
        let (alice, _, dm) = sample();
        let sd = SignedDm {
            dm,
            cert: alice.cert.clone(),
        };
        sd.verify().unwrap();
        let back = SignedDm::decode(&sd.encode()).unwrap();
        assert_eq!(back, sd);
        back.verify().unwrap();

        let mut bad = sd.clone();
        bad.cert.issued_at_ms += 1;
        assert!(bad.verify().is_err());
        assert!(SignedDm::decode(&sd.encode()[..5]).is_err());
    }
}
