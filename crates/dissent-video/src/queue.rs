//! Send-side priority for audio and video packets sharing one link.
//!
//! Audio always leaves first. Video waits behind it, and a video frame older than
//! `MAX_VIDEO_AGE_MS` is dropped rather than sent late. Dropping a frame also drops the
//! delta frames that depend on it, so the queue waits for the next keyframe before it sends
//! video again.
//!
//! The queue takes the current time as a parameter, so tests control the clock and
//! the caller can drive it from any timer.

use std::collections::VecDeque;

/// A video frame queued for longer than this is stale. Showing it now would lag the call.
pub const MAX_VIDEO_AGE_MS: u64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Audio,
    Video,
}

/// A packet that the link should send now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub lane: Lane,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
struct Queued {
    payload: Vec<u8>,
    keyframe: bool,
    queued_ms: u64,
}

#[derive(Debug, Default)]
pub struct SendQueue {
    audio: VecDeque<Vec<u8>>,
    video: VecDeque<Queued>,
    /// Set after video is dropped. Delta frames are refused until a keyframe arrives.
    waiting_keyframe: bool,
    dropped_video: u64,
}

impl SendQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_audio(&mut self, payload: Vec<u8>) {
        self.audio.push_back(payload);
    }

    /// Queues one video packet. Returns `false` when the packet was refused because it
    /// depends on a frame that was already dropped.
    pub fn push_video(&mut self, payload: Vec<u8>, keyframe: bool, now_ms: u64) -> bool {
        if self.waiting_keyframe && !keyframe {
            self.dropped_video += 1;
            return false;
        }
        if keyframe {
            self.waiting_keyframe = false;
        }
        self.video.push_back(Queued {
            payload,
            keyframe,
            queued_ms: now_ms,
        });
        true
    }

    /// Takes the next packet to send, or `None` when nothing is ready.
    pub fn pop(&mut self, now_ms: u64) -> Option<Outgoing> {
        if let Some(payload) = self.audio.pop_front() {
            return Some(Outgoing {
                lane: Lane::Audio,
                payload,
            });
        }
        self.drop_stale(now_ms);
        self.video.pop_front().map(|frame| Outgoing {
            lane: Lane::Video,
            payload: frame.payload,
        })
    }

    /// Video packets dropped so far, for stats.
    pub fn dropped_video(&self) -> u64 {
        self.dropped_video
    }

    pub fn pending_video(&self) -> usize {
        self.video.len()
    }

    pub fn pending_audio(&self) -> usize {
        self.audio.len()
    }

    /// Drops stale video up to a fresh keyframe. If none is fresh, drops all video and waits.
    fn drop_stale(&mut self, now_ms: u64) {
        let stale = |frame: &Queued| now_ms.saturating_sub(frame.queued_ms) > MAX_VIDEO_AGE_MS;
        let Some(head) = self.video.front() else {
            return;
        };
        if !stale(head) {
            return;
        }
        let keep_from = self
            .video
            .iter()
            .position(|frame| frame.keyframe && !stale(frame));
        let drop_count = match keep_from {
            Some(index) => index,
            None => {
                self.waiting_keyframe = true;
                self.video.len()
            }
        };
        self.video.drain(..drop_count);
        self.dropped_video += drop_count as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(tag: u8) -> Vec<u8> {
        vec![tag]
    }

    #[test]
    fn audio_leaves_before_video_even_when_queued_later() {
        let mut q = SendQueue::new();
        assert!(q.push_video(payload(1), true, 0));
        q.push_audio(payload(2));
        assert_eq!(q.pop(0).unwrap().lane, Lane::Audio);
        assert_eq!(q.pop(0).unwrap().lane, Lane::Video);
        assert_eq!(q.pop(0), None);
    }

    #[test]
    fn audio_order_is_kept() {
        let mut q = SendQueue::new();
        for tag in 0..5 {
            q.push_audio(payload(tag));
        }
        let order: Vec<u8> = (0..5).map(|_| q.pop(0).unwrap().payload[0]).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn fresh_video_is_sent_in_order() {
        let mut q = SendQueue::new();
        q.push_video(payload(1), true, 0);
        q.push_video(payload(2), false, 10);
        assert_eq!(q.pop(20).unwrap().payload, payload(1));
        assert_eq!(q.pop(20).unwrap().payload, payload(2));
        assert_eq!(q.dropped_video(), 0);
    }

    #[test]
    fn stale_video_is_dropped_and_audio_still_flows() {
        let mut q = SendQueue::new();
        q.push_video(payload(1), true, 0);
        q.push_video(payload(2), false, 0);
        q.push_audio(payload(9));
        assert_eq!(q.pop(1_000).unwrap().lane, Lane::Audio);
        assert_eq!(q.pop(1_000), None, "stale video must not be sent");
        assert_eq!(q.dropped_video(), 2);
    }

    #[test]
    fn stale_frames_are_dropped_up_to_a_fresh_keyframe() {
        let mut q = SendQueue::new();
        q.push_video(payload(1), true, 0);
        q.push_video(payload(2), false, 0);
        q.push_video(payload(3), true, 300);
        q.push_video(payload(4), false, 300);
        let sent = q.pop(350).unwrap();
        assert_eq!(
            sent.payload,
            payload(3),
            "the fresh keyframe is the next video sent"
        );
        assert_eq!(q.pop(350).unwrap().payload, payload(4));
        assert_eq!(q.dropped_video(), 2);
    }

    #[test]
    fn deltas_are_refused_until_the_next_keyframe() {
        let mut q = SendQueue::new();
        q.push_video(payload(1), true, 0);
        assert_eq!(q.pop(1_000), None);
        assert!(
            !q.push_video(payload(2), false, 1_000),
            "a delta after a drop depends on lost data"
        );
        assert!(
            q.push_video(payload(3), true, 1_000),
            "a keyframe resumes the stream"
        );
        assert!(q.push_video(payload(4), false, 1_000));
        assert_eq!(q.pop(1_000).unwrap().payload, payload(3));
    }

    #[test]
    fn video_is_not_dropped_at_the_age_limit() {
        let mut q = SendQueue::new();
        q.push_video(payload(1), true, 0);
        assert_eq!(q.pop(MAX_VIDEO_AGE_MS).unwrap().payload, payload(1));
    }

    #[test]
    fn audio_is_never_dropped_by_age() {
        let mut q = SendQueue::new();
        q.push_audio(payload(1));
        assert_eq!(q.pop(10 * MAX_VIDEO_AGE_MS).unwrap().payload, payload(1));
    }
}
