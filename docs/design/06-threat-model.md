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

1. **Open: unauthorized MLS commits are accepted.** `apply_commit` merges any valid commit from
   any group member. A malicious or compromised member can add an outsider's device. Honest
   devices would then encrypt to it. **Fix:** before merging, check the commit against the event
   DAG:
   - the committer must be the expected committer, an owner or admin per replay
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
4. **Open: SFrame replay.** Replayed media frames aren't explicitly rejected. **Fix:** a per-sender
   sliding replay window on the SFrame counter.
5. **Hardening: post-quantum key exchange.** Direct messages rely on the QUIC handshake alone.
   Prefer the hybrid X25519 + ML-KEM key exchange if iroh supports it, so recorded traffic can't
   be decrypted later by a quantum computer.
6. **Accepted, documented:** a peer forwarder in a large call sees who is speaking and how
   loudly, through the audio-level byte. Message metadata (author, time, channel) is visible to
   members, as in any group chat.
7. **Accepted for now:** the keystore, MLS state and decrypted content are plaintext on disk.
   This is tracked separately: protect them with the OS keychain or a passphrase.

## How the fixes are verified

Each fix lands with an adversarial test:
- A malicious member commits an outsider's device, and every honest peer rejects it.
- A welcome into a group with a non-member is refused.
- A peer without the invite secret can't sync or read control events.
- A replayed SFrame frame is dropped.
- An accepted rotation raises a warning.
