# Design 06: threat model and end-to-end encryption review

Review date: 2026-10-24. Scope: whether a relay, a gossip hop, a DHT node or a man in the
middle can read or alter Dissent traffic. Findings marked **open** are being fixed.

## Who we defend against

- **Network attackers and relays.** They see and can modify every packet between devices.
- **Gossip hops.** Other peers that forward a community's messages.
- **DHT nodes.** Strangers that store and serve Dissent records on the Mainline DHT.
- **Malicious members.** People legitimately in a server who try to read or change more than
  their role allows.
- **Out of scope here:** someone with access to your unlocked computer, and a compromised root key.

## What holds today

| Property | Mechanism |
| --- | --- |
| Transport confidentiality and peer authentication | Every connection is iroh QUIC with TLS 1.3, authenticated by the device's Ed25519 key. Dialing a `DeviceId` fails unless the far end holds that key, so relays only forward ciphertext and cannot impersonate a device. |
| Event and address integrity | Every event, signed address, device certificate, genesis and rotation is Ed25519-signed with a domain-separated context, and verified on receipt. |
| Device to identity binding | A device's MLS credential is its root-signed `DeviceCert`, and its MLS signature key is the device key itself. Both are checked on every add, welcome and commit. |
| Message text confidentiality | Channel message text, replies and edits are sealed with MLS (RFC 9420, X25519, AES-128-GCM, Ed25519). There is no plaintext fallback: sending fails if the group isn't ready. |
| Media confidentiality | Call frames are SFrame-encrypted (RFC 9605) with keys from the MLS exporter. Forwarders hold no key. |
| Direct messages | Sent only device to device over the authenticated QUIC connection, never stored by third parties. Signed by the sender and bound to the recipient. |

## Findings

1. **Fixed: unauthorized MLS commits are refused.** `apply_commit` merges any valid commit from
   any group member. A malicious or compromised member can add an outsider's device. Honest
   devices would then encrypt to it. **Fix:** before merging, check the commit against the event
   DAG:
   - the author must be a current member, and the add, removal and welcome rules in `docs/design/05-encrypted-servers.md` must hold
   - every added device must belong to a current, non-banned member
   - removals must target only banned or departed members, or the committer's own stale devices

   Joiners apply the same check to a welcome, so they're never welcomed into a group containing
   non-members.
2. **Open: community metadata is readable without an invite.** Anyone who knows a community id
   can join its gossip topic or sync it, and read control events: display names, handles, roles,
   bans, membership timing and reaction emoji. DHT nodes learn the community id and the plaintext
   name in the community record. **Fix:**
   - Derive the gossip topic and an access key from a per-community secret carried in invites.
   - Require proof of that secret, or known membership, for sync RPCs.
   - Encrypt control payloads and the record's name under the access key.
   - Seal reaction emoji under MLS.
3. **Open: no key-change warning.** When a contact's root rotates via recovery, nothing tells the
   user to re-verify the safety number. **Fix:** an engine event and a UI notice on accepted
   rotations, and on new devices for people you've DMed.
4. **Fixed: SFrame replay.** Each receiver keeps a 1024-frame sliding window per sender and
   epoch. The window is checked before decryption, and it advances only after a frame
   authenticates. Replays and frames older than the window are dropped.
4b. **Fixed, with residual risk: impersonation inside a call.** Every call member holds the same
   SFrame group key (RFC 9605 §7.2). Each sender signs a digest of its frames with its device key:
   every 25 audio frames (about 500 ms) or 10 video frames, with 8-byte frame hashes, which costs
   about 4.5 kbps per speaker. Receivers play at once and verify against the sender's DeviceCert.
   A forger is flagged about 0.5 s after starting, and its sender id's frames are then held until
   verified, so the real speaker keeps playing. Residual risk: up to 25 forged audio frames,
   roughly 500 ms, can play before detection. Every tier is authenticated: 1:1, mesh, forwarder
   and cascade. 4-byte hashes would save about 1.6 kbps but allow a 2^32 second-preimage search,
   which a GPU can do in seconds, so they are rejected.
5. **Fixed: post-quantum key exchange.** Every connection prefers the hybrid X25519 + ML-KEM-768
   TLS key exchange, falling back to X25519 only for peers that don't support it. Traffic
   recorded today, including direct messages, resists later decryption by a quantum computer.
   Server messages move to the hybrid MLS suite `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`,
   which uses AES-256-GCM. That is an IETF draft suite, and its wire format may change before the
   RFC.
6. **Accepted, documented:** a peer forwarder in a large call sees who is speaking and how
   loudly, through the audio-level byte. Message metadata (author, time, channel) is visible to
   members, as in any group chat.
7. **Partly fixed: data at rest.** The identity file is sealed with XChaCha20-Poly1305 under a
   data key held in the OS credential store: Windows Credential Manager, macOS Keychain or Linux
   Secret Service. Argon2id passphrase derivation is the fallback for headless machines. **Open:**
   the MLS state and the decrypted content log are still plaintext, and core will adopt the same
   `Vault`. Malware running as the user is out of scope.

## How the fixes are verified

Each fix lands with an adversarial test:
- A malicious member commits an outsider's device, and every honest peer rejects it.
- A welcome into a group with a non-member is refused.
- A peer without the invite secret can't sync or read control events.
- A replayed SFrame frame is dropped.
- An accepted rotation raises a warning.
