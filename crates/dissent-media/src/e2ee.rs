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

use crate::auth::{frame_hash, HASH_LEN};
use crate::error::{MediaError, Result};
use crate::frame::{stamp, FRAME_SAMPLES, STAMP_LEN};
#[cfg(test)]
use crate::frame::payload_of_stamp;
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

/// The sender a wire frame names in its key id, read without any key. A forwarder routes by it.
pub(crate) fn peek_sender(wire: &[u8]) -> Option<u64> {
    let rest = wire.get(1..)?;
    let view = EncryptedFrameView::try_with_meta_data(rest, &wire[..1]).ok()?;
    Some(MlsKeyId::from_key_id(view.header().key_id(), bit_range()).member_index())
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

/// The first byte of every frame's payload: no redundancy follows.
const PLAIN_FLAG: u8 = 0;
/// The first byte of a redundant payload: the previous frame follows, so a lost one can be
/// recovered from the next.
const RED_FLAG: u8 = 1;

/// Redundancy turns on above this packet loss, and off again only below the lower mark, so it
/// does not flap at the boundary.
const RED_ON_LOSS: f32 = 0.10;
const RED_OFF_LOSS: f32 = 0.05;

/// Whether redundancy should be on for a link that loses `loss` of its packets.
pub(crate) fn redundancy_for(loss: f32, currently_on: bool) -> bool {
    if currently_on {
        loss > RED_OFF_LOSS
    } else {
        loss > RED_ON_LOSS
    }
}

/// The previous frame of a sender, kept so the next one can carry it.
struct Prev {
    counter: u64,
    hash: [u8; HASH_LEN],
    plain: Vec<u8>,
}

/// Encrypts the frames a local sender produces, under one epoch of the media key.
pub(crate) struct SframeSender {
    key: EncryptionKey,
    counter: MonotonicCounter,
    red: bool,
    prev: Option<Prev>,
    /// The next frame's sequence number. It keeps counting across rekeys.
    seq: u32,
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
            red: false,
            prev: None,
            seq: 0,
        })
    }

    /// Derives the sending key for a new epoch, carrying over the sequence counter and the
    /// redundancy setting, so sequence numbers keep counting across a rekey.
    pub(crate) fn rekeyed(&self, media_key: &MediaKey, sender_id: u64, epoch: u64) -> Result<Self> {
        let mut next = Self::new(media_key, sender_id, epoch)?;
        next.seq = self.seq;
        next.red = self.red;
        Ok(next)
    }

    /// Turns redundancy on or off. With it on, each frame carries the one before it.
        pub(crate) fn set_red(&mut self, on: bool) {
        self.red = on;
    }

    /// Whether redundancy is on.
    pub(crate) fn red(&self) -> bool {
        self.red
    }

    /// Encrypts one frame. The output is the audio-level byte, then the SFrame header, then the
    /// ciphertext. The level byte is authenticated but not encrypted.
    ///
    /// The plaintext is prefixed with a frame stamp: the sequence number, the timestamp and the
    /// level. `timestamp` is the sample clock at 48 kHz. `None` derives it from the sequence
    /// number, [`FRAME_SAMPLES`] per frame. Each call uses the next sequence number.
    ///
    /// The encrypted payload starts with a redundancy flag. With redundancy on, the flag is
    /// followed by the previous frame's counter, hash and stamped plaintext.
    pub(crate) fn encrypt(
        &mut self,
        plaintext: &[u8],
        timestamp: Option<u32>,
        level: u8,
    ) -> Result<Vec<u8>> {
        let seq = self.seq;
        let timestamp = timestamp.unwrap_or(seq.wrapping_mul(FRAME_SAMPLES));
        let mut stamped = Vec::with_capacity(STAMP_LEN + plaintext.len());
        stamped.extend_from_slice(&stamp(seq, timestamp, level));
        stamped.extend_from_slice(plaintext);

        let mut payload = Vec::with_capacity(stamped.len() + 32);
        match self.prev.as_ref() {
            Some(prev) if self.red => {
                payload.push(RED_FLAG);
                payload.extend_from_slice(&prev.counter.to_le_bytes());
                payload.extend_from_slice(&prev.hash);
                payload.extend_from_slice(&(prev.plain.len() as u16).to_le_bytes());
                payload.extend_from_slice(&prev.plain);
            }
            _ => payload.push(PLAIN_FLAG),
        }
        payload.extend_from_slice(&stamped);

        let frame = MediaFrame::try_with_meta_data(&mut self.counter, &payload, [level])?;
        let encrypted = frame.encrypt(&self.key)?;
        let wire = encrypted.as_ref().to_vec();
        self.seq = seq.wrapping_add(1);
        if let Some(counter) = peek_counter(&wire) {
            self.prev = Some(Prev {
                counter,
                hash: frame_hash(&wire),
                plain: stamped,
            });
        }
        Ok(wire)
    }
}

/// A media key together with the epoch bits it belongs to.
struct EpochKey {
    lsb: u64,
    key: MediaKey,
}

/// A frame that decrypted. Its sender, epoch and counter come from the authenticated header.
/// `plaintext` is the frame stamp followed by the payload, see [`crate::frame`].
pub(crate) struct Opened {
    pub(crate) sender: u64,
    pub(crate) lsb: u64,
    pub(crate) counter: u64,
    pub(crate) plaintext: Vec<u8>,
    /// The previous frame, carried for redundancy, if the sender included it.
    pub(crate) recovered: Option<Recovered>,
}

/// A frame carried inside the next one, for recovery after loss.
pub(crate) struct Recovered {
    pub(crate) counter: u64,
    pub(crate) hash: [u8; HASH_LEN],
    pub(crate) plaintext: Vec<u8>,
}

/// Splits a decrypted payload into the redundant frame it carries, if any, and the body.
fn unpack(payload: &[u8]) -> Result<(Option<Recovered>, &[u8])> {
    let (flag, rest) = payload.split_first().ok_or(MediaError::MalformedFrame)?;
    match *flag {
        PLAIN_FLAG => Ok((None, rest)),
        RED_FLAG => {
            let fixed = 8 + HASH_LEN + 2;
            if rest.len() < fixed {
                return Err(MediaError::MalformedFrame);
            }
            let counter = u64::from_le_bytes(
                rest[..8]
                    .try_into()
                    .map_err(|_| MediaError::MalformedFrame)?,
            );
            let mut hash = [0u8; HASH_LEN];
            hash.copy_from_slice(&rest[8..8 + HASH_LEN]);
            let len = u16::from_le_bytes(
                rest[8 + HASH_LEN..fixed]
                    .try_into()
                    .map_err(|_| MediaError::MalformedFrame)?,
            ) as usize;
            let plain = rest
                .get(fixed..fixed + len)
                .ok_or(MediaError::MalformedFrame)?;
            let body = &rest[fixed + len..];
            Ok((
                Some(Recovered {
                    counter,
                    hash,
                    plaintext: plain.to_vec(),
                }),
                body,
            ))
        }
        _ => Err(MediaError::MalformedFrame),
    }
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
        let (recovered, body) = unpack(frame.payload())?;
        Ok(Opened {
            sender: key_id.member_index(),
            lsb: key_id.epoch_lsb(),
            counter,
            plaintext: body.to_vec(),
            recovered,
        })
    }

    /// Returns the sender id and the plaintext of a wire frame, without replay checks.
    #[cfg(test)]
    pub(crate) fn decrypt(&self, wire: &[u8], now: Instant) -> Result<(u64, Vec<u8>)> {
        let opened = self.open(wire, now)?;
        Ok((opened.sender, payload_of_stamp(&opened.plaintext)?.to_vec()))
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
        let encrypted = sender.encrypt(&plaintext, None, DEFAULT_LEVEL).unwrap();

        let receiver = SframeReceiver::new(key(7), 0);
        let (from, decrypted) = receiver.decrypt(&encrypted, Instant::now()).unwrap();
        assert_eq!(from, 3);
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn audio_level_and_counter_are_visible_without_a_key() {
        let mut sender = SframeSender::new(&key(7), 3, 0).unwrap();
        let first = sender.encrypt(&[1u8; 160], None, 42).unwrap();
        let second = sender.encrypt(&[1u8; 160], None, 42).unwrap();
        assert_eq!(audio_level(&first), Some(42));
        assert_eq!(peek_counter(&first), Some(0));
        assert_eq!(peek_counter(&second), Some(1));
    }

    #[test]
    fn altering_the_audio_level_breaks_decryption() {
        let mut sender = SframeSender::new(&key(7), 3, 0).unwrap();
        let mut encrypted = sender.encrypt(&[1u8; 160], None, 42).unwrap();
        encrypted[0] = 0;
        let receiver = SframeReceiver::new(key(7), 0);
        assert!(receiver.decrypt(&encrypted, Instant::now()).is_err());
    }

    #[test]
    fn ciphertext_differs_from_plaintext() {
        let plaintext = vec![0x41u8; 160];
        let mut sender = SframeSender::new(&key(9), 1, 0).unwrap();
        let encrypted = sender.encrypt(&plaintext, None, DEFAULT_LEVEL).unwrap();

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
        let a = sender.encrypt(&plaintext, None, DEFAULT_LEVEL).unwrap();
        let b = sender.encrypt(&plaintext, None, DEFAULT_LEVEL).unwrap();
        assert_ne!(a, b, "the frame counter must make each encryption unique");
    }

    #[test]
    fn wrong_key_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2, 0).unwrap();
        let encrypted = sender.encrypt(&[0u8; 160], None, DEFAULT_LEVEL).unwrap();
        let receiver = SframeReceiver::new(key(2), 0);
        assert!(receiver.decrypt(&encrypted, Instant::now()).is_err());
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let mut sender = SframeSender::new(&key(1), 2, 0).unwrap();
        let mut encrypted = sender.encrypt(&[0u8; 160], None, DEFAULT_LEVEL).unwrap();
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
        let old_frame = old_tx.encrypt(&[7u8; 160], None, DEFAULT_LEVEL).unwrap();
        let mut new_tx = SframeSender::new(&key(2), 3, 1).unwrap();
        let new_frame = new_tx.encrypt(&[8u8; 160], None, DEFAULT_LEVEL).unwrap();

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
        let wire = member.encrypt(b"I am Bob", None, DEFAULT_LEVEL).unwrap();

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
        let wire = outsider.encrypt(b"I am Bob", None, DEFAULT_LEVEL).unwrap();
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

#[cfg(test)]
mod red_tests {
    use super::*;
    use crate::auth::frame_hash;

    fn key(byte: u8) -> MediaKey {
        MediaKey::new([byte; 32])
    }

    #[test]
    fn a_redundant_frame_carries_the_one_before_it() {
        let mut tx = SframeSender::new(&key(3), 5, 0).unwrap();
        let first = tx.encrypt(&[1u8; 40], None, DEFAULT_LEVEL).unwrap();
        tx.set_red(true);
        let second = tx.encrypt(&[2u8; 40], None, DEFAULT_LEVEL).unwrap();

        let rx = SframeReceiver::new(key(3), 0);
        let now = Instant::now();
        let opened = rx.open(&second, now).unwrap();
        assert_eq!(
            payload_of_stamp(&opened.plaintext).unwrap(),
            vec![2u8; 40],
            "the body is the current frame"
        );
        let recovered = opened.recovered.expect("the previous frame rides along");
        assert_eq!(recovered.counter, 0);
        assert_eq!(recovered.hash, frame_hash(&first));
        assert_eq!(payload_of_stamp(&recovered.plaintext).unwrap(), vec![1u8; 40]);
    }

    #[test]
    fn a_recovered_frame_keeps_its_sequence_number_timestamp_and_level() {
        let mut tx = SframeSender::new(&key(3), 5, 0).unwrap();
        let _first = tx.encrypt(&[1u8; 40], Some(1234), 33).unwrap();
        tx.set_red(true);
        let second = tx.encrypt(&[2u8; 40], None, 44).unwrap();

        let opened = SframeReceiver::new(key(3), 0)
            .open(&second, Instant::now())
            .unwrap();
        let recovered = opened.recovered.expect("the previous frame rides along");
        let lost = crate::frame::received(5, &recovered.plaintext).unwrap();
        assert_eq!((lost.seq, lost.timestamp, lost.audio_level), (0, 1234, 33));
        let current = crate::frame::received(5, &opened.plaintext).unwrap();
        assert_eq!((current.seq, current.timestamp, current.audio_level), (1, 960, 44));
    }

    #[test]
    fn a_plain_frame_recovers_nothing() {
        let mut tx = SframeSender::new(&key(3), 5, 0).unwrap();
        let wire = tx.encrypt(&[9u8; 40], None, DEFAULT_LEVEL).unwrap();
        let opened = SframeReceiver::new(key(3), 0)
            .open(&wire, Instant::now())
            .unwrap();
        assert!(opened.recovered.is_none());
        assert_eq!(payload_of_stamp(&opened.plaintext).unwrap(), vec![9u8; 40]);
    }

    #[test]
    fn redundancy_turns_on_above_ten_percent_and_off_below_five() {
        assert!(!redundancy_for(0.09, false), "9% does not turn it on");
        assert!(redundancy_for(0.11, false), "11% turns it on");
        assert!(redundancy_for(0.07, true), "7% keeps it on");
        assert!(!redundancy_for(0.04, true), "4% turns it off");
    }
}
