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
