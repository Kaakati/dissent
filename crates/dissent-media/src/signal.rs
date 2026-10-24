//! Signalling messages. The caller carries them over any transport as opaque bytes.

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One signalling message between two peers of a call.
///
/// The SDP and candidate strings are the standard WebRTC text forms. The transport only sees
/// the bytes from [`Signal::to_bytes`], so it needs no knowledge of WebRTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    /// The caller's session description offer.
    Offer(String),
    /// The callee's session description answer.
    Answer(String),
    /// A remote ICE candidate in SDP attribute form. Host candidates ride in the offer and answer,
    /// so the spike never sends this variant itself.
    Candidate(String),
}

impl Signal {
    /// Encodes the message with postcard, the wire format for signalling.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(postcard::to_allocvec(self)?)
    }

    /// Decodes a message produced by [`Signal::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;

    #[test]
    fn signals_round_trip_through_bytes() {
        for signal in [
            Signal::Offer("v=0\r\n".to_string()),
            Signal::Answer("v=0\r\nanswer".to_string()),
            Signal::Candidate("candidate:1 1 udp 2130706431 127.0.0.1 9 typ host".to_string()),
        ] {
            let bytes = signal.to_bytes().unwrap();
            assert_eq!(Signal::from_bytes(&bytes).unwrap(), signal);
        }
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(Signal::from_bytes(&[0xff, 0xff, 0xff]).is_err());
    }
}
