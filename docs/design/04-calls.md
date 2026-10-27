# Design 04: voice and video calls (spike)

Status: **spike**. The crate `crates/dissent-media` proves the media path on loopback. Nothing in the app calls it yet.

Scope: phase 4 of the architecture spec. This note covers 1:1 calls first, then the mesh and peer-SFU tiers that build on the same pieces.

## What the spike proves

1. Two peers complete a WebRTC offer and answer, ICE and DTLS, using only bytes handed to and from the caller.
2. Every audio frame is encrypted with SFrame (RFC 9605) before str0m packetizes it.
3. A peer with the wrong media key receives packets but cannot decrypt any frame.

Audio is synthetic. Each frame is 160 bytes, one per 20 ms, with no Opus codec and no audio device yet.

## How a 1:1 call is set up

The media crate never touches the network for signalling. It returns a `Signal` value, and the app sends its bytes over whatever channel it has.

1. The caller calls `Call::start_offer`. This binds a UDP socket, adds a host candidate, and returns `Signal::Offer`.
2. The caller sends the offer bytes to the callee.
3. The callee calls `Call::accept` with the offer. This returns `Signal::Answer`.
4. The callee sends the answer bytes back.
5. The caller calls `Call::apply` with the answer. ICE and DTLS then complete on their own.

Host candidates ride inside the offer and answer, so the spike never sends trickled candidates.

## Transports

A call moves datagrams through a `PacketTransport`. The call logic does not depend on which one it gets.

- `UdpTransport` is a plain UDP socket. It serves loopback tests and direct IP paths.
- `LinkTransport` carries the datagrams as QUIC datagrams on an iroh connection, under the ALPN `dissent/media/0`. This is the path for calls across NAT. A test runs a full call between two `dissent-net` nodes over it, with relays disabled and a direct path.

iroh does the NAT work. It hole-punches a direct path when it can, and falls back to a relay when it cannot. A relay forwards only encrypted QUIC datagrams and cannot read media. Calls over iroh need no STUN or TURN servers.

str0m still needs candidates, so each side advertises one synthetic host candidate. The caller uses `10.255.0.1:1` and the callee uses `10.255.0.2:1`. The transport reports every received datagram as coming from the peer's synthetic address, and sends every outgoing datagram over the iroh connection. The addresses are never routed.

We use full ICE with that single candidate pair, not ICE-lite. ICE-lite suits a server with a public address that answers checks and runs none of its own. Both peers here are full agents, and the transport already offers exactly one path, so the checks succeed on the first try.

**Transport.** The spike does not choose a transport. The two candidates in the spec are:

- The existing direct device connection that DMs use. This is addressed to one peer and already end to end encrypted between devices. It is the likely choice for 1:1 calls.
- Community gossip, as the spec's media row says. It fits group calls better.

Signalling is small and happens once per call, so either works. The choice is for the app layer, not the media crate.

## Where the keys come from

Each call uses one 32-byte secret, the `MediaKey`. The media crate does not create it.

In Dissent the secret comes from the MLS exporter of the group that the call belongs to. A 1:1 call between two people uses a two-member MLS group, such as the DM group. Both peers export the same secret for the call.

Per-sender keys come from that secret. RFC 9605 section 5.2 defines the derivation, and the `sframe` crate implements it:

- The export label is fixed by RFC 9605 as `SFrame 1.0 Base Key`. The spec's earlier sketch used `dissent/sframe`. The RFC label wins, because the derivation requires it.
- The SFrame key ID is built from the sender's MLS member index. Every sender in the call therefore has its own key, and the receiver learns the sender from the authenticated header.
- The spike uses MLS epoch 0 and gives the member index 24 bits, so sender IDs run up to 16,777,215.

The MLS epoch belongs in the key ID. When the group changes, the epoch changes, and old frames stop decrypting. Rekeying during a call is not built yet.

## Frame path

Sender side, for each frame:

1. SFrame encrypts the plaintext with the sender's key. The output is the SFrame header followed by the ciphertext and tag.
2. str0m writes those bytes as the RTP payload on the audio track, using the Opus payload type.
3. DTLS-SRTP protects the RTP packet on its hop to the peer.

Receiver side: SRTP and DTLS are removed by str0m, then SFrame decrypts the payload. A frame that fails decryption is counted and dropped, and never reaches the app.

## Frame stamp: sequence number and timestamp

Every frame's encrypted plaintext starts with a 9-byte stamp:

- **Sequence number** (u32): the sender's frame count, from 0. It keeps counting across a rekey and across a forwarder failover, so a jitter buffer sees consecutive numbers.
- **Timestamp** (u32): the sender's sample clock at 48 kHz. Frames sent without an explicit timestamp use 960 samples per sequence number, which is 20 ms.
- **Audio level** (u8): the RFC 6464 level, also in the clear as the first byte of the wire frame. The stamp copy is authenticated, and it is what a receiver reports.

The stamp is inside SFrame encryption, so a forwarder cannot change it, and a forged stamp fails the tag. Forwarders pass the bytes on unchanged, so failover needs no re-stamping. A frame recovered by redundancy carries its own stamp.

The cost is 9 bytes per frame, about 3.6 kbps at 50 frames a second, before SFrame's own header and tag.

API:

- Send: `send_audio_frame(frame)` derives the timestamp. `send_audio_frame_at(frame, timestamp)` takes the sender's clock. Participants take the level as well.
- Receive: `next_received_frame()` returns `ReceivedFrame { sender, seq, timestamp, audio_level, payload }`. The older `next_frame()` returns `(sender, payload)` and is a thin wrapper, kept for one release.

## What a forwarder sees

A forwarder is any peer that relays media for others, such as a peer-SFU. In the spec this is a member with good upload.

A forwarder terminates DTLS-SRTP with each peer, so it sees RTP headers in the clear. It sees:

- RTP sequence numbers, timestamps, and SSRCs, which identify streams.
- Packet sizes and timing, which show who is talking and roughly how loud.
- The SFrame header: the key ID, which names the sender's MLS member index, and the frame counter.

A forwarder does not see the MLS secret or any SFrame key. It cannot decrypt a single frame. The forwarder can route by sender and forward encrypted frames unchanged. This is the point of SFrame: SRTP protects each hop, and SFrame protects the content end to end.

Each frame also carries its audio level, one unencrypted byte of SFrame metadata. The forwarder reads it to choose speakers, and it is authenticated, so a forwarder cannot change it. The leak is described under Peer-SFU below.

## Mesh (implemented)

**Mesh (2 to about 6 video, about 15 audio).** `Mesh` holds one call per remote peer. All of them share the call's media key, and each peer's frames carry that peer's MLS member index. The implementation is in `crates/dissent-media/src/mesh.rs`.

- **Who offers.** The lower peer id offers and the higher peer id accepts. Each pair therefore has exactly one offer, so there is no glare.
- **Encrypt once.** A frame is SFrame-encrypted once, and the same bytes go to every peer. The sender pays one encryption per frame. Upload still grows with N-1 copies, which is the mesh cost the spec accepts.
- **Failed peers.** A peer whose link fails or closes is dropped from the mesh. The other peers keep working, and `next_frame` never blocks on a dead peer.

**Rekey.** `Mesh::set_key(new_key, new_epoch)` runs when the MLS group changes, for example after a ban or a join.

- The key id carries the low 8 bits of the MLS epoch. The new epoch must differ from the current one in those bits.
- Receivers keep the previous epoch's key for a 2 s grace period, then drop it. A frame from an older epoch fails to decrypt. Only one previous epoch is kept, so a rekey cannot leave two old keys alive.
- A removed peer does not get the new key, so frames sent under the new epoch are undecryptable for it.
- Limit: a removed peer keeps every key it already held. It can still decrypt anything sent under the old epoch that it receives. Forward secrecy starts at the rekey, not at the removal.

**Tests.** A 4-peer mesh delivers all 3 x 50 frames to each peer, with correct sender ids. A removed peer leaves the rest of the mesh working. A removed peer whose link still runs after a rekey decrypts none of the new frames. A 2-peer mesh works over a real iroh link on loopback.

## Peer-SFU (implemented)

A peer-SFU is one participant that forwards the call for the others. The spike implements the election, the keyless forwarder, and failover. The code is in `election.rs`, `sfu.rs` and `participant.rs`.

**Election.** Every peer runs the same function on the same gossiped candidate set, so all peers agree without a vote. Candidates rank by these rules, in order:

1. not on battery,
2. wired,
3. higher upload,
4. CPU ok,
5. lower median RTT,
6. lower peer id, as the final tie-break.

A candidate is eligible only if its capacity covers the call. Capacity comes from the spec's bitrates. Video at 720p costs about 1.5 Mbps per stream, so 20 Mbps covers 13 streams. Audio costs three speakers at 32 kbps per listener, so 20 Mbps covers 208 listeners.

**Forwarding.** `Sfu` is built without a media key, and its constructor takes none. Each participant has one link to the forwarder. The forwarder writes each received frame, unchanged, to every other participant's link. In audio mode it forwards only the top three speakers, ranked by audio level. A frame counts as speech only if its level is 50 or lower on the RFC 6464 scale. Without that threshold, a forwarder that has just started can forward silence, because it has not yet heard the speakers. Video mode forwards everything.

**What the forwarder sees.** The forwarder reads the audio-level byte of every frame, the sender of each frame by link, the frame sizes and timing, the RTP headers, and the SFrame key id. The key id holds the sender's MLS member index and the epoch bits. It cannot read the audio. The level byte is SFrame metadata, so it is authenticated but not encrypted, and any change to it breaks decryption. This leak is deliberate: the forwarder needs the level to choose speakers. Who speaks, and how loudly, is therefore visible to every forwarder. That goes into the privacy review.

**Failover.** Each participant keeps links to a pool of four forwarders, so a backup is connected before it is needed. Only the active forwarder receives frames, so the backup stays idle. The participant switches when one of three things happens:

- the active forwarder's link closes, which is a link error,
- a send through it fails,
- no frame arrives from it for 500 ms, which is silence.

On a switch, the best remaining pool member becomes active and the next one becomes backup. The switch needs no handshake. The new forwarder learns the speakers from the frames that now arrive. The spike has no separate announcement message, so the switch is visible only through those frames.

**Measured.** The test runs 20 participants in audio mode, over in-memory links. Three of them speak. The primary forwarder is lost two seconds in. Recovery is the time from the fault until frames from every remaining speaker arrive again.

| Fault | Recovery, max | Recovery, median | Longest gap |
| --- | --- | --- | --- |
| Forwarder killed | 30 ms | 23 ms | 54 ms |
| Forwarder frozen | 545 ms | 535 ms | 560 ms |

Across runs, the kill case stayed between 30 and 37 ms. The freeze case stayed between 520 and 545 ms, which is the 500 ms silence timeout plus detection. Both cases had zero plaintext leaks at the forwarder and zero undecryptable frames at the participants. Both are well under the two-second exit criterion.

A two-participant call through one forwarder also ran over real iroh links on loopback. Each participant heard all 75 frames the other sent.


## Stages: cascaded forwarders (100+, implemented)

Calls of 100 participants and more use a forwarding tree. The code is in `cascade.rs`, `sfu.rs` and `participant.rs`.

**Shape.** The tree has two levels of forwarders: a root, and child forwarders under it. A frame from a speaker crosses at most three forwarders: the speaker's child, the root, and the listener's child. That is the hop count the spec's depth-3 limit allows.

**Election.** `plan` ranks the candidates with the same `elect` and `capacity` as a single forwarder. The best two become the root and its standby. The next `child_count` become children, and each child must carry its own share of listeners plus a sibling's share, because it takes over a failed sibling's listeners. Each listener gets a primary child and a standby child, round robin in id order, so a failed child's listeners fall to a sibling that already holds a share.

**Routing.** Each forwarder routes by the sender named in the unencrypted key id.

- A frame from a downlink (a listener or a child) whose sender is a top speaker at that forwarder goes to the other downlinks and to the active parent. The sender's own link never gets it back.
- A frame from the active parent goes to every downlink. The parent already chose it, and the sender's own link is skipped.
- Digests follow the same rules as frames, so every listener still verifies every speaker.
- A standby parent stays connected and sends nothing until it is promoted, so failover needs no handshake.

Speakers send to their own child. Listeners receive from their child only. Each listener also stays connected to its standby child, but only the active child's media is consumed. The standby's copy of a frame is dropped before any state sees it, so it cannot stand in for the active copy.

**Top speakers at each level.** Each forwarder picks its top three speakers from its own downlinks. The root picks the global top three from what the children send up. A listener can hear up to three local speakers and three remote ones when speakers are split across children, so in the worst split a listener hears six. With all three speakers under one child, or any split that leaves one child with fewer than three local speakers, it hears three.

**Failover.** A child's listeners move to their standby child, using the same failover as a flat forwarder. When the root fails, every child's active parent moves to the standby root, which is already connected, and the children re-parent without a handshake.

**Scheduling.** Forwarders poll their links from a rotating start, so a busy downlink cannot starve the upstream link. Without this, the upstream link never runs its handshake or timers, and its frames are dropped.

**Measured.** Release build, 150 participants, three speakers, five children, each with a standby, plus a root and standby root. Speakers talk for five seconds, and the fault comes two seconds in. Recovery is measured for the affected listeners, as in the flat failover test.

| Fault | Affected listeners | Recovery, max | Recovery, median |
| --- | --- | --- | --- |
| Child forwarder killed | 30 | 31 ms | 20 ms |
| Root killed | 150 | 33 ms | 17 ms |

Every listener heard all three speakers, and no listener heard a non-speaker, including the sender itself. The hop count is three by construction, and the test checks the topology before the call starts. Per-forwarder load was between 8,700 and 9,100 packets seen at each child, with about 800 at the root, and 13,600 at the child that absorbed a failed sibling's listeners. Authentication held end to end: no impersonation, no undecryptable frame, and no plaintext seen by any forwarder. In the honest run no frame went unauthenticated. The fault runs left about one batch unauthenticated per listener, which is the batch in flight when the forwarder died. Its digest was lost, so it counts as tolerated and mutes no one.

**Debug builds.** In a debug build the 150-participant run saturates the CPU. Digests then arrive after the 500 ms verification window, and honest frames are counted as unauthenticated. The same scenario in release has none. For that reason the 150-participant tests are marked ignored, and they run with `cargo test --release -p dissent-media --test cascade -- --ignored`. A 20-participant version runs in every debug pass.

**Not measured here.** The forwarders are dedicated peers in the test, not participants that also speak, and the test runs in memory, so latency over real networks is not measured. A child that is also a speaker needs a local path, which the spike does not build.

## Replay and sender authentication

This section answers findings 4 and the impersonation risk in the threat model. SFrame alone cannot tell members apart, because every member holds the same media key (RFC 9605 section 7.2). The design below adds a signature by each sender's device key. The code is in `auth.rs`.

**Digests.** A sender hashes every encrypted frame. After every ten frames, or 200 ms, whichever comes first, it sends a digest: the sender id, the epoch, the first counter, and the 16-byte blake3 hash of each frame in the batch. The digest is signed with the device key over the domain `dissent/sframe-auth/v1`. Digests travel on the same transport as frames, marked by a reserved first byte, and forwarders relay them unchanged.

**Receiving.** A receiver plays each frame as it arrives. When the digest for its batch arrives, the receiver checks the signature against the sender's device certificate, which chains to a root. Three outcomes follow:

- A frame whose hash matches its digest is verified. Its counter then enters the replay window.
- A frame that no digest covers within 500 ms counts as unauthenticated. A lost digest causes this. It is tolerated, and it never mutes anyone.
- A frame whose hash differs from the digest for its counter is a forgery. Its sender is flagged as impersonated and muted for the rest of the epoch. From then on the sender's frames play only after a digest verifies them. The real sender keeps playing, with up to one batch of delay.

A digest whose signature does not verify is also treated as a forgery in the sender's name.

**Replay.** The replay window counts only verified frames. A forged frame cannot move it, so it cannot block the real sender's later frames. The window is shared by every link of a participant, so a frame replayed on a second forwarder is also caught.

**Residual risk.** Forged audio can play for up to about 200 to 500 ms before it is flagged. The batch closes at ten frames, and the forgery is flagged when that batch's digest arrives. A forgery that reuses a counter the victim will send is flagged at that digest. A forgery more than 50 counters ahead of the last verified counter is held and never plays. It is counted as unauthenticated but not flagged, because a sender whose digests were lost looks the same for a while, and muting on that would punish honest senders. Frames that play before their digest also cannot be recalled after a flag.

**Cost.** Audio digests cover 25 frames, about 500 ms, with 8-byte frame hashes. A speaker's digest traffic measures 4.5 kbps, from the packets the sender writes over five seconds. That is above the 3 kbps target. Most of it is the 64-byte signature and the 8-byte hashes; four-byte hashes would bring it to about 2.9 kbps, at the cost of a weaker per-frame check. Video digests cover 10 frames, which the estimate does not include.

**Residual risk.** A forgery is flagged when its batch's digest arrives. With 25-frame batches that is about 500 ms after the forged traffic starts, and 25 forged frames can play before the flag. With 10-frame batches the flag comes after about 225 ms, with 10 forged frames played. The 25-frame case is measured in `auth_forgery.rs`.

**Tests.** The unit tests in `auth.rs` cover honest traffic, batch closing, a replayed frame before and after verification, out-of-order frames, too-old frames, a mismatch that mutes and still plays the real frame, a muted sender that plays only verified frames, a far-ahead frame, a lost digest, the 500 ms deadline, a forged digest signature, and plain links. The integration tests cover an honest authenticated mesh, an honest forwarded call, and an authenticated 1:1 call (`Call::offer_authenticated`), both with zero unauthenticated frames and zero flags. The forgery test runs a member under another member's sender id and a device key of its own. It is flagged 228 ms after the forged traffic starts, all 50 real frames play, and 10 forged frames played in the window before the flag.

**Limit of the cipher layer.** The cipher layer alone still lets any member send under another member's id. The `auth` layer is what detects it. A member with a device key can sign only its own digests, so it cannot forge another member's digest.

## Bandwidth, bitrate and layers

Each call runs a per-receiver estimator, `bwe.rs`. It takes the round-trip time that str0m reports, and the loss the peer reports, once per stats interval. It returns a target audio bitrate between 16 and 64 kbps.

- **Delay.** Queuing delay is the RTT above the lowest RTT seen. Above 40 ms the target falls by 15%. Below 10 ms and not rising, it rises 1 kbps.
- **Loss.** Above 10% the target falls by half the loss. Below 2% it rises 1 kbps.

The target reaches the audio pipeline through `Call::on_target_bitrate`, which fires each time the target changes. A clean in-memory call moved from 32 kbps to 41 kbps over three seconds, with five changes.

Forwarders choose a layer per listener with `pick_layer`, the highest layer under the listener's budget. Set the budget with `Sfu::set_listener_budget`. Simulcast frames are not carried yet, so the choice is ready but has nothing to switch between.

**Centrality.** `centrality` measures each candidate's median RTT from the other participants. `apply_centrality` writes it into the candidate's RTT, which `elect` uses after upload and CPU. Among otherwise equal forwarders, the most central one forwards.

## Audio redundancy

Each frame's payload starts with a flag. When redundancy is on, the flag is followed by the previous frame's counter, its hash, and its plaintext, so a receiver can recover a frame lost in transit. The forwarder does not change: the key ID and level byte stay where they were, and the extra data is inside the encrypted payload.

Redundancy turns on above 10% reported loss, and off again below 5%, so it does not flap. A test drops every fifth packet on a 1:1 call: without redundancy 238 of 250 frames arrived, and with it all 250 did. The cost is that each frame carries the previous one, so the payload roughly doubles.

## Video

Status: **phase 1 built**, in `crates/dissent-video`. The crate holds the capture sources, the AV1 codec pair, the bitrate policy and the send queue, each behind a trait. It is not wired into `dissent-media`, the desktop app or the UI. Phase 2 does that wiring.

### Codecs

- **AV1 encoder: SVT-AV1 v4.2.0**, through `svt-av1-sys`. The source is the `third_party/SVT-AV1` git submodule, pinned to commit `9292ec8e32bce26f781f277ec8739b53426c4300` (tag v4.2.0). `svt-av1-sys` builds it with CMake and generates bindings with bindgen, and `dissent-video` wraps the raw API. The old `shiguredo_svt_av1` crate is gone, because it fetched the source by tag at build time. The crate clones SVT-AV1 at build time with `git clone --depth 1 --branch v4.2.0` from gitlab.com. The tag is not checked against a commit or hash, so the source is pinned only as far as that tag is trusted. It builds with CMake, which the crate downloads from Kitware's GitHub releases and checks against a SHA-256 pinned in `shiguredo_cmake` 4.4.0. SVT needs NASM for its x86 assembly. The encoder sits behind the `svt-av1` cargo feature, which is off by default, so a plain build needs no NASM, libclang or CMake download. Without the feature a call offers H.264 only. Real-time coding at preset 12 is the measured choice for calls. rav1e is removed. It was 10 to 20 times slower and had no non-real-time use here.
- **AV1 decoder: rav1d 1.1.** A pure Rust port of dav1d, built for 8-bit with no assembly. dav1d's C library needs meson and NASM, so the Rust port is used instead. One thread and no frame delay, so a chunk yields its frame on the same call. The only `unsafe` in the crate is in `decode/av1.rs`, which wraps rav1d's dav1d-shaped API.
- **H.264 encoder and decoder: OpenH264** through the `openh264` crate. Its `openh264-sys2` build compiles the bundled OpenH264 source with `cc`. The build is pure C here: `OPENH264_NO_ASM=1`. Its NASM path compiles the assembly, but the objects never reach the static library. The link then fails with unresolved SSE symbols on MSVC. Pure C is slower, so H.264 is kept to the lowest rung, 360p.
- **VP8: not built.** H.264 is the fallback instead.
- **Negotiation.** `codec::negotiate(ours, theirs)` returns the codecs both sides support, in preference order. AV1 comes first whenever both sides have it. H.264 is picked when a peer lacks AV1. An empty result means no call. `VideoAdapter::with_codecs` keeps only the rungs for the negotiated codecs.

**Build requirements.** Only with `--features svt-av1`. SVT-AV1 is built from source on every platform, so each one needs these:

| OS | Needs at build time |
| --- | --- |
| Windows (MSVC) | NASM, LLVM's libclang (bindgen), git with `core.longpaths` on, and the Visual Studio C++ toolchain. CMake is downloaded by the crate. |
| macOS (arm64, x64) | NASM (Homebrew), git, and the Xcode command-line tools (Apple's clang and libclang). CMake is downloaded by the crate. |
| Linux | NASM and libclang-dev, plus git and a C compiler. CMake is downloaded by the crate. |

Build time: a cold SVT-AV1 source build takes about ten minutes on this Windows machine and runs once per target directory. Later builds reuse it.

The release workflow's `codecs` job installs these per OS in a marked block. It builds and tests `dissent-video` on Windows, macOS arm64 and Linux. macOS Intel is not built. There is no pinned Intel runner.

**Hardware encoders are next, not now.** NVENC, Media Foundation, VideoToolbox and VAAPI would each need their own binding and a runtime probe. `VideoEncoder` is the seam they plug into. They are out of scope for this phase.

**CPU cost.** Release build through the integrated path (`encode::open`, the same call the app will make), 1280x720, 1000 kbps, 30 frames of the noisy test pattern, mean encode time per frame including the final flush:

| Codec | Encoder | ms per frame at 720p | Share of the 33 ms budget |
| --- | --- | --- | --- |
| AV1 | SVT-AV1 v4.2.0, preset 12, real-time, CBR | 9.9 | 30% |
| H.264 | OpenH264, pure C build | 12.4 | 38% |

This 24-thread Windows machine was shared with other builds, so the figures are rough. The test pattern has more detail than screen content, so real AV1 cost is likely lower.

Reference numbers for SVT-AV1 and the old encoder, measured on the same host:

- rav1e (removed) took 83 to 106 ms at 720p and 118 ms at 360p with its defaults. Eight tile columns and look-ahead 1 brought 720p to 73 ms. It never reached the budget, even with tuning.
- ffmpeg's `libsvtav1` at preset 12 on the easier `testsrc2` content took 2.9 ms at 720p, 300 frames. The gap to 9.9 ms is the content, plus the wrapper overhead.
- Linux Docker, 14 cores: `libsvtav1` took 2 ms at 360p and 5 ms at 720p. Its numbers are not from the integrated path.

**Quality and rate.** At 160x90 and 30 fps, the default test drops from 65 kbps to 17 kbps within a second of a `set_bitrate(40)` call. The encoder spends well under a low target at this size, so the test checks the relative drop instead of the absolute rate. rate control is fixed when SVT-AV1 is created, so `set_bitrate` finishes the old encoder, keeps its chunks, and builds a new one. The new encoder starts on a keyframe. OpenH264 is rebuilt the same way.

### Capture

- **Camera:** nokhwa 0.10. Media Foundation on Windows, AVFoundation on macOS. V4L2 on Linux. It asks for the closest mode to the requested size and fps. The source scales what the device delivers to the requested size.
- **Screen and window:** xcap 0.9, which wraps the platform capture APIs on Windows, macOS and Linux. It lists displays and visible titled windows, and paces capture to the frame rate. A target is looked up on every frame, because xcap's monitor handle is not `Send` on Windows. On Linux a display is a stream, and a window is grabbed per frame.
- **Cursor:** xcap draws no cursor. `Cursor::Excluded` is the only supported mode, and `Cursor::Included` returns `VideoError::Unsupported`. The option is kept so it can light up when a backend can draw the cursor.
- **Test source:** `PatternSource`, which is deterministic and needs no hardware. It drives the tests and CI.

On Linux, camera capture uses nokhwa's V4L2 backend, and screen capture uses xcap: the X11 root window, or the desktop portal's ScreenCast on Wayland, which shows the system's own picker. Both need libclang, PipeWire, xcb, Wayland, EGL and gbm development packages at build time. The Linux test and desktop jobs in the release workflow install them.

### Rate control

Audio always keeps its reserve, and video gets what is left:

```text
audio_reserve = audio_target * 1.25 + 16 kbps
budget        = max(0, available - audio_reserve)
```

`VideoAdapter::update` takes one sample per call and returns a `VideoPlan` with the rung, size, fps and encoder bitrate.

| Content | Ladder, best first (size, fps, bitrate range) |
| --- | --- |
| Camera | AV1 720p 30 fps, 1000-2500 kbps; AV1 720p 15 fps, 600-1000 kbps; AV1 360p 15 fps, 250-600 kbps; H.264 360p 15 fps, 150-400 kbps |
| Screen | AV1 1080p 15 fps, 1500-3000; AV1 1080p 10 fps, 1000-1500; AV1 1080p 5 fps, 700-1000; AV1 720p 10 fps, 400-700; AV1 720p 5 fps, 250-400; H.264 360p 10 fps, 150-250 kbps |

- **Order of steps.** Bitrate moves first within a rung. Then frame rate drops. Then resolution drops. Screen content drops frame rate before resolution, so text stays sharp.
- **Pause.** Below the lowest rung's minimum, which is 150 kbps for both camera and screen, video pauses at once. It does not wait for a streak, so audio is protected immediately.
- **Hysteresis.** A step down needs three samples below the rung's minimum. A step up needs five samples above the minimum plus 20%. A bitrate increase needs a 10% change, while decreases apply on the same sample. A budget that hovers at a threshold therefore does not flap. The unit tests cover this.
- **Invariant.** A non-paused plan never sends more than `budget`, so video plus the audio reserve never exceeds the available bandwidth.

### Runtime downgrade

`CpuGovernor` decides from measured encode time whether the encoder keeps up at the current rung. It keeps an exponential average of encode time per frame. A run of ten frames with the average above 80% of the frame period moves the floor one rung down. A run of 60 frames where the next rung up would fit under 50% of its period moves it back up. Pixels times frame rate predicts the cost of the rung above. After each verdict the averages restart, since the encoder is rebuilt. `VideoAdapter::apply_cpu` raises or lowers the quality floor, and bandwidth still applies on top. The audio reserve is not changed.

The lowest rung of each ladder is H.264 at 360p. A slow AV1 encoder therefore falls back in three steps: frame rate, then resolution, then codec. Each step is one `CpuVerdict::Downgrade`. The floor only moves down the ladder, and an upgrade moves it back one rung at a time.

### Priority and pause

`SendQueue` is a pure component, with time passed in. Phase 2 puts it in front of the link.

- Audio is always sent before video and is never dropped for age.
- A video packet older than 200 ms is dropped, along with the frames before the next fresh keyframe. If no fresh keyframe exists, all queued video is dropped.
- After a drop, delta frames are refused until a keyframe arrives, since they depend on lost data.

### Bandwidth and SFrame

Video frames use the same SFrame media key and per-sender key IDs as audio (the sections above on keys and frame path). The frame header stays in the clear, so forwarders can still route by sender without decrypting. Video joins the sender-authentication digests: a digest batch covers ten frames or 200 ms, whichever is first. A keyframe can span many packets. A forwarder must forward a keyframe whole or not at all, which is the same rule as the queue's drop.

### Not built in this phase

- **Hardware encoders:** NVENC, Media Foundation, VideoToolbox and VAAPI. They are the next step. Each needs a binding and a runtime probe, and `VideoEncoder` is the seam.
- **VP8.** H.264 is the fallback instead.
- **Cursor capture.** xcap draws no cursor, so `Cursor::Included` returns `Unsupported`.
- **Wiring into `dissent-media`, the app and the UI.**
- **SVT log noise.** SVT-AV1 writes `Svt[info]` and `SvtMalloc[info]` lines to stderr. Set the `SVT_LOG` environment variable to a level above info (see `Lib/Codec/svt_log.c` in SVT-AV1) before the app ships.


## Video phase 2 plan

Status: **phase 2 built.** `dissent-media` carries video over the H.264 carrier with BWE, the gate and the SFU routing. `dissent-core` runs the camera and the screen share in the call loop, sizes their bitrate from the link's estimate minus the audio reservation, and shows previews, including a self tile. The desktop app and ui-next show them. AV1 is negotiated in calls when every peer supports it (behind the `svt-av1` feature; the desktop release does not enable it yet). The lobby shows a preview of your own camera. The share button lists the displays and windows that `xcap` reports, and falls back to the primary display.

### What the sources show

1. **str0m packetizes AV1 and H.264 itself.** `Writer::write` runs the codec packetizer for the payload type (`str0m/src/packet/h264.rs`, `av1.rs`). Those packetizers parse NAL units or OBUs, so an SFrame ciphertext handed to them would come out mangled. Opus is the only pass-through: `OpusPacketizer` only splits at the MTU. Audio frames are one Opus packet and never fragment, which is why audio works today.
2. **The call negotiates Opus only.** `new_rtc` (`dissent-media/src/call.rs:115`) calls `clear_codecs()` and `enable_opus(true, false)`. The only media line is audio (`call.rs:273`). `MediaAdded` handling covers audio only (`call.rs:654`).
3. **Keyframe requests are not handled.** `on_event` (`call.rs:652`) ignores `Event::KeyframeRequest`. str0m emits it when a receiver sends PLI or FIR.
4. **str0m's congestion estimate is not used, and not enabled.** str0m emits `Event::EgressBitrateEstimate(BweKind)` (`str0m/src/lib.rs:1014`), and `on_event` drops it. The audio target comes from the RTT and loss estimator in `dissent-media/src/bwe.rs`. str0m's own estimate fires only when `enable_bwe` is set in the config, which `new_rtc` does not do.
5. **The forwarder treats every datagram as audio.** `forward` (`sfu.rs:271`) takes the sender from the key ID and reads an audio level for `Link::Down`. A video chunk cannot pass through that code unchanged.
6. **The desktop camera path stops at a stub.** `toggleCamera` (`ui/actions.js:749`) calls `set_camera` (`apps/desktop/src-tauri/src/part6.rs:267`), which calls `Engine::set_camera` (`dissent-core/src/calls.rs:2269`). That returns "video is not built yet" when the camera turns on. The lobby and incoming views leave video out (`ui/views/lobby.js:4`, `ui/views/incoming.js:3`). No screen-share control exists in the UI.
7. **macOS has no camera usage string.** `tauri.conf.json` has no `NSCameraUsageDescription`, and macOS refuses camera access without one.

### Decision: option E, an H.264 carrier on the writer path

Each encrypted video frame is one NAL unit on the video m-line, escaped so no start code can appear inside it. str0m fragments it into FU-A packets and reassembles it on the receiver. Audio stays on the writer path, unchanged.

The options that were rejected:

| Option | What it does | Verdict |
| --- | --- | --- |
| A. RTP mode | `set_rtp_mode(true)` makes the whole `Rtc` RTP-only, and `Writer::write` panics (`str0m/src/lib.rs:1466`). The mode is per `Rtc`, not per media line (`session.rs:216`), so audio would move too. | Rejected. Audio was just tuned and works today, and RTP mode would change its packetization. |
| B. Opus pass-through for video | Write video chunks through the Opus packetizer. | Rejected. The SDP would name the wrong codec. |
| C. Fork str0m | Add a pass-through video packetizer. | Fallback only. It costs a fork. |
| D. SCTP unreliable channel | Send video over a data channel with no retransmits. | Rejected. It loses RTP feedback, the pacer and PLI. |
| E. H.264 carrier | Escaped SFrame ciphertext as one H.264 NAL unit on the writer path. | **Chosen.** |

**Why the SDP says H.264.** The SDP label is only the carrier. str0m needs a codec with a packetizer that accepts an arbitrary NAL, and H.264 is the one that does. The real codec, AV1 or H.264, is chosen in our own signalling, after the connection is up, and rides in the clear chunk header. Nothing outside dissent reads these bytes as H.264. Interoperability with real H.264 or AV1 endpoints is not a goal for calls.

**The guard against str0m changes.** A str0m upgrade that starts parsing NAL contents would break option E silently. The spike's carrier tests (`escape`/`unescape` round trip, FU-A reassembly of a 50 KB NAL, 5% loss) move into `dissent-media` as regression tests, so CI fails on such an upgrade.

### Built in phase 2

- **Carrier and gate** (`dissent-media/src/video.rs`): option E as described above, a per-sender gate that drops deltas after a gap and asks for a keyframe (throttled to 200 ms), and a replay window per sender. The video key is derived from the media key, so video and audio never share a (key, counter) pair.
- **Negotiation:** every offer carries a video line. `m=video 0` in the answer leaves the call audio-only (`has_video()` is false), with no error. `enable_h264` and `enable_bwe` are in `new_rtc`.
- **Forwarder:** an SFU relays video for active senders only. A downlink starts at a keyframe for each sender, and a downlink's keyframe request restarts its stream.
- **Core and desktop:** `set_camera(true)` starts a capture thread (640x360, 15 fps, 600 kbps, H.264), and the call loop sends its frames through the mesh or the forwarder. Received frames are decoded per sender and sent to the window as 320 px RGB previews, base64 encoded, in a `video_frame` event with the person's root id.
- **Linux:** camera and screen capture build with the same packages as the rest of the app, see the release workflow.
- **Bitrate follows the link:** each tick the call loop takes the smallest estimate across peers (str0m's BWE, which needs `enable_bwe`) and the largest audio target, and sizes each capture with `VideoAdapter`, so the audio reservation holds. A budget too small for video pauses it.
- **Previews:** each remote frame becomes a 320 px JPEG at quality 70, base64 in the `video_frame` event. About 116 KB/s per remote camera, measured on the test picture. Raw RGB base64 would be about 3.5 MB/s.
- **Datagram margin:** str0m's MTU is the RTP payload limit, so the transport's datagram limit is reduced by 128 bytes for the RTP header, extensions and SRTP tag. Without it, a full-size video packet exceeds the transport limit and the call drops the peer.
- **SVT-AV1 wrapper:** `svt-av1-sys` builds only with its `build` feature, which `dissent-video`'s `svt-av1` feature enables. The desktop release turns the feature on, and its CI installs NASM and libclang and checks out the submodule. SVT's own log is set to error-only (`SVT_LOG=2`) unless the caller already set `SVT_LOG`.

### Spike results

Measured in an in-memory harness on str0m 0.24.1, the same version as v0.1.4 and HEAD. Scratch crate outside the workspace.

- **Audio, writer path versus RTP mode.** Both deliver 50 of 50 Opus frames unchanged. RTP mode keeps 960-tick RTP timestamps and carries transport-cc sequence numbers. Neither mode emitted a bandwidth estimate in the audio-only run.
- **PLI through RTP-mode streams.** Works, and so does PLI on the writer path.
- **50 KB frames with 5% loss, RTP mode with NACK and RTX.** 3,744 of 3,750 packets were recovered. 57 of 60 frames were complete. The residual is why RTX is required and why PLI stays in the design.
- **H.264 carrier, writer path, 5% loss.** 60 of 60 frames match exactly after unescaping.
- **Bandwidth estimate.** `EgressBitrateEstimate` fired 17 times in 12 s of 1.36 Mbps video, with `enable_bwe(Some(1000 kbps))`. TWCC estimates rose from 1,000 to 1,555 kbps. With BWE off, zero events. TWCC was already negotiated on the video m-line by default. So `enable_bwe` goes in `new_rtc`, with an initial estimate.
- **Loss matrix, option E, 600 frames at 30 fps, 5 KB deltas and 25 KB keyframes every 30 frames.**

| Loss | With NACK and RTX: frames complete | Without RTX: frames complete | Without RTX: PLI sent | Without RTX: recovery p50 / p95 / max |
| --- | --- | --- | --- | --- |
| None | 600/600 | 600/600 | 0 | n/a |
| Random 1% | 600/600 | 248/600 | 39 | 0 / 0.92 / 1.08 s |
| Random 5% | 600/600 | 101/600 | 81 | 0.31 / 1.88 / 2.55 s |
| Random 10% | 600/600 | 15/600 | 98 | 4.07 / 5.04 / 5.04 s |
| Burst about 5% | 600/600 | 277/600 | 35 | 0.13 / 0.80 / 1.00 s |

Single runs with one seed each. Recovery was measured on the simulated clock.

**Conclusions from the spike:**
- **RTX on the carrier payload type is required.** Without it, PLI alone is too slow above 1% loss, because each recovery depends on one intact keyframe of about 21 packets.
- **`enable_bwe` goes in `new_rtc`, with an initial estimate.** Without it the estimate never fires.
- **str0m forces NACK on for every video payload type** (`add_config` sets `fb_nack` true). It cannot be turned off through the public API. With RTX absent, NACKs are sent but never answered.
- **Keyframes are capped at about 2× a delta frame.** Keyframes are the recovery bottleneck, so their size is limited. The encoder work must check the controls it has:
  - **SVT-AV1** (`svt-av1-sys`): `intra_refresh_type` (`KfRefresh` for closed-GOP; `FwdkfRefresh` is incompatible with CBR), `startup_qp_offset` (QP offset for the startup GOP only, not general keyframes), `max_qp_allowed`, and `intra_period_length`. A per-keyframe QP cap is not exposed, so the cap is enforced by `max_qp_allowed` on all frames or by a keyframe interval shorter than the current one.
  - **OpenH264** (`openh264` 0.9.9): `qp(QpRange)` applies to all frames, `intra_frame_period`, and `force_intra_frame`. There is no intra-refresh mode and no keyframe-specific QP. Keyframe size control needs a frame-size budget check and a re-encode with a higher QP, which is a later step.
  - Until the encoder work confirms otherwise, the keyframe cap is a target, not a guarantee.

### Wire format for option E

Each video frame is one NAL unit: a one-byte NAL header, then the escaped body. The body is:

- `frame_id` (u32): the sender's video frame counter.
- `flags` (1 byte): keyframe, plus the rest reserved.
- `codec` (1 byte): AV1 or H.264, the real codec, so the receiver knows what it is decoding. This is negotiated in our own signalling, not SDP.
- the SFrame ciphertext.

The SFrame header stays in the clear, so the forwarder still routes by key ID. Audio's wire format does not change. Since str0m fragments the NAL, there is no chunk index in our header. The forwarder can read the keyframe flag without decrypting, because it sits in the clear part of the body.

### Negotiation

- `new_rtc`: enable H.264 with `enable_h264(true)` and enable `enable_bwe(Some(initial estimate))`. Keep Opus. The payload type must have an RTX resend payload type, so `add_h264` or `add_config` is used for the video payload with `resend` set.
- Put the video m-line in the first offer with `Direction::SendRecv`, even when the camera is off. Turning the camera on then needs no renegotiation.
- **`m=video 0` means no video, not failure.** Spike 1 showed a str0m 0.24.1 answerer that lacks video answers `m=video 0`, and audio still completes (20 of 20 frames). The offerer must treat a rejected video m-line as "no video", and the call stays audio-only without an error. This must be tested in `dissent-media` (see the regression tests above).
- The real codec, AV1 or H.264, is chosen in our own signalling, from `codec::negotiate` (`dissent-video/src/codec.rs`). AV1 is preferred when both sides have it, H.264 otherwise.
- Receivers request keyframes with PLI or FIR, which map to `VideoEncoder::request_keyframe`. A resolution or fps change rebuilds the encoder, because both are fixed per encoder.
- Handle `Event::MediaAdded` for video as well as audio.

### Forwarder routing

- `forward` takes the frame kind from the raw inbox, so video and audio get separate queues in `sfu.rs` and `link.rs`.
- Video is relayed only for the top speakers already picked (`is_active`). Each downlink gets a whole keyframe or none of its frames.
- Forwarders never decode, so their CPU cost stays at packet routing.
- A listener that starts receiving a new speaker's video requests a keyframe from the forwarder, through the same request path as PLI.
- Keyframe recovery: a downlink that sees a gap in `frame_id` drops delta frames and sends PLI upstream on its parent link, through `Writer::request_keyframe` on that link. A listener's PLI to its forwarder uses the same path. The receiver retries PLI every 200 ms until a keyframe arrives, as the spike did.
- Mesh fan-out (`mesh.rs`) sends each encrypted frame once and relays it, with no re-encryption.

### Bandwidth and congestion

- **The audio reservation does not change.** The audio target still comes from `bwe.rs` through `on_target_bitrate`. The reservation stays `audio_target * 5 / 4 + 16` (`VideoAdapter::update`, `dissent-video/src/adapt.rs`).
- **Video budget.** Handle `Event::EgressBitrateEstimate` in `call.rs`. The available bandwidth is str0m's estimate, and `VideoAdapter::update(available, audio_target)` gives the video budget after the reservation.
- **CPU.** The governor's `apply_cpu` runs on the encoder timing, as in phase 1.

### UI hooks

- **Camera.** `toggleCamera` and `set_camera` already exist. Phase 2 replaces the stub at `calls.rs:2269` with a call into the media session, which opens `CameraSource`, runs the encoder and drives the send queue. The lobby preview (`ui/views/lobby.js`) and the incoming answer (`ui/views/incoming.js`) change from "off" to the real state.
- **Remote video.** Decoded frames reach the UI through a Tauri `Channel`, one frame per message, as RGBA, drawn on a canvas tile per speaker. Remote video needs a new engine event beside `CallUpdated`.
- **Share screen.** Part of the first cut, built for the primary display on Windows and macOS. Linux shares through the same picker path, with the portal on Wayland. A window picker is still to do.
- **macOS.** Add `NSCameraUsageDescription` under `bundle.macOS` in `tauri.conf.json` before the first camera build.

### Files to touch

- `crates/dissent-media`: `call.rs` (codecs, BWE, video mid, events, `m=video 0` handling); new `video.rs` (the option E carrier, keyframe gating, PLI); `frame.rs` (video header); `sfu.rs` and `link.rs` (video queues, `forward`); `mesh.rs` (fan-out); `e2ee.rs` (video frames with the same keys); `auth.rs` (`for_video`, already present). `bwe.rs` and `election.rs` are unchanged. Regression tests from the spike live under `tests/`.
- `crates/dissent-media/Cargo.toml`: depends on `dissent-video`. `dissent-video` is a leaf, so the dependency direction holds.
- `crates/dissent-core/src/calls.rs`: a real `set_camera`, camera state, video events.
- `apps/desktop/src-tauri/src/part6.rs`: `set_camera` wiring, the frame channel, and screen share.
- `apps/desktop/src-tauri/tauri.conf.json`: `NSCameraUsageDescription`.
- `apps/desktop/ui-next/src`: `lib/actions.ts`, `lib/api.ts`, the lobby and incoming call components, the call view for tiles, `index.css` and `shell.css`.
- Not touched: `dissent-audio`, and the audio reservation in `adapt.rs`.

### Test plan

- **Carrier regression (in `dissent-media`, CI):** `escape`/`unescape` round trip; a 50 KB NAL survives FU-A fragmentation and reassembly; 5% loss delivers every frame intact with RTX; a str0m upgrade that parses NAL contents fails here.
- **Unit, no network:** a gap drops deltas and asks for a keyframe; the keyframe flag is read from the clear part; SFrame video round trip across a key rotation; the `for_video` digest covers ten frames.
- **Negotiation:** an offer with video from a peer without video answers `m=video 0`, and the call completes audio-only with no error. An audio-only offer to a video-capable answerer still completes.
- **Integration, networked, `#[ignore]`, through `scripts/fwtest.sh` with `loopback_only`:** two peers get a first frame within 2 s, and the audio frame count is unchanged with video on. Loss runs repeat the spike's matrix. Three peers with the forwarder: only top speakers are forwarded.
- **BWE:** with `enable_bwe`, `EgressBitrateEstimate` fires under sustained video; the video budget falls below the audio reservation's floor and never takes audio's share.
- **Audio regression:** audio packet sizes and the reservation are checked before and after the change.
- **Platform:** CI uses `PatternSource` for the video path, with no camera. A real camera and screen share are checked by hand on Windows and macOS.
- **Benchmark:** 720p with three peers, forwarder CPU measured against the budget in "CPU cost".

### Open decisions for the lead

1. Keyframe cap: which encoder control to use, `max_qp_allowed` on all frames or a shorter keyframe interval for SVT, and whether OpenH264 needs a re-encode check. Needs an encoder spike.
2. Screen share is in the first cut, as asked. It is not built yet (see Status).

## Limits of the spike

- No Opus codec, audio devices, or video. Frames are synthetic.
- One synthetic candidate per side on iroh. Address-based ICE is therefore trivial, and no real address is ever learned.
- No trickle ICE. Candidates travel inside the offer and answer.
- No SFrame key rotation. The epoch is fixed at 0.
- Replay protection comes from SRTP only. SFrame's own validation hook is not wired in.
- Sender IDs are limited to 24 bits.
- Tests bind only to `127.0.0.1`, so Windows Firewall never prompts.

## Next steps

1. Choose the signalling transport for 1:1 calls, either the DM device connection or gossip.
2. Test calls across a real relay path. The loopback test uses a direct path only, so the relay fallback is still unproven.
3. Wire the MLS exporter from `dissent-mls` in place of the raw `MediaKey`, using the RFC label.
4. Add the Opus codec and an audio device path, then measure latency and loss on real networks.
5. Add epoch changes to the key ID and rekeying when membership changes.
6. A forwarder that is also a speaker, with its own participant link, and cascades over real links.

## Tier migration

A voice channel call starts as a mesh and moves between tiers as its live roster changes. The tiers are a mesh (up to six devices), a forwarded call (one elected forwarder relays between the others, from seven devices), and a cascade (a forwarded call above `cascade_at`, not built yet).

- **Live roster.** Each device keeps the devices that are in the call, by sender id. The roster grows on joins and answers, and shrinks on leaves and bans.
- **Hysteresis.** `TierPolicy::tier_for` moves a mesh up at seven devices and a forwarded call down at five, so a count that hovers at one edge does not flap. A device migrates only after its roster has wanted another tier for ten seconds.
- **Staged room.** A migration builds the room for the new tier beside the current one. The new room carries a generation id derived from the tier and the roster hash. Offers name that generation, so an acceptor matches an offer to the room it is for.
- **Link-ready swap.** The staged room links to every peer it should reach, and the call swaps to it once those links are up, or after twenty seconds if at least one link is up. It does not wait for a first frame: a staged room sends nothing until the swap, so two sides that each waited on the other's frames would deadlock. The old room carries the audio until the swap, and the audio state then moves to the new room.
- **Signed tier message.** The migrating device signs a tier announcement over the room, tier, forwarder and roster hash, and sends it to the others. A receiver applies it only if the signature is from a device in its own roster and the roster hash matches its own. A forged or unsigned message, or one that names a different roster, is ignored.

Migration is per device, so two devices can reach the same tier a little apart. The generation id and the tier message keep them in step.
