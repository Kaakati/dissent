//! The SFrame layer (RFC 9605). Every media frame is encrypted here, before str0m packetizes it.
//!
//! The key id is the MLS member index of the sender, so a receiver learns who sent a frame from
//! the authenticated header, and every sender gets its own key without any coordination.

use sframe::{
    frame::{EncryptedFrameView, MediaFrame, MonotonicCounter},
    key::{DecryptionKey, EncryptionKey},
    mls::{MlsKeyId, MlsKeyIdBitRange},
    CipherSuite,
};

use crate::error::{MediaError, Result};
use crate::key::MediaKey;

/// The RFC 9605 cipher suite used for every media frame.
pub const CIPHER_SUITE: CipherSuite = CipherSuite::AesGcm256Sha512;

/// Epoch bits in the key id. The spike always uses epoch 0.
const EPOCH_BITS: u8 = 8;
/// Member index bits in the key id.
const INDEX_BITS: u8 = 24;

/// The largest sender id a key id can carry.
pub const MAX_SENDER_ID: u64 = (1 << INDEX_BITS) - 1;

fn bit_range() -> MlsKeyIdBitRange {
    MlsKeyIdBitRange::new(EPOCH_BITS, INDEX_BITS)
}

/// Encrypts the frames a single local sender produces.
pub(crate) struct SframeSender {
    key: EncryptionKey,
    counter: MonotonicCounter,
}

impl SframeSender {
    /// Derives the sending key of `sender_id` from the call's media key.
    pub(crate) fn new(media_key: &MediaKey, sender_id: u64) -> Result<Self> {
        if sender_id > MAX_SENDER_ID {
            return Err(MediaError::SenderIdOutOfRange(sender_id));
        }
        let key_id = MlsKeyId::try_new(0u64, 0u64, sender_id, bit_range())?;
        let key = EncryptionKey::derive_from_mls(CIPHER_SUITE, media_key, key_id)?;
        Ok(Self {
            key,
            counter: MonotonicCounter::default(),
        })
    }

    /// Encrypts one plaintext frame. The output is the SFrame header followed by the ciphertext.
    pub(crate) fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let frame = MediaFrame::try_new(&mut self.counter, plaintext)?;
        let encrypted = frame.encrypt(&self.key)?;
        Ok(encrypted.as_ref().to_vec())
    }
}

/// Decrypts incoming frames. It derives the key of each sender from the key id in the header.
pub(crate) struct SframeReceiver {
    media_key: MediaKey,
}

impl SframeReceiver {
    pub(crate) fn new(media_key: MediaKey) -> Self {
        Self { media_key }
    }

    /// Returns the sender id and the plaintext. Fails when the frame does not decrypt under
    /// this call's key, which covers a wrong key, tampering and malformed frames.
    pub(crate) fn decrypt(&self, encrypted: &[u8]) -> Result<(u64, Vec<u8>)> {
        let view = EncryptedFrameView::try_new(encrypted)?;
        let key_id = MlsKeyId::from_key_id(view.header().key_id(), bit_range());
        let key = DecryptionKey::derive_from_mls(CIPHER_SUITE, &self.media_key, key_id)?;
        let frame = view.decrypt(&key)?;
        Ok((key_id.member_index(), frame.payload().to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> MediaKey {
        MediaKey::new([byte; 32])
    }

    #[test]
    fn cipher_suite_takes_a_32_byte_key() {
        // MediaKey exports exactly 32 bytes, which must match the suite's key length.
        assert_eq!(CIPHER_SUITE.key_len(), MediaKey::LEN);
    }

    #[test]
    fn round_trips_and_names_the_sender() {
        let plaintext: Vec<u8> = (0..160).map(|i| i as u8).collect();
        let mut sender = SframeSender::new(&key(7), 3).unwrap();
        let encrypted = sender.encrypt(&plaintext).unwrap();

        let receiver = SframeReceiver::new(key(7));
        let (from, decrypted) = receiver.decrypt(&encrypted).unwrap();
        assert_eq!(from, 3);
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn ciphertext_differs_from_plaintext() {
        let plaintext = vec![0x41u8; 160];
        let mut sender = SframeSender::new(&key(9), 1).unwrap();
        let encrypted = sender.encrypt(&plaintext).unwrap();

        // The payload is not visible in the wire bytes.
        assert!(encrypted
            .windows(plaintext.len())
            .all(|w| w != plaintext.as_slice()));
        assert!(!encrypted.windows(32).any(|w| w.iter().all(|&b| b == 0x41)));
        assert_ne!(&encrypted[encrypted.len() - 160..], plaintext.as_slice());
    }

    #[test]
    fn same_plaintext_gives_distinct_ciphertexts() {
        let plaintext = vec![5u8; 160];
        let mut sender = SframeSender::new(&key(9), 1).unwrap();
        let a = sender.encrypt(&plaintext).unwrap();
        let b = sender.encrypt(&plaintext).unwrap();
        assert_ne!(a, b, "the frame counter must make each encryption unique");
    }

    #[test]
    fn wrong_key_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2).unwrap();
        let encrypted = sender.encrypt(&[0u8; 160]).unwrap();
        let receiver = SframeReceiver::new(key(2));
        assert!(receiver.decrypt(&encrypted).is_err());
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2).unwrap();
        let mut encrypted = sender.encrypt(&[0u8; 160]).unwrap();
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0x01;
        let receiver = SframeReceiver::new(key(1));
        assert!(receiver.decrypt(&encrypted).is_err());
    }

    #[test]
    fn sender_id_must_fit_the_key_id() {
        assert!(SframeSender::new(&key(1), MAX_SENDER_ID).is_ok());
        assert!(matches!(
            SframeSender::new(&key(1), MAX_SENDER_ID + 1),
            Err(MediaError::SenderIdOutOfRange(_))
        ));
    }
}
