# Design 05: encrypted community messages

Status: first version. Scope is text channels in communities. DMs keep their own path.

## What is encrypted

Message text is sealed. Permission state and the public structure of the DAG stay in clear.

| Payload | Clear | Sealed |
|---|---|---|
| `Message`, `Reply`, `Edit` | ids, author, channel, hlc, parents, target, kind, epoch | text |
| `Delete`, `React`, `Unreact` | everything | nothing (reaction emoji stays clear) |
| Control events (`Join`, `Leave`, `Ban`, roles, handles, rotations, devices, channels) | everything | nothing |

Reactions keep the emoji in clear. Reaction state is keyed by emoji and is part of `CommunityState`. Sealing it would make state depend on which keys a peer holds.

A sealed event is `Payload::Sealed { epoch, kind, ciphertext }`, where `kind` is `Message`, `Reply { target }` or `Edit { target }`. The target is in clear, so permission checks (`NotAuthorOfTarget`, `UnknownTarget`, live-message rules) run on plaintext metadata.

## Two views of a community

- **Permission state** (`CommunityState` in `dissent-events`) is identical on every peer. A sealed message is recorded with its id, author, hlc, reply target and a `sealed` flag. Its text is empty in state.
- **Content view** is local. Each device decrypts sealed text it can reach, and stores the plaintext in `content.log`. A message with no plaintext is `locked`.

Replay never reads ciphertext, so every peer computes the same state regardless of keys.

## Who commits

MLS commits are not DAG events. They travel as `GossipMsg::Commit` on the community topic, and as the `MlsCommits` RPC for catch-up.

- Rank: owner first, then admins by join time, then root id. Only owner and admins commit.
- Preferred committer: the highest-ranked owner or admin that is online in the local view (self, or a gossip neighbour's device).
- Only the preferred committer adds members (`MlsJoin` requests) and removes them (bans, leaves, departures).
- Collision: a commit for an epoch already applied is ignored, so the first one applied wins. The lower-ranked committer's change is re-issued by the winner's reconcile. This is weaker than the full rule: committers merge at once, so a higher-ranked committer cannot override a commit already applied. Full tie-break needs deferred merge and is future work.

## Joining

1. The joiner gets the community's events, authors its `Join`, and pushes the `Join` to the invite peers (as before).
2. It sends `MlsJoin { community, from }` to each invite peer. A peer that is not the preferred committer answers with an error, and the joiner tries the next peer.
3. The committer checks the joiner's membership in the DAG, then fetches one key package from the joiner's device with `KeyPackages`.
4. The committer adds the device (`add_members`), broadcasts the commit, appends it to `mls_commits.log`, and returns the welcome in the reply.
5. The joiner joins from the welcome.

## Key packages

Each device keeps a pool of 5 single-use key packages. `KeyPackages` returns up to the requested count and consumes them, and the pool is topped up when it runs low. Private key material lives in MLS storage, so the pool survives restarts through the snapshot.

## Ciphersuite

Groups use `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`. It pairs a hybrid X25519 and ML-KEM-768 (X-Wing) key exchange with AES-256-GCM, SHA-384 and Ed25519 signatures, so the key exchange resists a future quantum computer while device keys stay Ed25519. The ciphersuite comes from the IETF draft `draft-ietf-mls-pq-ciphersuites`, not a published RFC. The wire format may change before the RFC is published, which would break interop with peers on another draft revision. Key packages, welcomes and commits from any other ciphersuite are refused, and so are snapshots taken under another suite.

Sizes with this suite, measured in `dissent-mls` tests: a key package is about 2.8 KB, up from about 0.4 KB under the classical suite. A two-member welcome is about 5.9 KB, and an add commit is about 5.8 KB. Welcomes and commits grow with the ratchet tree, roughly 1.2 KB of public key for each member and each parent node. They fit core's 4 MiB page and 16 MiB frame caps. Gossip's default message size is 4096 bytes, so commits sent as `GossipMsg::Commit` need a larger limit on the gossip builder.

## Bans and departures

A ban, leave or departure removes every device of that member. The preferred committer runs this check when control events change and on each maintenance pass. The removal commit is delivered to everyone, including the removed device, which then drops the group. Messages sent after that epoch are locked for it.

## Persistence

`openmls_sqlite_storage` is not in the local registry, so this uses OpenMLS's `MemoryStorage` and snapshots it. The storage's `values` map is written to `mls.bin` after each MLS mutation, including encryption, since sender ratchets advance on every message. Plaintext history lives in `content.log`, so losing MLS state keeps everything already decrypted. Snapshots hold key material in plain text. This matches the keystore, and it needs hardening before 1.0.

## History for new members

MLS does not let a new member decrypt messages from before its join epoch, and this is intended. Those messages show as `locked`. The desktop shows "Messages before you joined are encrypted". `MessageItem` gains `locked: bool`.

## Limits

- Commits can only be applied in epoch order. A device that misses a commit fetches the missing ones through `MlsCommits` on its next sync.
- A moderator cannot commit. If no owner or admin is online, joins and removals wait.
- Unban does not re-add the device. The member must request `MlsJoin` again.
- A new device of an existing member must also send `MlsJoin`.
- Commits over about 4 KB exceed iroh-gossip's default message size. Gossip must be built with a larger `max_message_size` before commits reach peers reliably.
