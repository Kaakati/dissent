//! The sound of a shared screen. A sharer can add the computer's sound, off unless they turn it on, and each
//! viewer has a volume for it.
//!
//! It travels on the video carrier as frames whose codec is Opus, under the same SFrame key as the
//! video. Every frame stands alone, so it needs no gate and no layers. A released device reads the codec tag,
//! does not know it, and drops the frame as malformed before anything else sees it, so a released viewer gets
//! the picture without sound, and a released forwarder does not pass the sound on. See "Screen share" in
//! `docs/design/04-calls.md`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dissent_audio::{Decoder, Encoder, OpusDecoder, OpusEncoder, FRAME_SAMPLES};
use dissent_media::VideoCodec;

use crate::call_video::{Encoded, TestCapture};
use crate::error::{CoreError, Result};

/// What the sound takes from the link, in kbps: its bitrate plus packet headers.
pub(crate) const SCREEN_AUDIO_KBPS: u32 = 110;
const BITRATE_BPS: u32 = 96_000;
/// Frames the thread may queue for the call loop, which runs every 20 ms.
const QUEUE: usize = 16;
/// The sound queued before a stream starts to play, and the most kept, in samples at 48 kHz. Video frames
/// share the carrier, so packets can come in bursts.
const START_SAMPLES: usize = FRAME_SAMPLES * 3;
const MOST_SAMPLES: usize = FRAME_SAMPLES * 25;
/// The sound of a sender whose frames stopped this long ago counts as over.
const GONE: Duration = Duration::from_secs(3);

/// Whether this device can share its sound.
pub fn screen_audio_available() -> bool {
    dissent_audio::loopback::available()
}

/// A thread that captures and encodes the sound of a share. Dropping it stops the thread.
pub(crate) struct ScreenAudio {
    stop: Arc<AtomicBool>,
    rx: Receiver<Encoded>,
}

impl ScreenAudio {
    /// Starts on the computer's sound, or on a test tone when `source` is not a device, so no test
    /// captures the machine's sound. A device that refuses fails here.
    pub(crate) fn start(source: TestCapture) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel::<Encoded>(QUEUE);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("dissent-screen-audio".into())
            .spawn(move || {
                let mut read = match Source::open(source) {
                    Ok(read) => read,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut encoder = match OpusEncoder::for_music(BITRATE_BPS) {
                    Ok(encoder) => encoder,
                    Err(e) => {
                        let _ = ready_tx.send(Err(CoreError::Invalid(format!("screen audio: {e}"))));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let mut pending = Vec::new();
                let mut frame_id = 0u32;
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    if read.read(&mut pending).is_err() {
                        break;
                    }
                    let mut used = 0;
                    while pending.len() - used >= FRAME_SAMPLES {
                        let Ok(data) = encoder.encode(&pending[used..used + FRAME_SAMPLES]) else {
                            return;
                        };
                        used += FRAME_SAMPLES;
                        let frame = Encoded { frame_id, keyframe: true, codec: VideoCodec::Opus, layer: 0, data };
                        frame_id = frame_id.wrapping_add(1);
                        // A call loop that is behind loses sound rather than delaying it.
                        if let Err(mpsc::TrySendError::Disconnected(_)) = tx.try_send(frame) {
                            return;
                        }
                    }
                    pending.drain(..used);
                }
            })
            .map_err(|e| CoreError::Invalid(format!("screen audio thread: {e}")))?;
        ready_rx
            .recv()
            .map_err(|_| CoreError::Invalid("the screen audio thread ended before it started".into()))??;
        Ok(Self { stop, rx })
    }

    pub(crate) fn try_next(&self) -> Option<Encoded> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ScreenAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

enum Source {
    System(dissent_audio::loopback::SystemAudio),
    /// A 440 Hz tone, on the clock.
    Tone { started: Instant, written: u64 },
}

impl Source {
    fn open(source: TestCapture) -> Result<Self> {
        match source {
            TestCapture::Device => dissent_audio::loopback::SystemAudio::open()
                .map(Self::System)
                .map_err(|e| CoreError::Invalid(format!("screen audio: {e}"))),
            _ => Ok(Self::Tone { started: Instant::now(), written: 0 }),
        }
    }

    fn read(&mut self, out: &mut Vec<f32>) -> std::result::Result<(), ()> {
        match self {
            Self::System(capture) => capture.read_48k(out).map_err(|_| ()),
            Self::Tone { started, written } => {
                let due = started.elapsed().as_micros() as u64 * 48 / 1_000;
                while *written < due {
                    let t = *written as f32 / 48_000.0;
                    out.push((t * 440.0 * std::f32::consts::TAU).sin() * 0.3);
                    *written += 1;
                }
                Ok(())
            }
        }
    }
}

/// One sharer's sound as it arrives here.
struct Incoming {
    root: String,
    decoder: OpusDecoder,
    queue: VecDeque<f32>,
    playing: bool,
    last: Instant,
}

/// The sound of every sharer in this call, mixed into the call's playout. The mix goes where the voices go, so it
/// follows the chosen speaker and deafen, and the echo canceller knows it.
#[derive(Default)]
pub(crate) struct ScreenAudioMix {
    streams: BTreeMap<u64, Incoming>,
}

impl ScreenAudioMix {
    /// Decodes one received frame.
    pub(crate) fn receive(&mut self, sender: u64, root: &str, payload: &[u8]) {
        let incoming = match self.streams.entry(sender) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let Ok(decoder) = OpusDecoder::new() else {
                    return;
                };
                entry.insert(Incoming { root: root.to_string(), decoder, queue: VecDeque::new(), playing: false, last: Instant::now() })
            }
        };
        incoming.last = Instant::now();
        let mut pcm = [0f32; FRAME_SAMPLES];
        if incoming.decoder.decode(Some(payload), &mut pcm).is_ok() {
            incoming.queue.extend(pcm);
        }
        // Too far behind: the oldest sound goes, so the share stays near the picture.
        let over = incoming.queue.len().saturating_sub(MOST_SAMPLES);
        incoming.queue.drain(..over);
    }

    /// Adds each sharer's sound to `out`, at the volume this viewer chose, in percent of its level. 100 by default.
    pub(crate) fn mix(&mut self, out: &mut [f32], volumes: &BTreeMap<String, u32>) {
        self.streams.retain(|_, incoming| incoming.last.elapsed() < GONE);
        for incoming in self.streams.values_mut() {
            if !incoming.playing {
                incoming.playing = incoming.queue.len() >= START_SAMPLES;
                if !incoming.playing {
                    continue;
                }
            }
            if incoming.queue.is_empty() {
                incoming.playing = false;
                continue;
            }
            let gain = volumes.get(&incoming.root).copied().unwrap_or(100) as f32 / 100.0;
            let take = out.len().min(incoming.queue.len());
            for (slot, sample) in out.iter_mut().zip(incoming.queue.drain(..take)) {
                *slot += sample * gain;
            }
        }
    }

    /// The root ids whose sound is arriving.
    pub(crate) fn sounding(&self) -> Vec<String> {
        self.streams.values().filter(|i| i.last.elapsed() < GONE).map(|i| i.root.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32
    }

    /// Runs a test tone through the sender's thread and the receiver's mix.
    fn tone_through_the_mix(volume: Option<u32>) -> f32 {
        let send = ScreenAudio::start(TestCapture::Pattern).expect("a tone needs no device");
        let mut mix = ScreenAudioMix::default();
        let volumes: BTreeMap<String, u32> = volume.map(|v| ("root".to_string(), v)).into_iter().collect();
        let (started, mut last) = (Instant::now(), 0.0);
        while started.elapsed() < Duration::from_millis(900) {
            while let Some(frame) = send.try_next() {
                assert_eq!((frame.codec, frame.keyframe), (VideoCodec::Opus, true));
                mix.receive(7, "root", &frame.data);
            }
            let mut out = vec![0f32; FRAME_SAMPLES];
            mix.mix(&mut out, &volumes);
            last = energy(&out).max(last * 0.9);
            std::thread::sleep(Duration::from_millis(20));
        }
        last
    }

    #[test]
    fn a_tone_reaches_the_mix_and_the_volume_scales_it() {
        let full = tone_through_the_mix(None);
        assert!(full > 0.01, "the tone is heard: {full}");
        assert_eq!(tone_through_the_mix(Some(0)), 0.0, "volume 0 is silent");
        let half = tone_through_the_mix(Some(50));
        assert!(half < full * 0.5, "half volume is quieter: {half} and {full}");
    }

    #[test]
    fn a_mix_with_nothing_arriving_leaves_the_voices_alone() {
        let mut out = vec![0.25f32; FRAME_SAMPLES];
        ScreenAudioMix::default().mix(&mut out, &BTreeMap::new());
        assert!(out.iter().all(|s| *s == 0.25));
    }

    #[test]
    fn a_stream_far_behind_keeps_only_its_newest_sound() {
        let mut encoder = OpusEncoder::for_music(BITRATE_BPS).unwrap();
        let mut mix = ScreenAudioMix::default();
        for _ in 0..100 {
            let packet = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap();
            mix.receive(1, "root", &packet);
        }
        assert_eq!(mix.streams[&1].queue.len(), MOST_SAMPLES);
        assert_eq!(mix.sounding(), ["root"]);
    }
}
