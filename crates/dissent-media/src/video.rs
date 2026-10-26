//! Video frames on the H.264 carrier.
//!
//! A video frame is sent as one NAL unit on the video media line. str0m fragments that NAL into
//! FU-A packets and reassembles it on the other side. The NAL body is:
//!
//! - `frame_id` (u32, big endian): the sender's video frame counter.
//! - `flags` (u8): bit 0 is the keyframe flag.
//! - `codec` (u8): the real codec, AV1 or H.264. The SDP label is only the carrier.
//! - the SFrame ciphertext of the codec frame.
//!
//! The first three fields stay in the clear, so a forwarder can route and keyframe-gate without
//! decrypting. The body is escaped the way RBSP is, so no start code can appear inside it. str0m
//! never parses the bytes, which is why a str0m upgrade that starts parsing NAL contents breaks
//! this carrier. The regression tests in `tests/video_carrier.rs` guard against that.

use crate::error::{MediaError, Result};

/// Clear header bytes in front of the ciphertext: frame id, flags, codec.
pub(crate) const HEADER_LEN: usize = 6;

/// The NAL header byte: a non-IDR slice, so str0m never treats a carrier as a real IDR.
const NAL_HEADER: u8 = 0x01;

/// The keyframe bit of `flags`.
const FLAG_KEYFRAME: u8 = 0x01;
/// The screen bit of `flags`: set for a shared screen, clear for a camera.
const FLAG_SCREEN: u8 = 0x02;

/// How many recent counters per sender the replay check remembers.
const REPLAY_WINDOW: usize = 128;

/// The codec a video frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// AV1, decoded with rav1d.
    Av1,
    /// H.264, decoded with OpenH264.
    H264,
}

impl VideoCodec {
    fn tag(self) -> u8 {
        match self {
            Self::Av1 => 1,
            Self::H264 => 2,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Av1),
            2 => Some(Self::H264),
            _ => None,
        }
    }
}

/// What a video frame is: its counter, whether it is a keyframe, its codec, and whether it is a
/// shared screen rather than a camera. A sender numbers each of its streams separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMeta {
    /// The sender's frame counter for this stream, from 0.
    pub frame_id: u32,
    /// Whether the frame is a keyframe.
    pub keyframe: bool,
    /// The codec the frame is encoded with.
    pub codec: VideoCodec,
    /// Whether the frame is part of a shared screen rather than a camera.
    pub screen: bool,
}

/// A video frame as the receiver delivers it, after decryption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// The sender's id.
    pub sender: u64,
    /// The sender's video frame counter for this stream.
    pub frame_id: u32,
    /// Whether the frame is a keyframe, which a decoder can start from.
    pub keyframe: bool,
    /// The codec the frame is encoded with.
    pub codec: VideoCodec,
    /// Whether the frame is a shared screen rather than a camera.
    pub screen: bool,
    /// The decrypted codec frame.
    pub payload: Vec<u8>,
}

/// The key the gate and the forwarder keep a stream under: each sender has one camera stream and
/// one screen stream.
pub(crate) fn stream_key(sender: u64, screen: bool) -> u64 {
    (sender << 1) | u64::from(screen)
}

/// The clear header of a carried frame, read without any key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub frame_id: u32,
    pub keyframe: bool,
    pub codec: VideoCodec,
    pub screen: bool,
}

/// Escapes a body so `00 00 0x` (x <= 3) never appears: an `03` is inserted after two zeros.
pub(crate) fn escape(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + body.len() / 64 + 1);
    let mut zeros = 0;
    for &b in body {
        if zeros >= 2 && b <= 3 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Undoes [`escape`].
pub(crate) fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 0x03 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Builds the carrier NAL for one encrypted frame. `ciphertext` is the SFrame output.
pub(crate) fn carry(meta: VideoMeta, ciphertext: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    body.extend_from_slice(&meta.frame_id.to_be_bytes());
    let mut flags = 0;
    if meta.keyframe {
        flags |= FLAG_KEYFRAME;
    }
    if meta.screen {
        flags |= FLAG_SCREEN;
    }
    body.push(flags);
    body.push(meta.codec.tag());
    body.extend_from_slice(ciphertext);
    let mut nal = Vec::with_capacity(body.len() + body.len() / 64 + 1);
    nal.push(NAL_HEADER);
    nal.extend(escape(&body));
    nal
}

/// The carrier NAL without the Annex B start code that str0m's depacketizer adds. Forwarders
/// keep and resend this form, since the writer must not see a start code.
pub(crate) fn nal_of(delivered: &[u8]) -> Result<&[u8]> {
    if delivered.starts_with(&[0, 0, 0, 1]) {
        Ok(&delivered[4..])
    } else if delivered.starts_with(&[0, 0, 1]) {
        Ok(&delivered[3..])
    } else {
        Err(MediaError::BadState("video frame has no start code"))
    }
}

/// Reads the clear header and the ciphertext from a carrier NAL, see [`nal_of`].
pub(crate) fn uncarry(nal: &[u8]) -> Result<(Header, Vec<u8>)> {
    if nal.first() != Some(&NAL_HEADER) {
        return Err(MediaError::BadState("video frame has no carrier header"));
    }
    let body = unescape(&nal[1..]);
    if body.len() < HEADER_LEN {
        return Err(MediaError::BadState("video frame header is short"));
    }
    let header = Header {
        frame_id: u32::from_be_bytes(body[0..4].try_into().expect("four bytes")),
        keyframe: body[4] & FLAG_KEYFRAME != 0,
        screen: body[4] & FLAG_SCREEN != 0,
        codec: VideoCodec::from_tag(body[5])
            .ok_or(MediaError::BadState("video frame names an unknown codec"))?,
    };
    Ok((header, body[HEADER_LEN..].to_vec()))
}

/// Remembers the recent counters of each sender, so a replayed ciphertext is dropped.
#[derive(Debug, Default)]
pub(crate) struct VideoReplay {
    seen: std::collections::HashMap<u64, std::collections::VecDeque<u64>>,
}

impl VideoReplay {
    /// Records `counter` for `sender`. Returns false when it was already seen or is older than
    /// the window.
    pub(crate) fn accept(&mut self, sender: u64, counter: u64) -> bool {
        let recent = self.seen.entry(sender).or_default();
        if recent.contains(&counter) {
            return false;
        }
        if let Some(&oldest) = recent.front() {
            if recent.len() >= REPLAY_WINDOW && counter < oldest {
                return false;
            }
        }
        recent.push_back(counter);
        if recent.len() > REPLAY_WINDOW {
            recent.pop_front();
        }
        true
    }
}

/// A frame that a keyless call received, audio or video, before it is forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawFrame {
    Audio(Vec<u8>),
    Video(RawVideo),
}

/// A carried frame that a keyless call hands on, unchanged, with what a forwarder needs to route
/// it: the sender and whether it is a keyframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawVideo {
    pub nal: Vec<u8>,
    pub sender: u64,
    pub screen: bool,
    pub keyframe: bool,
}

/// What the gate decides for one arriving frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Admit {
    /// Hand the frame on.
    pub deliver: bool,
    /// The sender's stream is broken, so a keyframe is needed. The caller asks for one, with a
    /// throttle.
    pub need_keyframe: bool,
}

#[derive(Debug, Clone, Copy)]
struct SenderGate {
    /// The next frame id expected in sequence, once synchronised.
    expected: u32,
    /// True until a keyframe resynchronises the stream. A new sender starts here.
    waiting: bool,
}

/// Per-sender frame sequencing. A frame that does not follow the last one delivered is a gap:
/// the deltas after it cannot be decoded, so they are dropped until the next keyframe. The
/// decoder starts on a keyframe, so a new sender starts in the waiting state.
#[derive(Debug, Default)]
pub(crate) struct VideoGate {
    senders: std::collections::HashMap<u64, SenderGate>,
}

impl VideoGate {
    /// Whether any stream is waiting for a keyframe. A waiting stream asks again on a timer,
    /// because a lost keyframe leaves no later frame to trigger the next request.
    pub(crate) fn waiting(&self) -> bool {
        self.senders.values().any(|gate| gate.waiting)
    }

    pub(crate) fn admit(&mut self, stream: u64, frame_id: u32, keyframe: bool) -> Admit {
        let gate = self.senders.entry(stream).or_insert(SenderGate {
            expected: frame_id,
            waiting: true,
        });
        if keyframe {
            // A keyframe always resynchronises, even when it is not the next one.
            gate.waiting = false;
            gate.expected = frame_id.wrapping_add(1);
            return Admit {
                deliver: true,
                need_keyframe: false,
            };
        }
        if gate.waiting {
            return Admit {
                deliver: false,
                need_keyframe: true,
            };
        }
        if frame_id == gate.expected {
            gate.expected = frame_id.wrapping_add(1);
            return Admit {
                deliver: true,
                need_keyframe: false,
            };
        }
        if frame_id.wrapping_sub(gate.expected) > u32::MAX / 2 {
            // Older than the expected frame: a duplicate or a late copy.
            return Admit {
                deliver: false,
                need_keyframe: false,
            };
        }
        // A gap: frames were lost ahead of this one.
        gate.waiting = true;
        Admit {
            deliver: false,
            need_keyframe: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7) % 251) as u8).collect()
    }

    #[test]
    fn escape_round_trips_zero_runs() {
        let raw = vec![0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0, 0];
        let escaped = escape(&raw);
        // A start code needs 00 00 01; after escaping, 00 00 is never followed by 00, 01 or 02.
        assert!(!escaped
            .windows(3)
            .any(|w| w[0] == 0 && w[1] == 0 && w[2] < 3));
        assert_eq!(unescape(&escaped), raw);
    }

    #[test]
    fn carry_and_uncarry_keep_the_header_and_the_ciphertext() {
        let ciphertext = body(50_000);
        let meta = VideoMeta {
            frame_id: 9,
            keyframe: true,
            codec: VideoCodec::Av1,
            screen: true,
        };
        let nal = carry(meta, &ciphertext);
        let mut delivered = vec![0, 0, 0, 1];
        delivered.extend_from_slice(&nal);
        let (header, got) = uncarry(nal_of(&delivered).unwrap()).unwrap();
        assert_eq!(
            header,
            Header {
                frame_id: 9,
                keyframe: true,
                codec: VideoCodec::Av1,
                screen: true,
            }
        );
        assert_eq!(got, ciphertext);
    }

    #[test]
    fn a_delta_frame_has_no_keyframe_flag() {
        let meta = VideoMeta {
            frame_id: 4,
            keyframe: false,
            codec: VideoCodec::H264,
            screen: false,
        };
        let nal = carry(meta, &[1, 2, 3]);
        let mut delivered = vec![0, 0, 1];
        delivered.extend_from_slice(&nal);
        let (header, _) = uncarry(nal_of(&delivered).unwrap()).unwrap();
        assert!(!header.keyframe);
        assert_eq!(header.codec, VideoCodec::H264);
    }

    #[test]
    fn uncarry_rejects_bytes_without_a_start_code() {
        assert!(nal_of(&[1, 2, 3, 4, 5, 6, 7]).is_err());
    }

    #[test]
    fn uncarry_rejects_a_nal_with_the_wrong_header() {
        assert!(uncarry(&[0x65, 0, 0, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn replay_window_drops_repeats_and_old_counters() {
        let mut replay = VideoReplay::default();
        assert!(replay.accept(1, 10));
        assert!(!replay.accept(1, 10), "a repeat is a replay");
        assert!(replay.accept(2, 10), "counters are per sender");
        for c in 11..11 + REPLAY_WINDOW as u64 {
            assert!(replay.accept(1, c));
        }
        assert!(
            !replay.accept(1, 10),
            "a counter older than the window is refused"
        );
    }
}
