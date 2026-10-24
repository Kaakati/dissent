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

MLS commits are not DAG events. Each applied commit is announced on the community topic as a `GossipMsg::Commit` carrying a `SignedCommit`. The announcement holds the epoch, a blake3 hash of the commit, and the committer's device signature. Receivers fetch the bytes with the `MlsCommit` RPC from the committer, or from the neighbour that relayed the announcement, and check the hash before applying. Gossip never carries the commit bytes, so a large post-quantum commit cannot exceed the gossip frame limit. Catch-up uses the `MlsCommits` RPC.

- Any member may commit, but a commit takes effect only if the DAG allows it. Every peer checks this before merging, and refused commits are logged at warn level with the committer.
- An add must bring in a device of a current, non-banned member.
- A removal must take out a device of a banned or departed member, or one of the author's own devices.
- The author must be a current member. A welcome must list only current members.
- Rank sets only how long a device waits before acting: owner, then admins, then moderators, then members by join time. A device waits 3 s for each higher-ranked member that is online, up to 15 s, and then acts unless the change is already done.
- Collision: a commit for an epoch already applied is ignored, so the first one applied wins. Two members that commit at once can fork the group. The delay makes this rare, but full resolution needs deferred merge and is future work.

## Joining

1. The joiner gets the community's events, authors its `Join`, and pushes the `Join` to the invite peers (as before).
2. It sends `MlsJoin { community, from }` to each invite peer. A peer that is not a current member answers with an error, and the joiner tries the next peer. Otherwise the peer waits its turn, adds the joiner, and announces the commit.
3. The committer checks the joiner's membership in the DAG, then fetches one key package from the joiner's device with `KeyPackages`.
4. The committer adds the device (`add_members`), broadcasts the commit, appends it to `mls_commits.log`, and returns the welcome in the reply.
5. The joiner joins from the welcome.

## Key packages

Each device keeps a pool of 5 single-use key packages. `KeyPackages` returns up to the requested count and consumes them, and the pool is topped up when it runs low. Private key material lives in MLS storage, so the pool survives restarts through the snapshot.

## Ciphersuite

Groups use `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`. It pairs a hybrid X25519 and ML-KEM-768 (X-Wing) key exchange with AES-256-GCM, SHA-384 and Ed25519 signatures, so the key exchange resists a future quantum computer while device keys stay Ed25519. The ciphersuite comes from the IETF draft `draft-ietf-mls-pq-ciphersuites`, not a published RFC. The wire format may change before the RFC is published, which would break interop with peers on another draft revision. Key packages, welcomes and commits from any other ciphersuite are refused, and so are snapshots taken under another suite.

Sizes with this suite, measured in `dissent-mls` tests: a key package is about 2.8 KB, up from about 0.4 KB under the classical suite. A two-member welcome is about 5.9 KB, and an add commit is about 5.8 KB. Welcomes and commits grow with the ratchet tree, roughly 1.2 KB of public key for each member and each parent node. They fit core's 4 MiB page and 16 MiB frame caps. Gossip's default message size is 4096 bytes, so commits sent as `GossipMsg::Commit` need a larger limit on the gossip builder.

## Bans and departures

A ban, leave or departure removes every device of that member. Any member runs this check, after its turn to act when control events change and on each maintenance pass. The removal commit is delivered to everyone, including the removed device, which then drops the group. Messages sent after that epoch are locked for it.

## Persistence

`openmls_sqlite_storage` is not in the local registry, so this uses OpenMLS's `MemoryStorage` and snapshots it. The storage's `values` map is written to `mls.bin`. Snapshots hold key material in plain text. This matches the keystore, and it needs hardening before 1.0.

Writes are split by kind of change:

- Group changes (commits, joins, removals, key package pool refills) and our own sends are written at once and synced to disk. A lost commit would strand a device. A lost sender ratchet could make us reuse a key for the next message, which breaks AES-GCM for both messages, so sends cannot be debounced.
- Decrypting a received message only marks the state dirty. The state is written at most once a second, or after 100 steps, and on maintenance and shutdown.

Because sends are written at once, a crash never loses a sender ratchet step. So no key generation can be reused after a restart, and no self-update is needed to recover. Sends cost one synced write each, about 3.8 ms, which is invisible to people.

A crash can lose the receive-side ratchet steps taken since the last write. The plaintext those steps produced is already in `content.log`, so decrypted history stays readable. The lost steps only mean that a later restart may decrypt the same ciphertexts again, which is harmless because the text is already stored.

## History for new members

MLS does not let a new member decrypt messages from before its join epoch, and this is intended. Those messages show as `locked`. The desktop shows "Messages before you joined are encrypted". `MessageItem` gains `locked: bool`.

## Limits

- Commits can only be applied in epoch order. A device that misses a commit fetches the missing ones through `MlsCommits` on its next sync.
- A moderator cannot commit. If no owner or admin is online, joins and removals wait.
- Unban does not re-add the device. The member must request `MlsJoin` again.
- A new device of an existing member must also send `MlsJoin`.
- Commits over about 4 KB exceed iroh-gossip's default message size. Gossip must be built with a larger `max_message_size` before commits reach peers reliably.
