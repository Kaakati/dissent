//! Wire envelopes. An event travels with the device cert needed to verify it.

use crate::{Error, Event, Result};
use dissent_identity::DeviceCert;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvent {
    pub event: Event,
    pub cert: DeviceCert,
}

impl SignedEvent {
    pub fn verify(&self) -> Result<()> {
        self.event.verify(&self.cert)
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("SignedEvent always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).map_err(|e| Error::Encoding(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{event, User};
    use crate::{EventId, Hlc, Payload};

    fn sample() -> (User, SignedEvent) {
        let u = User::new();
        let ev = event(
            &u,
            vec![EventId([4; 32])],
            Hlc {
                wall_ms: 9,
                counter: 1,
            },
            Payload::Join {
                display_name: "Ann".into(),
            },
        );
        let se = SignedEvent {
            event: ev,
            cert: u.cert.clone(),
        };
        (u, se)
    }

    #[test]
    fn roundtrip_and_verify() {
        let (_, se) = sample();
        se.verify().unwrap();
        let back = SignedEvent::decode(&se.encode()).unwrap();
        assert_eq!(back, se);
        back.verify().unwrap();
    }

    #[test]
    fn tamper_is_rejected() {
        let (_, se) = sample();

        let mut body = se.clone();
        body.event.body.hlc.wall_ms += 1;
        assert!(body.verify().is_err());

        let mut cert = se.clone();
        cert.cert.issued_at_ms += 1;
        assert!(cert.verify().is_err());

        let mut sig = se.clone();
        sig.event.sig.0[10] ^= 0x80;
        assert!(sig.verify().is_err());

        let other = User::new();
        let mut swapped = se.clone();
        swapped.cert = other.cert.clone();
        assert!(swapped.verify().is_err());

        assert!(SignedEvent::decode(&se.encode()[..10]).is_err());
    }
}
