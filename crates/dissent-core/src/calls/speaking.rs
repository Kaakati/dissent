//! Who is speaking in the current call, for the green ring on avatars and tiles. Another person
//! speaks while their audio frames keep arriving, and this device's person while it sends frames:
//! the voice pipeline sends only voiced frames, so silence sends nothing. A person counts as speaking
//! for `SPEAKING_HOLD` after their last frame, so the ring does not flicker between words.
//!
//! The call loop checks on every tick, right after it has taken in and sent that tick's frames, and
//! emits `CallSpeaking` only when the set of speakers changed. Both ends of speaking are measured in
//! time, not ticks: a frame starts it at once, and `SPEAKING_HOLD` after the last frame ends it. On a
//! loaded machine whose ticks run late, a check still follows each frame, so a short burst is not
//! missed, as it was when the check ran every fifth tick before the sends.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::{lock, Room};
use crate::engine::Engine;
use crate::views::EngineEvent;

/// How long a person counts as speaking after their last frame.
pub(crate) const SPEAKING_HOLD: Duration = Duration::from_millis(450);

/// This device's own speaking, and the speakers last shown to the window.
#[derive(Default)]
pub(crate) struct Speaking {
    /// When this device last sent an audio frame.
    pub last_sent: Option<Instant>,
    /// The sender ids last reported as speaking, this device's included.
    shown: BTreeSet<u64>,
    /// Checks run, for a test that timed out.
    pub checks: u64,
    /// Checks that found the speakers changed and announced it.
    pub emits: u64,
}

impl Speaking {
    /// How many speakers the window was last told of.
    pub(crate) fn shown_len(&self) -> usize {
        self.shown.len()
    }
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
            state.checks += 1;
            state.emits += u64::from(changed);
            changed
        };
        if changed {
            self.emit(EngineEvent::CallSpeaking);
        }
    }
}
