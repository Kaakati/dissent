//! Who is speaking in the current call, for the green ring on avatars and tiles. Another person
//! speaks while their audio frames keep arriving, and this device's person while it sends frames:
//! the voice pipeline sends only voiced frames, so silence sends nothing. A person counts as speaking
//! for `SPEAKING_HOLD` after their last frame, so the ring does not flicker between words.
//!
//! The call loop checks every `SPEAKING_EVERY` ticks, about ten times a second, and emits
//! `CallSpeaking` only when the set of speakers changed, so the window redraws at most that often.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::{lock, Room};
use crate::engine::Engine;
use crate::views::EngineEvent;

/// How long a person counts as speaking after their last frame.
pub(crate) const SPEAKING_HOLD: Duration = Duration::from_millis(450);
/// Call-loop ticks (20 ms each) between two checks of who speaks: five ticks, ten checks a second.
pub(crate) const SPEAKING_EVERY: u32 = 5;

/// This device's own speaking, and the speakers last shown to the window.
#[derive(Default)]
pub(crate) struct Speaking {
    /// When this device last sent an audio frame.
    pub last_sent: Option<Instant>,
    /// The sender ids last reported as speaking, this device's included.
    shown: BTreeSet<u64>,
}

impl Room {
    /// Records that this device just sent an audio frame.
    pub(crate) fn note_sent_audio(&self) {
        lock(&self.speaking).last_sent = Some(Instant::now());
    }

    /// Whether this device's person speaks now: it sent a frame within the hold, and is not muted.
    pub(crate) fn self_speaking(&self, now: Instant) -> bool {
        !self.is_muted() && lock(&self.speaking).last_sent.is_some_and(|t| now.duration_since(t) < SPEAKING_HOLD)
    }

    /// The sender ids speaking now, this device's included.
    pub(crate) fn speaking_sids(&self, now: Instant) -> BTreeSet<u64> {
        let mut out: BTreeSet<u64> = lock(&self.peers)
            .iter()
            .filter(|(_, p)| !p.muted && p.last_frame.is_some_and(|t| now.duration_since(t) < SPEAKING_HOLD))
            .map(|(sid, _)| *sid)
            .collect();
        if self.self_speaking(now) {
            out.insert(self.my_sid);
        }
        out
    }

    /// The person behind each speaking sender id, as root ids.
    pub(crate) fn speaking_roots(&self, now: Instant) -> BTreeSet<String> {
        let sids = self.speaking_sids(now);
        let peers = lock(&self.peers);
        sids.iter().filter_map(|sid| peers.get(sid).map(|p| p.root.to_string())).collect()
    }
}

impl Engine {
    /// Tells the window when the set of speakers changed since the last check.
    pub(crate) fn check_speaking(&self, room: &Room) {
        let now = Instant::now();
        let speaking = room.speaking_sids(now);
        let changed = {
            let mut state = lock(&room.speaking);
            let changed = state.shown != speaking;
            state.shown = speaking;
            changed
        };
        if changed {
            self.emit(EngineEvent::CallSpeaking);
        }
    }
}
