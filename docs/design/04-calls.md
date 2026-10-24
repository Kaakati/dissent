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

**Cascaded forwarders (100+)** are not built. Forwarders relaying to forwarders would reuse the same encrypted frames and the same election.

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

**Cost.** One digest is 243 bytes, and a speaker sends five a second. That is about 9.7 kbps, not the 2.5 kbps first estimated, because the estimate counted only the 64-byte signatures. Cutting the hash to 8 bytes would bring it to about 6.5 kbps. Batching more frames per digest would bring it lower, at the cost of more delay before a forgery is flagged.

**Tests.** The unit tests in `auth.rs` cover honest traffic, batch closing, a replayed frame before and after verification, out-of-order frames, too-old frames, a mismatch that mutes and still plays the real frame, a muted sender that plays only verified frames, a far-ahead frame, a lost digest, the 500 ms deadline, a forged digest signature, and plain links. The integration tests cover an honest authenticated mesh and an honest forwarded call, both with zero unauthenticated frames and zero flags. The forgery test runs a member under another member's sender id and a device key of its own. It is flagged 228 ms after the forged traffic starts, all 50 real frames play, and 10 forged frames played in the window before the flag.

**Limit of the cipher layer.** The cipher layer alone still lets any member send under another member's id. The `auth` layer is what detects it. A member with a device key can sign only its own digests, so it cannot forge another member's digest.

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
6. Cascaded forwarders for 100+ participants, and a forwarder that is also a speaker over a real link.
