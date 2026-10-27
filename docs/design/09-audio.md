# Design 09: voice pipeline

Status: `crates/dissent-audio` implements the codec, jitter buffer, processing, pipeline and device
layer, with tests. Nothing in the media crate or the desktop app uses it yet. The integration
steps are at the end.

## What the pipeline does

Mic audio becomes 20 ms Opus packets on the send side. Each packet carries one audio-level byte in
front. On the receive side, each sender gets its own jitter buffer and decoder, and up to a fixed
number of senders are mixed into one playout stream. The crate never touches the network or a
UI framework. The app calls it on a 20 ms clock.

Every frame is 960 samples, mono, 48 kHz. Devices run at 48 kHz when they support it. Otherwise
the crate resamples linearly on the pipeline thread.

## Latency budget

Target: under 150 ms mouth to ear. The numbers below are estimates from the design, not
measurements, except where noted.

| Stage | Estimate | Notes |
|---|---|---|
| Capture device buffer | 10 to 20 ms | Depends on the OS audio stack |
| Frame fill | 0 to 20 ms, 10 ms average | Waiting for a full 20 ms frame |
| RNNoise | about 10 ms | One 10 ms block; the exact delay is not measured |
| Opus encoder lookahead | 6.5 ms | Measured: the codec round trip lags by 309 samples |
| Network, one way | path dependent | Not in this crate |
| Jitter buffer target | 20 to 200 ms | Adapts to jitter. About 75 ms in the 0 to 80 ms jitter test |
| Playout device buffer | 10 to 20 ms | Depends on the OS audio stack |

The jitter target is the largest term we control. A stable link with 20 ms of jitter keeps the
target near 20 to 40 ms and the total well under budget. A link with 80 ms of jitter pushes the
target near 75 ms, and the total then depends on the network path.

## Codec choices

- **Opus, VOIP mode, 48 kHz mono, 20 ms frames.** VOIP is the speech mode. It is the right choice
  for voice. It is not a waveform-faithful mode for tonal or music content.
- **Bitrate 16 to 64 kbps, default 32 kbps, VBR.** Packet sizes follow the setting. A 16 kbps
  setting averaged 40 bytes per frame, and 64 kbps averaged 159 bytes per frame.
- **DTX on.** Silence becomes one- or two-byte packets. In the test, 38 of the last 40 frames of
  silence were that small. The rest are the periodic updates libopus sends about every 400 ms.
- **In-band FEC on, expected loss as a setting.** The default expected loss is 5 percent. FEC
  puts a low-bitrate copy of the previous frame into the next packet. In the test, FEC recovered
  a single dropped frame at 12.9 dB, against 0.9 dB for concealment alone, averaged over 10 drops.
- **Deep redundancy (DRED) is not enabled.** Opus 1.5 DRED and its neural concealment need model
  weights that the vendored source does not include. The weights are downloaded at build time
  upstream. Without them, this build has FEC and classic packet loss concealment only.

Waveform SNR is a weak quality measure for Opus in VOIP mode. A 1 kHz sine reached 27 dB at
64 kbps in VOIP mode and 44 dB in the audio and music modes. Speech, not sine waves, is the
target, so VOIP stays.

## Build requirements

- **libopus is built from source by `build.rs` with the `cc` crate.** The source is vendored under
  `crates/dissent-audio/vendor/libopus` (libopus 1.5.2, without the DNN models). The build needs a
  C compiler that `cc` can find: MSVC on Windows, clang or gcc elsewhere. It needs no cmake, nasm
  or autotools.
- **Why not a published `-sys` crate.** `opus-static-sys`, `audiopus_sys`, `opusic-sys` and
  `libopus_sys` all build through cmake. `opus-sys` 0.2 builds through autotools and bundles
  libopus 1.1, which has no DRED.
- **cpal** needs the ALSA development headers on Linux (`libasound2-dev`). It needs nothing extra
  on Windows (WASAPI) or macOS (CoreAudio).
- **Echo cancellation is our own code.** `webrtc-audio-processing` 2.x needs meson, ninja and
  autotools, and bindgen with libclang. Those are not available on this machine, so we did not
  use it.

## Noise suppression, AGC and voice activity

- **RNNoise** (`nnnoiseless`, pure Rust) runs on 480-sample blocks. Two blocks make one frame.
  On coloured background noise, the test measured 48 dB of noise reduction. On white noise it
  measured only 1 to 3 dB, because RNNoise is trained on speech-band background noise.
- **Voice activity** comes from RNNoise's probability, with 100 ms of hangover. The test gave a
  mean probability of 0.71 on speech-like input and 0.00 on silence.
- **Gated frames go out as digital silence.** When a frame is muted, push-to-talk is closed, or
  voice activity says no speech, the sender zeroes the frame. The encoder then emits DTX packets.
  The receiver never sees a gap, so it never runs concealment for a muted sender.
- **Audio level** follows RFC 6464: 0 is full scale and 127 is silence. A 0.3 amplitude sine
  gives 13. It travels as one byte in front of the Opus packet.
- **AGC** targets -20 dBFS with at most +/-12 dB of gain. The level estimate is smoothed over about
  200 ms. On the test input it landed at -21.1 dBFS.

## Echo cancellation

- **Normalised LMS on the full band at 48 kHz.** The filter predicts the echo of the audio we
  played out, and subtracts that prediction from the mic. Default tail is a parameter, and the
  test used 64 ms.
- **Measured.** With a two-tap echo path at 20 and 35 ms, the canceller reached 46 dB of echo
  return loss enhancement after three seconds, using white far-end noise.
- **Limits.**
  - It is linear. Speaker distortion and nonlinear echo pass through.
  - It has no double-talk detector. Near-end speech during far-end speech slows convergence and
    leaves residue.
  - It does not estimate delay. The tail must cover the device round-trip latency plus the
    acoustic path. A tail shorter than that cancels nothing.
  - Convergence on speech as the far-end signal is unmeasured. White noise converges fast, and
    speech converges slower.
  - Cost is two multiply-adds per tap per sample. A 128 ms tail is about 12,000 operations per
    sample in each direction. Release-mode CPU use is not measured.
- **Headphones need none.** The app should skip echo cancellation when the output is headphones.

## Double-talk: what we tried

Double talk means both ends speak at once. The canceller must keep the near-end voice and still remove
the echo. Today the time-domain filter fails this: during double talk the near-end voice comes out
about 5.6 dB quieter than it went in (`echo_path::double_talk_keeps_the_near_end_voice`, ignored). The
measurements below come from `crates/dissent-audio/tests/echo_path.rs`: a room impulse response with a
250 ms tail, speaker buffering of 40 to 120 ms, 200 ppm clock drift, and a speech-like far end.

- **Learned-gain detector (time domain), reverted.** Mic power is compared with the echo-path gain
  learned from mic over reference power while the far end talks alone, and the filter freezes when the
  mic is 1.4 times that. It does not read the filter output, so it avoids the circularity below. It
  failed in three ways:
  - Freezing on the first trigger let the learned gain sink, and the filter stopped for good: far-only
    ERLE fell to about 0 dB.
  - Slow learning during double talk kept far-only ERLE at 36 to 40 dB, but near-end speech stayed
    about 1 dB below its residual. The detector fires for about 70% of the double-talk window and drops
    out between syllables. In those gaps the filter keeps adapting on the near-end voice.
  - A 0.25 s hangover fixed the gaps but froze the filter during far-only false alarms, so far-only
    ERLE fell to 2 to 14 dB. Margins of 1.4, 2.0 and 3.0 all failed.
- **Circular detection.** A detector that reads the filter's echo estimate cannot see near-end speech
  the filter has already learned, because the estimate contains it. NLMS over 4608 taps absorbs part of
  the near-end voice, so the estimate goes up with the mic and the test never fires. Any next detector
  must read the mic and the reference only, or the errors in the filter's own output.
- **Partitioned frequency-domain filter (block path), reverted.** A 26-partition overlap-save filter
  with a 480-sample block and a 256 ms tail. The echo path reproduced exactly with known weights, so the
  failures below are in adaptation, and each one was reproduced in NumPy first.
  - **Normaliser instability.** Dividing each bin's step by that block's own power diverges. The
    constrained gradient spreads energy across bins, so a bin with little reference power takes a huge
    step. Smoothing the power over blocks fixes the blow-up. The power still needs a floor at 10% of the
    mean over bins, or a weak bin's step grows without bound.
  - **Weight constraint.** Without the constraint the taps spread past the block, and the echo wraps.
    The weights have to be transformed back, cut to the first block of taps, and transformed forward
    after each update.
  - **DC offset.** A reference with a DC offset puts most of its power in the lowest bin. That bin's
    step then dominates the update and the filter diverges. The unit tests use zero-mean noise. A real
    reference should be high-passed before it reaches the filter.
  - **Partition count.** Normalising by the total power over all partitions converges far too slowly:
    the echo's first partition learns about 26 times slower than it would alone. Per-partition
    normalisation converges, but it is unstable on coloured input (AR(1) with coefficient 0.98: 19 dB at
    step 0.03, unstable at 0.1). Each partition sees the same error, so the partitions are correlated,
    and the step has to be scaled to the partition count.
  - **Numbers at the best setting** (per-partition normalisation, step 0.1, room harness): far-only
    ERLE 34.4 dB at 0 ppm, 19.2 dB at 200 ppm. Double talk gave an output of -1.4 dBFS against a
    near-end voice at -19.3 dBFS, so the block path adds energy when both talk. CPU was not measured.
- **Zero-lag coherence gate, not working.** The per-bin step was scaled by the coherence of mic and
  reference. The coherence is computed over one block, so it sees only zero-lag coherence. Echo that
  arrives more than one block late looks incoherent, and the gate stops learning on the echo itself.
  Any per-bin statistic has to be computed after the delay alignment, or over a window that covers the
  echo's full spread.

Next attempt: start from the block path's constraint and normaliser fixes above, or from a detector
that reads only the mic and the reference. Test with `echo_path`, including the ignored double-talk
test. Keep the time-domain path until a new one meets far-only ERLE of at least 38 dB and double-talk
within 1.5 dB.

## Jitter buffer

- **Inputs.** Each packet carries a sequence number, an RTP timestamp, the payload and the arrival
  time.
- **Delay target.** The 95th percentile of arrival jitter over the last 200 packets, clamped to
  20 to 200 ms. The target grows at once and shrinks by 2 percent per packet.
- **Start.** Playout starts one target delay after the first packet of a stream.
- **Missing frames.** If the next packet is present, FEC recovers the missing frame from it.
  Otherwise the decoder conceals the frame with PLC. After 25 concealed frames in a row, missing
  slots play as silence. When the buffer runs dry, it restarts and waits for the next stream.
- **Late packets** that arrive after their slot are dropped and counted as late. A missing slot
  counts as lost, so a late packet is counted as lost at its slot and again as late when it
  arrives.
- **Delay adjustment.** When the buffer is shallower than the target, playout holds for one tick
  and conceals that tick, adding 20 ms. The hold happens at most every three ticks. When the
  buffer is deeper than target plus 60 ms and the next packet is silence, one silent packet is
  skipped. A speech packet is skipped only when the buffer is deeper than target plus 120 ms.
- **Measured.** With 0 to 80 ms of jitter and 5 percent loss over 30 seconds, the delay converged
  to 75 ms, and no tick inside the stream was silent. A jitter onset raised the target to about
  76 ms within a second. The target fell to 49 ms one second after the jitter stopped, and to the
  20 ms floor after 20 seconds.
- **Stats** are the current target, buffered audio, received, late, duplicate, lost, FEC-recovered,
  concealed, silenced, stretched and compressed counts.

## What the tests cover

- Codec round trip on a sine sweep with noise: 12 dB at 32 kbps and 12 dB at 16 kbps.
- FEC against PLC for a single dropped frame, as above.
- Jitter buffer on cadence, in order, with loss and jitter, and with Opus.
- End to end over PCM. Each output frame names its packet, so ordering and duplicates are checked
  exactly under 5 percent loss and 0 to 80 ms jitter.
- End to end over Opus. Output power matches the input within 2 dB over the run, and within 6 dB
  over every half second after the first.
- The `#[ignore]` device test lists devices. It is not run in CI.

## Integration steps

For `dissent-media`:

1. Send one media frame per 20 ms. Its bytes are the audio-level byte followed by the Opus packet,
   from `SenderPacket::to_media_frame`. DTX frames are one or two bytes, so the transport must
   accept small frames.
2. The jitter buffer needs a sequence number and a timestamp per frame. `Mesh::next_frame` returns
   only the sender and the bytes today. Either prefix each frame with a 32-bit frame counter, or
   expose the RTP sequence number and timestamp from str0m.
3. Receive with `VoiceReceiver::push_media_frame`, passing the sender id, sequence, timestamp,
   arrival time in ms, and the frame bytes.

For the desktop app:

1. Open devices with `CaptureStream::open` and `PlayoutStream::open`. Use `list_inputs` and
   `list_outputs` for the settings screen.
2. Run the pipeline on a dedicated thread, not on the async runtime. Every 10 to 20 ms, call
   `CaptureStream::read_48k` and feed `VoiceSender::push_capture`. Send each returned packet.
3. Every 20 ms, on a monotonic clock, call `VoiceReceiver::pop_mix`. Write the result to
   `PlayoutStream::write_48k`, and pass the same frame to `VoiceSender::push_far_reference`.
4. Wire mute and push-to-talk to `set_muted` and `set_ptt_pressed`. Turn echo cancellation on
   with `enable_echo_cancellation` when the output is speakers, not headphones.
5. Read `VoiceReceiver::stats` and `CaptureStream::overflows` or `PlayoutStream::underruns` for
   the connection view.
