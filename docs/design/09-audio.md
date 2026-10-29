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

- **Correlation detector on the reference, time domain, reverted.** A running normalised correlation over
  20 ms between the mic and the reference aligned to its echo (`LAG_MARGIN` before the filter's newest sample),
  gated on the far end being loud. Adaptation froze, and the residual suppressor backed off, while the
  correlation was below a threshold. Measured with `echo_path`, against 40.9 dB ERLE and -24.9 dBFS output
  (near-end voice -19.3 dBFS) at HEAD:
  - Threshold 0.75: far-end ERLE 12.5 dB. The detector fires during echo-only audio, so the filter never
    converges. Double-talk output -10.3 dBFS, louder than the near end.
  - Threshold 0.6: far-end ERLE 19.3 dB, output -12.8 dBFS.
  - Threshold 0.3: far-end ERLE 39.6 dB, the only setting that keeps the target, but the output is -15.6
    dBFS. That is 3.7 dB louder than the near-end voice, so echo leaks through during double talk. The
    detector hardly fires in double talk, so the near-end gain is not recovered.
  - Disabling the residual suppressor (floor 1.0) gave the same double-talk output, so the suppressor
    back-off is not the cause.
  - Setting the detector stopped the delay re-estimate during double talk. Then the delay never moved from
    zero, the detector never got the aligned reference, and it stayed set. That is a deadlock: a detector
    fed by the delay estimate must not also gate the estimate.
  - Offline, on synthetic signals, the normalised correlation of an echo-only block against its aligned
    reference is about 0.89 at the median, and about 0.70 in double talk. Reflections keep the echo-only
    value well below 1, so no single threshold separates the two cases cleanly.
  - Conclusion: a detector on the reference and the mic alone does not separate echo from near-end speech
    in this room. A detector has to model the echo's full spread, not one aligned lag, and that brings
    back the dependency on the filter's own output that the earlier attempts ran into.

- **Two-path (background and foreground) canceller, reverted.** The background NLMS adapts on every sample,
  the foreground produces the output and does not adapt, and the foreground takes the background's coefficients
  after the background's block error is 3 dB below both the foreground's and the mic's for three blocks in a
  row. The idea was that double talk makes the background diverge, so nothing gets copied. Measured with
  `echo_path`, against 40.9 dB far-end ERLE at HEAD:
  - Background step 0.3: far-end ERLE 7.7 dB, and 6.4 dB with a loud far end. The background converges
    within a few blocks, and it keeps moving at that step. A copy is stale by the next block, so the
    foreground stays about 13 dB behind the background's own error. Copies happen about every third block.
  - Background step 0.05: far-end ERLE 18.4 dB. Step 0.1: 14.8 dB. A slower background converges too slowly
    for the foreground to catch it.
  - Double-talk output stayed at or above the near-end level (-7.0 dBFS at step 0.3), so the copy rule did not
    protect the foreground either.
  - Conclusion: a foreground that only copies a background cannot track the echo. It needs to adapt too, and
    then double talk moves it, which is the problem the copy rule was meant to solve.

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

## Static in double talk: measurements

A user reports static that comes and goes while both parties talk. The measurements use `echo_path`, with a
room impulse response, 60 ms of speaker buffering, 100 ppm drift, and a speech-like far end and near end.

- **Divergence bursts.** In 353 double-talk blocks of 20 ms, 44 (12%) had output louder than the mic, 12 of
  them by more than 3 dB, and the worst by 6.6 dB. The output can only exceed the mic when the echo estimate
  adds energy, so the filter had diverged. The guard in `EchoCanceller::watch_divergence` rolls the weights back
  to the last converged snapshot when the error stays louder than the mic for 20 ms, and passes the mic until the
  error falls below half of it. With the guard: 20 blocks above the mic, 1 above 3 dB, worst 5.4 dB. Far-only ERLE
  is unchanged (40.9 dB at 0 ppm, 38.8 dB at 200 ppm, 40 ms buffering, as before).
- **Echo leak in double talk, not fixed.** The canceller removes almost no echo while both talk: the leak is
  0.1 dB below the echo in double-talk blocks, against 37.7 dB in far-only blocks. The residual suppressor is off
  in double talk by design (`near_present`), so this is the filter. Freezing the weights during near-end frames
  (an oracle, not a detector) leaves the leak at 6 dB below the echo, and freezing from 8 s with the near end
  silent drops far-only ERLE to 7.7 dB. Freezing the lag estimate as well, and removing clock drift, did not
  change that. The mechanism is not yet pinned down. This is the likely static: an echo that the filter does not
  cancel while both talk.
- **Near voice down about 10 dB in near-only blocks.** Measured as the best-fit gain of the output to the near
  end. Disabling the residual suppressor's floor gives the same result, so the suppressor is not the cause.

Counters, logged at debug level every ten seconds by `voice.rs` (`audio counters: ...`): late wake-ups of the 20 ms
tick (over 5 ms, with the worst lateness), speaker underruns (silence filled by the speaker callback), mic
overflows, samples at full scale after input gain, and the canceller's divergence events, bypassed samples and
suppressor gate flips. Stream errors that cpal reports are now logged as warnings. They were silently dropped
before. Not counted yet: jitter-buffer underruns and PLC frames, and noise-suppressor gate flips. A live call on two
dev instances is needed to see which counter moves with the static.

Open: the double-talk leak above. The divergence guard only handles energy added by a diverged filter, which is one
part of the static.

## Echo after switching processing off and on mid-call

Report: after the other party set Voice Isolation to Studio and back during a call, they heard themselves as echo.
Cause: Studio turns echo cancellation off, and the canceller was kept. While it was off, the reference kept
being pushed but the canceller did not run, so its reference position and the mic position drifted apart by the
length of the off period. The canceller then read the far end from the wrong moment. Reconnecting built a new
canceller, which is why a new call cleared it. The test `echo_recovers_after_processing_is_switched_off_and_on`
reproduced it: ERLE stayed at 0 dB for the rest of the call after switching back on.
Fix: `CaptureChain::set_settings` drops the canceller whenever the echo setting changes, and the next frame builds
a new one, as call start does. Measured: ERLE 12.5 dB in the first two seconds after switching back on, 36.5 dB at
two to four seconds, 41 dB after that. The never-switched call holds about 43 dB.
Other mid-call changes: device switches restart the voice worker, so they build a new chain. Mute stops both the
reference and the mic frames, and `echo_recovers_after_the_mic_is_muted_and_unmuted` checks that unmuting
recovers. Noise suppression and AGC changes keep the canceller, as they should.

## Double talk: options compared (design, before the sonora engine)

Measured here, before choosing: the 10 dB near-voice loss is not a measurement artefact. The controls in
`measure_near_only_loss_controls` (same chain, same metric): with no echo and a silent far end the near voice
passes at 0.0 dB, so the harness and its level calibration are right. With the far end talking and no echo path
at all, the loss is 3.1 dB per block, with a best-fit near gain of -10.6 dB. The filter is fitting the near
voice's chance correlation with the far end. With 4608 taps and a 96 ms window, the NLMS has enough freedom to do
that. So the loss is the canceller's, and it appears whenever the far end talks.

Double-talk leak is the other half: 0.1 dB of cancellation in double-talk blocks, against 37.7 dB in far-only
blocks (`measure_echo_leak_in_double_talk`). The divergence guard, already in, fixes the energy bursts, not this.

| Option | Double-talk behaviour (expected, not measured here) | Licence | Build cost | Risk |
|---|---|---|---|---|
| Speex MDF (`aec-rs` 1.0.0 with `aec-rs-sys`, which bundles speexdsp) | Variable step from the residual-power trend and a leak estimate (`mdf.c`: `Pey`, `Pyy`, `leak_estimate`), plus a foreground and background filter with a backtrack on `Davg1`/`Davg2`. No explicit double-talk detector, but adaptation slows when the residual stops falling. Better than fixed-step NLMS, probably short of AEC3 | `aec-rs` MIT. Bundled speexdsp uses the Xiph BSD-style COPYING (header read; the full text still needs checking before shipping) | cmake and a C compiler on every CI runner, plus bindgen, which needs libclang on Windows, macOS and Linux | Fixed-point C with an unsafe FFI boundary. Old code, fixed filter sizes, and the crate is a thin wrapper, so we own the integration |
| Pure-Rust AEC3 port (`aec3` 0.4.0) | The WebRTC AEC3 design: a separate dominant near-end detector, a linear model that is frozen during near-end, and a residual suppressor. Expected to be the best double-talk behaviour of the four, but this is the port's claim, not a measurement | MIT OR BSD-3-Clause, with WebRTC's patent grant (PATENT file) | Pure Rust, no C. New dependencies: `rustfft` and `num-complex` (already in our lock file), `crossbeam-channel` (also there), and `log`. The crate is edition 2024, which our rustc 1.99 builds | The crate says it is a work in progress. Its only integration test is a smoke test with no double-talk metric. Port fidelity, CPU cost and the API are unmeasured |
| `webrtc-audio-processing` 2.1.0 (C++ WebRTC, AEC3 included) | Same algorithm family as the port, with the mature tuning. Expected best behaviour | The WebRTC COPYING is BSD-3-Clause with the patent grant. The wrapper's `license-file` field is non-standard, so read it | The bundled build runs meson and ninja with a C++ toolchain, and its build script calls a Unix `cp`, so Windows needs MSYS-style tools. The non-bundled build needs a system library through pkg-config, which means Linux only. Adds minutes to every CI job and to the installers | High on Windows. The build script is the main obstacle |
| NLMS with a variable step (our filter, plus a step that falls as near-end power rises) | Limited. Every detector we have tried is documented above as failing or as too weak. Only a shadow-filter scheme, which MDF also uses, is untried | Ours | None | Low cost, but repeating a known dead end is the main risk |

Recommendation:
1. Keep the NLMS, the divergence guard, the counters and the mode-switch fix in v0.1.12. The double-talk limit is known and documented.
2. Next, spike the pure-Rust AEC3 port (`aec3` 0.4.0) behind the echo harness, with no change to the product path.
   Measure double-talk leak, near-only loss, ERLE at 40 ms and 200 ppm, and CPU per 20 ms frame. Read the
   source for the license grant and for the port's fidelity to WebRTC's `echo_canceller3.cc`.
   Adopt it only if double talk improves by at least 20 dB of leak and the near-only loss falls under 1 dB.
3. If the port fails, the fallback is `webrtc-audio-processing`, and its build cost must be planned for Windows
   first: vendor the meson build or pre-build the library, and do not start from the bundled path as shipped.
4. Speex MDF is the middle option if we want C without WebRTC's build. Its benefit over NLMS is unmeasured.
   Do not take it before the AEC3 spike.

## sonora spike (off by default, feature `sonora`)

`sonora` 0.2.0 and `sonora-aec3` 0.2.0 (BSD-3-Clause, pure Rust, WebRTC APM M145) are in the workspace behind the
`sonora` feature of `dissent-audio`, which is off by default. The measurements are in `tests/echo_path.rs`
(`sonora_spike` and `sonora_final` modules, `--features sonora --ignored --nocapture`, CPU in `--release`).

Alignment matters. Sonora's capture path delays its output by about 425 samples (9 ms), and our NS path by 480
samples. The first runs compared unaligned output and gave misleading numbers (a near-only loss of 20 dB, which was
mostly the offset). Every figure below lags its output by the lag that maximises the correlation with the near end.

| Measure (AEC only unless stated) | Ours (NLMS) | sonora | Bar | Result |
|---|---|---|---|---|
| Far-only ERLE, last 2 s, 40 ms, 0 ppm | 40.9 dB | 51.5 dB | at least ours | Pass |
| Double-talk echo leak, near 0.2 (fitted near gain) | 2.4 dB below echo | 10.0 dB | 20 dB better than ours | Fail: 7.6 dB better |
| Double-talk, near 0.6 | -2.3 dB | 1.2 dB | 20 dB better | Fail: 3.5 dB better |
| Double-talk, near 1.0 | -3.0 dB | -1.7 dB | 20 dB better | Fail: 1.3 dB better |
| Near gain, double talk (best fit), near 0.2 | -13.5 dB | -18.3 dB | not worse than ours | Fail |
| Near-only loss, no echo, far silent | 0.0 dB | 0.12 dB per block | under 1 dB | Pass |
| Near-only loss, far talks, no echo path | 3.1 dB per block | 5.9 dB per block | under 1 dB | Fail |
| CPU per 10 ms frame, AEC only, release | 1200 us | 50 us | none | sonora about 24 times cheaper |

Noise suppression, aligned, quiet blocks and loud blocks: ours removes 7.3 dB of noise at 4.8 dB speech SNR;
sonora (Moderate) removes 15.0 dB at 2.0 dB speech SNR. The two suppressors ran at different strengths, so
this is a trade-off, not a verdict.

AGC2 with adaptive digital gain on (it is off by default in sonora): from a quiet -40 dBFS input, ours reaches
-27.3 dBFS and sonora -25.1 dBFS. From a loud -10 dBFS input, ours reaches -20.6 dBFS and sonora -9.6 dBFS.

Verdict: sonora meets the far-only ERLE bar and is much cheaper than our NLMS. It misses both double-talk bars:
the leak improves by about 8 dB at most, and the near voice is attenuated more than ours when the far end talks.
Per the agreed plan, the OS built-in AECs (VoiceProcessingIO, the Windows communications APO, the PipeWire
echo-cancel module) are the next options to compare, and aec3 stays the backup.

## Echo engine choice: Standard or WebRTC AEC3 (experimental)

Settings › Voice has an "Echo cancellation engine" choice, Standard (the default) or WebRTC AEC3 (experimental).
Standard is ours: the NLMS canceller, RNNoise and our AGC. WebRTC AEC3 is sonora 0.2.0 (`dissent-audio` feature
`sonora`, enabled in `dissent-desktop`): its echo canceller, noise suppression (Moderate, or High at the strong
setting) and AGC2 with adaptive digital gain replace ours. Our gate, level meter and fade stay in front of the voice.

Switching mid-call rebuilds the chain's engine state, as the profile switch does: a change of engine or of the echo
setting starts a fresh canceller and a fresh WebRTC pipeline. `echo_recovers_after_the_engine_is_switched_mid_call`
switches Standard, WebRTC and back to Standard. WebRTC reaches 42.6 dB ERLE two to four seconds after the switch, and
Standard reaches 36.5 dB two to four seconds after switching back. Without the feature, asking for WebRTC gives
exactly the Standard output (`the_webrtc_engine_falls_back_to_standard_without_the_feature`).

The 10-second counter line carries `engine`, the engine that actually runs.

Tuning: sonora exposes only AEC3's default configuration, its transparent-mode choice (legacy or HMM) and the
high-pass switch. Across those, the near gain at a near level of 1.0 moves by at most 2 dB (best -5.2 dB, default
-6.8 dB, ours -3.8 dB). The levers that matter for double talk (dominant near-end detection, the near-end suppressor
tuning) need a block-level driver on `sonora-aec3`'s `BlockProcessor`. That is not built, so no preset was tested.

Cost, measured:
- Windows: `cargo clippy -p dissent-audio` and `-p dissent-desktop` with `-D warnings` pass, with and without the feature.
- Linux (Docker, `rust:1` with the CI package list): `dissent-audio --features sonora` and `dissent-desktop` pass
  `clippy -D warnings`. The Linux desktop build needs the PipeWire and clang packages from `release.yml`, which the
  CLAUDE.md list does not include.
- macOS: not verified here. `cargo check --target x86_64-apple-darwin` fails in `dissent-audio`'s build script, which
  needs a macOS C compiler that this Windows host lacks. The macOS CI job is the check.
- Binary size: release `dissent-desktop.exe` is 65,388,544 bytes with the feature and 64,758,272 bytes without.
  The feature adds 630,272 bytes, about 1%.
- CPU: AEC only, release, measured earlier: sonora about 50 us per 10 ms frame, our NLMS about 1,200 us.

## Speaker underruns: real, and fixed

What the counter counts: `PlayoutStream`'s underrun counter goes up once per output frame that the callback
needs and the ring cannot supply, and that frame plays silence. It is per frame (per stereo pair), not per callback
and not per channel sample. So it is a count of silent frames, which are audible as clicks or static if they
come in runs.

Cause, confirmed on the real 48 kHz output. The voice thread writes 20 ms (960 frames) every tick, and the output
callback drains the ring continuously. There was no cushion, so the ring was empty just before each write. The
speaker's clock also ran about 0.7% faster than the 20 ms cadence, so the ring drained even with a cushion.

Measured with `speaker_underruns_at_the_voice_cadence` (ignored, needs an output device), counting only during the
writes (the test's drain after the last write is not counted):
- no cushion, no trim (the original behaviour): 672 underruns in 30 s of writes, with the capture chain not running
- debug build with the capture chain (echo cancellation and noise suppression) running before each write, same
  device: original 1,536 in 30 s (about 51 per second), fixed 0
- fixed: 0 in both

The fix in `device.rs`: a 60 ms cushion of silence is queued before the speaker starts (`PLAYOUT_CUSHION_MS`), and
the write rate follows the queued level. `LinearResampler::set_speed` trims the rate by up to 2% (`PLAYOUT_TRIM_MAX`)
in proportion to the error from the cushion (`PLAYOUT_TRIM_GAIN` = 0.5). This keeps the queue near 60 ms, so a late
tick of up to about 60 ms plays without a gap. A fixed cushion alone is not enough, because the speaker's clock
drifts (a 200 ms cushion still lost 576 frames over 30 s). `PlayoutStream::buffered_frames` reports the queue.

Not reproduced here: the live loopback counts from the desktop debug builds (about 500 per second on each side,
with the other counters at zero). This bench reaches about 51 per second under the same conditions, so the live
pipeline has a source this bench does not model. The fix removes the bench's underruns, and the live counter line
will show whether the live ones are gone.

### Live check: two debug instances, DM call, 60 s each side

Two debug desktop instances on loopback (`DISSENT_LOOPBACK=1`, `DISSENT_FAKE_CAPTURE=1`, separate data folders
and WebView2 profiles), a DM call with the microphone and speaker of one machine. Counters read from the 10 s
`audio counters` lines.

- Cushion 60 ms: zero speaker underruns in every window after the first. The first window (speaker open) showed
  576 frames on the caller.
- Cushion 40 ms, before the microphone change: zero in every window after the first. The first window showed 576
  frames on the caller and 96 on the callee.
- The first-window shortfall came from the voice thread's start-up order. Opening the microphone blocks for 32 to 54
  ms (the log timestamps), and it ran after the speaker had started, so the first playout write came late. The fix
  opens the microphone before the speaker when capture is on at the start (`apps/desktop/src-tauri/src/voice.rs`).
- Cushion 40 ms, after the microphone change: zero underruns in every window on both sides, including the first.

Chosen: 40 ms, so mouth-to-ear delay is 20 ms less than the 60 ms cushion.

### Mid-call microphone: mute, device switch, and a two-instance artefact

The microphone now opens on its own thread (`MicFeed` in `apps/desktop/src-tauri/src/voice.rs`), so its open never
blocks the voice loop. It opens once when the voice thread starts and stays open for the whole call. Mute does not
open or close it: muted frames still go through the chain, so its echo state stays in step, but nothing is sent. A
device change restarts the worker, as before.

Measured with two debug instances on one machine (DM call, caller on the Focusrite input after a switch, callee
without a usable microphone where stated):

- Helper thread alone, mute still closing the device: the five mute toggles gave 327 underruns on the caller in one
  window, and a second round gave 2,484, 516 and 65, with late ticks. The helper thread did not remove the stall.
- After the mute change, ten toggles on the caller with the microphone open: zero underruns and zero late ticks on
  the caller in every window.
- The same run had a device switch to the Focusrite input. The caller stayed clean. The callee logged 839 underruns
  and 20 late ticks in one window about 15 s after the switch.
- Both sides with no microphone open (a temporary build, since removed): ten toggles and a switch gave zero
  underruns on both sides, with at most one late tick.
- Both sides capturing from the default webcam microphone, no switch: underruns at call start and during the toggles
  (caller 32 and 110, callee 61, 502, 33 and 3), with zero or one late tick. The shared Focusrite clock is not the
  cause, since this run did not use it.
- Only the caller capturing (the callee's input set to a device that does not exist, so its capture retries): ten
  toggles on the caller gave zero underruns in every window, with one late tick.

Reading: with two instances on one machine, both sides capture and play through the same audio engine. The underruns
appear when both capture, and the caller stays clean when only it captures. Treat the remaining mid-call underruns as
an artefact of two instances on one machine. They are not confirmed for a call between two machines, which is the
real setup and cannot be run here. If two machines show them, the next step is to move the microphone read to a 5 ms
pump with a larger playout cushion, and measure the mic-read path separately from the open.
