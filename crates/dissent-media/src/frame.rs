//! The per-frame stamp that travels, encrypted and authenticated, with every audio frame.
//!
//! Each frame's encrypted payload starts with a stamp: the sender's frame sequence number, its
//! 48 kHz sample timestamp, and the audio level. Forwarders never open the payload, so they carry
//! the stamp unchanged. Receivers read it after decryption, and a lost frame's stamp comes back
//! with its redundant copy.

use crate::error::{MediaError, Result};

/// Samples in one frame at 48 kHz: 20 ms, the Opus frame length calls use.
pub const FRAME_SAMPLES: u32 = 960;

/// Bytes of stamp: sequence number (u32, little endian), timestamp (u32, little endian), and the
/// audio level (u8).
pub(crate) const STAMP_LEN: usize = 9;

/// A decrypted audio frame and its place in the sender's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFrame {
    /// The sender's id.
    pub sender: u64,
    /// The sender's frame count, starting at 0 and wrapping at 2^32. It keeps counting across a
    /// rekey, and across a forwarder failover, so consecutive frames differ by one.
    pub seq: u32,
    /// The sender's sample clock at capture, in samples at 48 kHz, wrapping at 2^32. Frames sent
    /// without an explicit timestamp are [`FRAME_SAMPLES`] apart.
    pub timestamp: u32,
    /// The RFC 6464 audio level the sender set: 0 is loudest, 127 is silence.
    pub audio_level: u8,
    /// The Opus payload.
    pub payload: Vec<u8>,
}

/// Builds the stamp that precedes a frame's payload.
pub(crate) fn stamp(seq: u32, timestamp: u32, audio_level: u8) -> [u8; STAMP_LEN] {
    let mut out = [0u8; STAMP_LEN];
    out[..4].copy_from_slice(&seq.to_le_bytes());
    out[4..8].copy_from_slice(&timestamp.to_le_bytes());
    out[8] = audio_level;
    out
}

/// Reads the stamp at the front of a decrypted frame, and returns the payload after it.
fn split(stamped: &[u8]) -> Result<(u32, u32, u8, &[u8])> {
    if stamped.len() < STAMP_LEN {
        return Err(MediaError::MalformedFrame);
    }
    let seq = u32::from_le_bytes([stamped[0], stamped[1], stamped[2], stamped[3]]);
    let timestamp = u32::from_le_bytes([stamped[4], stamped[5], stamped[6], stamped[7]]);
    Ok((seq, timestamp, stamped[8], &stamped[STAMP_LEN..]))
}

/// Turns a decrypted, stamped frame from `sender` into a [`ReceivedFrame`].
pub(crate) fn received(sender: u64, stamped: &[u8]) -> Result<ReceivedFrame> {
    let (seq, timestamp, audio_level, payload) = split(stamped)?;
    Ok(ReceivedFrame {
        sender,
        seq,
        timestamp,
        audio_level,
        payload: payload.to_vec(),
    })
}

// Returns the payload of a decrypted, stamped frame, without the stamp.
#[cfg(test)]
pub(crate) fn payload_of_stamp(stamped: &[u8]) -> Result<&[u8]> {
    split(stamped).map(|(_, _, _, payload)| payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips() {
        let mut bytes = stamp(7, 0xDEAD_BEEF, 42).to_vec();
        bytes.extend_from_slice(b"opus");
        let frame = received(9, &bytes).unwrap();
        assert_eq!(
            frame,
            ReceivedFrame {
                sender: 9,
                seq: 7,
                timestamp: 0xDEAD_BEEF,
                audio_level: 42,
                payload: b"opus".to_vec(),
            }
        );
    }

    #[test]
    fn short_frames_are_malformed() {
        assert!(matches!(
            received(1, &[0u8; STAMP_LEN - 1]),
            Err(MediaError::MalformedFrame)
        ));
    }
}
