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

Host candidates ride inside the offer and answer, so the spike never sends trickled candidates. Real calls across NAT need server-reflexive or relayed candidates, which means a later step (see next steps).

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

The spike does not enable RTP header extensions that carry audio levels. If they are enabled later, a forwarder sees the level in clear text, so that choice belongs in the privacy review.

## How the mesh and peer-SFU tiers build on this

**Mesh (2 to about 6 video, about 15 audio).** Each peer makes one `Call`-style session per other peer. Each session uses the same MLS-derived key, and the key ID already gives each sender a distinct key. Sending N-1 copies of each frame is an application loop over `send_audio_frame`. Nothing in the frame format changes.

**Peer-SFU (up to about 50).** One peer forwards. Each participant opens a session with the forwarder. The forwarder relays encrypted frames from one session to others without re-encrypting them, and it never needs the media key beyond what MLS already gives members. Election, backup selection, and failover under about 2 s are separate work, and they need the forwarder to hand over its sessions.

**Cascaded forwarders (100+).** Forwarders relay to other forwarders. The same encrypted frames pass through each hop, so the design holds. Latency and bandwidth limits are the open questions from the spec.

## Limits of the spike

- No Opus codec, audio devices, or video. Frames are synthetic.
- Host candidates only. Calls across NAT need candidates from iroh or STUN, and a TURN-like relay path.
- No trickle ICE. Candidates travel inside the offer and answer.
- No SFrame key rotation. The epoch is fixed at 0.
- Replay protection comes from SRTP only. SFrame's own validation hook is not wired in.
- Sender IDs are limited to 24 bits.
- Tests bind only to `127.0.0.1`, so Windows Firewall never prompts.

## Next steps

1. Choose the signalling transport for 1:1 calls, either the DM device connection or gossip.
2. Add server-reflexive and relayed candidates, so calls work across NAT.
3. Wire the MLS exporter from `dissent-mls` in place of the raw `MediaKey`, using the RFC label.
4. Add the Opus codec and an audio device path, then measure latency and loss on real networks.
5. Add epoch changes to the key ID and rekeying when membership changes.
6. Build the mesh loop, then the peer-SFU election and failover.
