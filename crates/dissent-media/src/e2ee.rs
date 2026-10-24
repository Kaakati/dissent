//! The SFrame layer (RFC 9605). Every media frame is encrypted here, before str0m packetizes it.
//!
//! The key id holds the MLS member index of the sender and the low bits of the MLS epoch. A
//! receiver therefore learns who sent a frame, and which epoch's key it needs, from the header.
//! Every sender gets its own key without any coordination.
//!
//! Each wire frame starts with one unencrypted audio-level byte, as in RFC 6464. It is SFrame
//! metadata: a forwarder can read it without a key, and any change to it breaks decryption.
//!
//! Replay checks and sender authentication are not here. They live in `auth`, because they must
//! see every link of a participant together.

use std::time::{Duration, Instant};

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

/// Epoch bits in the key id: the low bits of the MLS epoch.
const EPOCH_BITS: u8 = 8;
/// Member index bits in the key id.
const INDEX_BITS: u8 = 24;

/// The largest sender id a key id can carry.
pub const MAX_SENDER_ID: u64 = (1 << INDEX_BITS) - 1;

/// How long frames of the previous epoch still decrypt after a rekey.
pub(crate) const EPOCH_GRACE: Duration = Duration::from_secs(2);

/// Audio level stamped on frames from paths that do not measure it. 127 is silence in RFC 6464.
pub(crate) const DEFAULT_LEVEL: u8 = 127;

/// Width of the replay window, in frames. A frame more than this many behind the newest one
/// verified from its sender is too old to accept.
pub(crate) const REPLAY_WINDOW: u64 = 1024;

fn bit_range() -> MlsKeyIdBitRange {
    MlsKeyIdBitRange::new(EPOCH_BITS, INDEX_BITS)
}

/// The epoch bits of `epoch`, as they appear in a key id.
pub(crate) fn epoch_lsb(epoch: u64) -> u64 {
    epoch & ((1 << EPOCH_BITS) - 1)
}

/// The audio level a wire frame carries, read without any key.
pub(crate) fn audio_level(wire: &[u8]) -> Option<u8> {
    wire.first().copied()
}

/// The SFrame counter of a wire frame, read without any key.
pub(crate) fn peek_counter(wire: &[u8]) -> Option<u64> {
    let rest = wire.get(1..)?;
    let view = EncryptedFrameView::try_with_meta_data(rest, &wire[..1]).ok()?;
    Some(view.header().counter())
}

/// A sliding replay window over the SFrame counter of one sender in one epoch.
///
/// It remembers the highest counter seen, and a bitmap of which of the `REPLAY_WINDOW` counters
/// at or below it have arrived. Bit `i` stands for the counter `highest - i`.
#[derive(Default)]
pub(crate) struct ReplayWindow {
    seen_any: bool,
    highest: u64,
    bits: [u64; (REPLAY_WINDOW / 64) as usize],
}

impl ReplayWindow {
    /// Whether a frame with `counter` may be accepted: it is newer than anything seen, or inside
    /// the window and not seen yet.
    pub(crate) fn accepts(&self, counter: u64) -> bool {
        if !self.seen_any || counter > self.highest {
            return true;
        }
        let back = self.highest - counter;
        back < REPLAY_WINDOW && !self.bit(back)
    }

    /// Records a frame that has been accepted.
    pub(crate) fn accept(&mut self, counter: u64) {
        if !self.seen_any {
            self.seen_any = true;
            self.highest = counter;
            self.set(0);
        } else if counter > self.highest {
            self.shift(counter - self.highest);
            self.highest = counter;
            self.set(0);
        } else {
            let back = self.highest - counter;
            if back < REPLAY_WINDOW {
                self.set(back);
            }
        }
    }

    fn bit(&self, index: u64) -> bool {
        (self.bits[(index / 64) as usize] >> (index % 64)) & 1 == 1
    }

    fn set(&mut self, index: u64) {
        self.bits[(index / 64) as usize] |= 1 << (index % 64);
    }

    /// Moves every recorded bit `by` places further back, as `highest` advances.
    fn shift(&mut self, by: u64) {
        if by >= REPLAY_WINDOW {
            self.bits = [0; (REPLAY_WINDOW / 64) as usize];
            return;
        }
        let words = (by / 64) as usize;
        let rem = (by % 64) as u32;
        let old = self.bits;
        for (word, slot) in self.bits.iter_mut().enumerate() {
            let Some(src) = word.checked_sub(words) else {
                *slot = 0;
                continue;
            };
            let mut value = old[src] << rem;
            if rem > 0 && src >= 1 {
                value |= old[src - 1] >> (64 - rem);
            }
            *slot = value;
        }
    }
}

/// Encrypts the frames a local sender produces, under one epoch of the media key.
pub(crate) struct SframeSender {
    key: EncryptionKey,
    counter: MonotonicCounter,
}

impl SframeSender {
    /// Derives the sending key of `sender_id` for `epoch` from the media key.
    pub(crate) fn new(media_key: &MediaKey, sender_id: u64, epoch: u64) -> Result<Self> {
        if sender_id > MAX_SENDER_ID {
            return Err(MediaError::SenderIdOutOfRange(sender_id));
        }
        let key_id = MlsKeyId::try_new(0u64, epoch, sender_id, bit_range())?;
        let key = EncryptionKey::derive_from_mls(CIPHER_SUITE, media_key, key_id)?;
        Ok(Self {
            key,
            counter: MonotonicCounter::default(),
        })
    }

    /// Encrypts one plaintext frame. The output is the audio-level byte, then the SFrame header,
    /// then the ciphertext. The level byte is authenticated but not encrypted.
    pub(crate) fn encrypt(&mut self, plaintext: &[u8], level: u8) -> Result<Vec<u8>> {
        let frame = MediaFrame::try_with_meta_data(&mut self.counter, plaintext, [level])?;
        let encrypted = frame.encrypt(&self.key)?;
        Ok(encrypted.as_ref().to_vec())
    }
}

/// A media key together with the epoch bits it belongs to.
struct EpochKey {
    lsb: u64,
    key: MediaKey,
}

/// A frame that decrypted. Its sender, epoch and counter come from the authenticated header.
pub(crate) struct Opened {
    pub(crate) sender: u64,
    pub(crate) lsb: u64,
    pub(crate) counter: u64,
    pub(crate) plaintext: Vec<u8>,
}

/// Decrypts incoming frames. It holds the current epoch's key, and the previous epoch's key for
/// a grace period after a rekey.
pub(crate) struct SframeReceiver {
    current: EpochKey,
    previous: Option<(EpochKey, Instant)>,
}

impl SframeReceiver {
    pub(crate) fn new(media_key: MediaKey, epoch: u64) -> Self {
        Self {
            current: EpochKey {
                lsb: epoch_lsb(epoch),
                key: media_key,
            },
            previous: None,
        }
    }

    /// Starts accepting `epoch` with `media_key`. The old epoch keeps decrypting until
    /// `now + grace`, and no older epoch decrypts after that.
    pub(crate) fn rotate(
        &mut self,
        media_key: MediaKey,
        epoch: u64,
        now: Instant,
        grace: Duration,
    ) -> Result<()> {
        let lsb = epoch_lsb(epoch);
        if lsb == self.current.lsb {
            return Err(MediaError::BadState(
                "the new epoch has the same key id bits as the current one",
            ));
        }
        let old = std::mem::replace(
            &mut self.current,
            EpochKey {
                lsb,
                key: media_key,
            },
        );
        self.previous = Some((old, now + grace));
        Ok(())
    }

    /// Ends the previous epoch once its grace period is over. Returns its epoch bits, so the
    /// caller can drop that epoch's state too.
    pub(crate) fn expire_previous(&mut self, now: Instant) -> Option<u64> {
        let expired = matches!(&self.previous, Some((_, until)) if now >= *until);
        if expired {
            return self.previous.take().map(|(old, _)| old.lsb);
        }
        None
    }

    /// Decrypts a wire frame. Replay checks are the caller's job.
    pub(crate) fn open(&self, wire: &[u8], now: Instant) -> Result<Opened> {
        let rest = wire.get(1..).ok_or(MediaError::MalformedFrame)?;
        let view = EncryptedFrameView::try_with_meta_data(rest, &wire[..1])?;
        let key_id = MlsKeyId::from_key_id(view.header().key_id(), bit_range());
        let counter = view.header().counter();
        let material = self.key_for(key_id.epoch_lsb(), now)?;
        let key = DecryptionKey::derive_from_mls(CIPHER_SUITE, material, key_id)?;
        let frame = view.decrypt(&key)?;
        Ok(Opened {
            sender: key_id.member_index(),
            lsb: key_id.epoch_lsb(),
            counter,
            plaintext: frame.payload().to_vec(),
        })
    }

    /// Returns the sender id and the plaintext of a wire frame, without replay checks.
    #[cfg(test)]
    pub(crate) fn decrypt(&self, wire: &[u8], now: Instant) -> Result<(u64, Vec<u8>)> {
        let opened = self.open(wire, now)?;
        Ok((opened.sender, opened.plaintext))
    }

    fn key_for(&self, lsb: u64, now: Instant) -> Result<&MediaKey> {
        if lsb == self.current.lsb {
            return Ok(&self.current.key);
        }
        match &self.previous {
            Some((old, until)) if lsb == old.lsb && now < *until => Ok(&old.key),
            _ => Err(MediaError::EpochNotAccepted),
        }
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
        let mut sender = SframeSender::new(&key(7), 3, 0).unwrap();
        let encrypted = sender.encrypt(&plaintext, DEFAULT_LEVEL).unwrap();

        let receiver = SframeReceiver::new(key(7), 0);
        let (from, decrypted) = receiver.decrypt(&encrypted, Instant::now()).unwrap();
        assert_eq!(from, 3);
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn audio_level_and_counter_are_visible_without_a_key() {
        let mut sender = SframeSender::new(&key(7), 3, 0).unwrap();
        let first = sender.encrypt(&[1u8; 160], 42).unwrap();
        let second = sender.encrypt(&[1u8; 160], 42).unwrap();
        assert_eq!(audio_level(&first), Some(42));
        assert_eq!(peek_counter(&first), Some(0));
        assert_eq!(peek_counter(&second), Some(1));
    }

    #[test]
    fn altering_the_audio_level_breaks_decryption() {
        let mut sender = SframeSender::new(&key(7), 3, 0).unwrap();
        let mut encrypted = sender.encrypt(&[1u8; 160], 42).unwrap();
        encrypted[0] = 0;
        let receiver = SframeReceiver::new(key(7), 0);
        assert!(receiver.decrypt(&encrypted, Instant::now()).is_err());
    }

    #[test]
    fn ciphertext_differs_from_plaintext() {
        let plaintext = vec![0x41u8; 160];
        let mut sender = SframeSender::new(&key(9), 1, 0).unwrap();
        let encrypted = sender.encrypt(&plaintext, DEFAULT_LEVEL).unwrap();

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
        let mut sender = SframeSender::new(&key(9), 1, 0).unwrap();
        let a = sender.encrypt(&plaintext, DEFAULT_LEVEL).unwrap();
        let b = sender.encrypt(&plaintext, DEFAULT_LEVEL).unwrap();
        assert_ne!(a, b, "the frame counter must make each encryption unique");
    }

    #[test]
    fn wrong_key_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2, 0).unwrap();
        let encrypted = sender.encrypt(&[0u8; 160], DEFAULT_LEVEL).unwrap();
        let receiver = SframeReceiver::new(key(2), 0);
        assert!(receiver.decrypt(&encrypted, Instant::now()).is_err());
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2, 0).unwrap();
        let mut encrypted = sender.encrypt(&[0u8; 160], DEFAULT_LEVEL).unwrap();
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0x01;
        let receiver = SframeReceiver::new(key(1), 0);
        assert!(receiver.decrypt(&encrypted, Instant::now()).is_err());
    }

    #[test]
    fn sender_id_must_fit_the_key_id() {
        assert!(SframeSender::new(&key(1), MAX_SENDER_ID, 0).is_ok());
        assert!(matches!(
            SframeSender::new(&key(1), MAX_SENDER_ID + 1, 0),
            Err(MediaError::SenderIdOutOfRange(_))
        ));
    }

    #[test]
    fn old_epoch_decrypts_only_within_the_grace_window() {
        let mut old_tx = SframeSender::new(&key(1), 3, 0).unwrap();
        let old_frame = old_tx.encrypt(&[7u8; 160], DEFAULT_LEVEL).unwrap();
        let mut new_tx = SframeSender::new(&key(2), 3, 1).unwrap();
        let new_frame = new_tx.encrypt(&[8u8; 160], DEFAULT_LEVEL).unwrap();

        let mut rx = SframeReceiver::new(key(1), 0);
        let now = Instant::now();
        rx.rotate(key(2), 1, now, EPOCH_GRACE).unwrap();

        assert!(
            rx.decrypt(&new_frame, now).is_ok(),
            "the new epoch decrypts"
        );
        assert!(
            rx.decrypt(&old_frame, now).is_ok(),
            "the old epoch decrypts in the grace window"
        );
        let late = now + EPOCH_GRACE + Duration::from_millis(1);
        assert!(matches!(
            rx.decrypt(&old_frame, late),
            Err(MediaError::EpochNotAccepted)
        ));
    }

    #[test]
    fn rotation_rejects_an_epoch_with_the_same_key_id_bits() {
        let mut rx = SframeReceiver::new(key(1), 0);
        let wrap = 1u64 << EPOCH_BITS;
        assert!(rx
            .rotate(key(2), wrap, Instant::now(), EPOCH_GRACE)
            .is_err());
    }

    #[test]
    fn expiry_reports_the_epoch_bits_that_ended() {
        let mut rx = SframeReceiver::new(key(1), 0);
        let now = Instant::now();
        rx.rotate(key(2), 1, now, EPOCH_GRACE).unwrap();
        assert_eq!(rx.expire_previous(now), None);
        assert_eq!(rx.expire_previous(now + EPOCH_GRACE), Some(0));
        assert_eq!(rx.expire_previous(now + EPOCH_GRACE), None);
    }

    #[test]
    fn a_member_can_send_under_another_members_sender_id_at_the_crypto_layer() {
        // The limit of the cipher layer. Every member holds the media key, so any member can
        // build a frame that claims another member's id. Sender authentication in `auth` is
        // what detects this.
        let mut member = SframeSender::new(&key(1), 2, 0).unwrap();
        let wire = member.encrypt(b"I am Bob", DEFAULT_LEVEL).unwrap();

        let receiver = SframeReceiver::new(key(1), 0);
        let (sender, plaintext) = receiver.decrypt(&wire, Instant::now()).unwrap();
        assert_eq!(
            sender, 2,
            "the cipher layer attributes the frame to the claimed id"
        );
        assert_eq!(plaintext, b"I am Bob");
    }

    #[test]
    fn an_outsider_without_the_media_key_cannot_forge_any_sender() {
        let mut outsider = SframeSender::new(&key(99), 2, 0).unwrap();
        let wire = outsider.encrypt(b"I am Bob", DEFAULT_LEVEL).unwrap();
        let receiver = SframeReceiver::new(key(1), 0);
        assert!(receiver.decrypt(&wire, Instant::now()).is_err());
    }

    #[test]
    fn window_accepts_unseen_counters_and_rejects_seen_ones() {
        let mut window = ReplayWindow::default();
        assert!(window.accepts(0));
        window.accept(5);
        window.accept(200);
        assert!(!window.accepts(5), "5 was seen");
        assert!(window.accepts(199), "199 is inside the window and unseen");
        window.accept(199);
        assert!(!window.accepts(199));
        assert!(window.accepts(6));
    }

    #[test]
    fn window_bits_survive_large_jumps() {
        let mut window = ReplayWindow::default();
        window.accept(0);
        window.accept(1023);
        assert!(
            !window.accepts(0),
            "0 is still inside the window and was seen"
        );
        window.accept(1524);
        assert!(!window.accepts(0), "0 is now outside the window");
        assert!(window.accepts(1000), "1000 is inside the window and unseen");
        window.accept(1000);
        assert!(!window.accepts(1000));
    }
}
