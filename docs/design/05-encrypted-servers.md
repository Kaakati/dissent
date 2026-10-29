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
- **Content view** is local. Each device decrypts sealed text it can reach, and stores the plaintext in `content.sealed`, sealed under the vault. A message with no plaintext is `locked`.

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

## Leaving and the secret

Two things protect a community, and they are removed at different times.

- **Message text** is sealed with the MLS group. A departed device is removed by a removal commit, and every
  later epoch's keys exclude it. Messages from before its removal stay readable to it. The removal commit is
  issued by whichever member's turn comes first, and it does not wait for an admin. An admin is not needed for it.
- **The community secret** is held by every member, and it keys the gossip topic (`topic_salt`) and the RPC
  token (`community_token`). A departed member who keeps the secret can still compute both. Until a rotation, it
  can also receive control events on the topic.

What a departed member can do, after a leave:

- It cannot post. Replay refuses every event from a left member, except a rejoin (`Join`, `JoinVia`,
  `JoinAsStorage`) or another `Leave`.
- It cannot read message text sent after its removal commit. A stock client has also deleted its local data.
- It can still read sealed control events and reach the topic and token, until the secret rotates.

**Rotation on leave.** A rotation covers departed members as well as banned ones, using the same `SecretRotation`
payload. Clients up to v0.1.11 reject a rotation that covers a leaver, and would then be unable to read messages sent
after it, so issuing one is gated per community.

- *Phase 1 (v0.1.12):* replay accepts such a rotation. Issuing one was off.
- *Phase 2 (this release):* an admin issues a leave rotation only once every staying member device has said it
  accepts one. `rotation::ROTATE_ON_LEAVE` is now a kill switch, on.

**The probe.** Before a rotation that would cover a leaver, the admin's device sends `Request::RotationSupport` to
every device of every member who stays: storage members and its own root's other devices included, banned and
departed members and revoked devices left out. A new client answers `Response::RotationSupport(true)`. A client
from before this cannot decode the request and answers with an error. An error, no answer within 5 seconds, or no
counts as no, and the probe stops at the first one. A yes is kept in memory for ten minutes per device, so a
maintenance pass only asks the devices it has no fresh yes from. The hold is logged once when it starts and once
when it ends, not on every pass.

When a device says no, the rotation covers only the banned members, as in phase 1, and every client accepts it. The
departures stay owed: `rotation_due` keeps them, and each maintenance pass probes again. Once the last old device
updates, or leaves, the next pass rotates for every departure since the last rotation. The quick retries right after
a departure (`rotate_after_departure`) count a departure only while every staying device is known to say yes, so a
held rotation is left to the maintenance pass and the departure does not trigger ten probes.

**Why a probe, and not an advertised flag.** The candidates were a bit in an existing announcement (presence, call
`Features`, key packages), a new sealed-control event per device, or an RPC probe.

- A probe needs no change to anything signed, hashed or stored: one appended `Request` variant and one appended
  `Response` variant. Older peers already answer an unknown request with an error, which reads as no, so they cannot
  be cut off by a misread. `HistorySupport` and `MemberSealedSupport` work the same way.
- Presence beacons are signed and ephemeral, and only online devices send them; a field appended to one would need
  older decoders to tolerate trailing bytes in signed content. Call `Features` reach only devices in a call. Key
  packages are MLS objects, and a pool entry is used once.
- A sealed-control event would persist the claim, and so cover devices that are offline, but it would add an event
  per device to every DAG, a new carrier kind for replay, and a claim that stays true after a downgrade.

The cost of the probe is that an offline device blocks a leave rotation until it comes back, since its answer is
unknown. That is the safe side: the departed member keeps the secret a little longer, as before this release, and
no member loses access.

**Known gaps.** A device that joins between the probe and the rotation is not asked; a client up to v0.1.11 that
joins afterwards rejects the rotation in its replay. Clients from before this release that already accept leave
rotations (v0.1.12) do not answer the probe, and hold a rotation back until they update.

**Storage joins: a known gap.** `JoinAsStorage` carries no invite, and any holder of the secret can publish one under
a fresh root. A storage member is served the secret and the ciphertext, so a departed member could rejoin as
storage under a new root and receive both. This release refuses a storage join from any root that is a person here,
active or departed, and from the owner. It does not stop a fresh root. Planned fix:

- *Phase 1 (built):* new clients accept an admin-signed `StorageGrant { root, granted }`, appended and carried
  (`control::carry_sealed`), so older clients show it as locked. Owners and admins only; the latest per root wins,
  and `granted: false` withdraws it. After a storage join the seeder asks the owner's and admins' devices for one
  (`Request::StorageGrantAsk`, appended), again with a pause that doubles from 5 s to 10 min until it has one, and
  again on each start. A granter's device keeps the ask in memory, and Server Settings › Members lists each seeder
  as granted, asking or not granted, with Grant storage and Withdraw grant. The seeder's status says when it has no
  grant yet. Replay records grants (`CommunityState::storage_granted`) but a storage join does not need one yet
  (`storage_grants.rs`, tests in `relay_host_tests.rs` and `membership/storage_tests.rs`).
- *Phase 2:* a storage join needs a grant, once released seeders are old enough to have asked for one. The grant
  must then come before the join in replay order, or the join is refused, so a seeder joins, asks, and is refused
  until the grant lands; phase 2 has to settle whether a later grant re-admits it.

**Plain `Join` and `JoinVia`.** These have the same exposure. `Join` has no invite at all: replay accepts it from any
root that can publish under the secret, and the only check is a non-empty name. `JoinVia` names an invite id,
which is looked up in the state and must be unrevoked and unexpired, but the invite carries no signature that a
banned member could not reproduce; the secret is the only thing that authenticates it. So before a rotation, a
banned or departed member can come back as a new root through either. Rotation ends this: after a rotation and the
grace period, old-secret events are refused, so the window is the time from a ban or leave until the secret rotates.
Closing it for good needs the same grant idea as storage, applied to people, and is not in this release.

## Persistence

`openmls_sqlite_storage` is not in the local registry, so this uses OpenMLS's `MemoryStorage` and snapshots it. The storage's `values` map is sealed with XChaCha20-Poly1305 under a per-file subkey of the vault, then written to `mls.bin`. The decrypted content log, `content.sealed`, is sealed the same way, one record at a time under a subkey per community. A plaintext file from before sealing is migrated on first open.

Writes are split by kind of change:

- Group changes (commits, joins, removals, key package pool refills) are written at once and synced to disk. A lost commit would strand a device.
- Sending and receiving application messages are ratchet steps. The state is written at most once a second, or after 100 steps, and on maintenance and shutdown. A crash can lose the steps since the last write, so a sender reserves its generations first: each reservation covers 100 generations and is synced to `mls_reserve.bin` before the first send past it.
- Events are appended to the events log without a sync. The log is synced by a group flush at most every 250 ms, before any MLS state that depends on those events is written, and on shutdown. See the durability section of the threat model.

On restart the sender resumes at its last reservation. It creates messages up to that mark and discards them, so every generation it used before the crash is below the mark and none is used twice. A crash skips at most 100 generations, which is well inside the receivers' forward distance of 1000 (`RATCHET_FORWARD`); their out-of-order tolerance is 64 (`RATCHET_TOLERANCE`). Sends cost one synced write per 100 sends, not one per send.

The history seals and the event seals draw a random 192-bit nonce for each message and keep no counter, so a crash cannot make them reuse a nonce.

A crash can lose the receive-side ratchet steps taken since the last write. The plaintext those steps produced is already in `content.sealed`, so decrypted history stays readable. The lost steps only mean that a later restart may decrypt the same ciphertexts again, which is harmless because the text is already stored.

## History for new members

MLS does not let a new member decrypt messages from before its join epoch, and this is intended. Those messages show as `locked`. The desktop shows "Messages before you joined are encrypted". `MessageItem` gains `locked: bool`.

## Member-only control

Control events (membership, roles, bans, names, channels, relays, invites, settings) are sealed under the community secret until the owner turns member-only control on. Storage members hold that secret, so they can read them. With member-only control on, new control events are sealed under the MLS epoch instead, and a storage member cannot read them.

- **Epoch key.** Each device exports `export("dissent/control/v1", 32)` from its MLS group when it enters an epoch, and keeps the key sealed in `epoch-keys.bin`. Two keys come from it, under separate blake3 domains: `enc` opens the payload, and `write` checks a tag. Storage members never enter the group, so they never get `enc`.
- **Payload.** `Payload::MemberSealedControl { epoch, ciphertext, tag }`. The ciphertext is XChaCha20-Poly1305 under `enc`, with the header as associated data under its own domain. `tag` is a keyed hash of the header and ciphertext under `write`. Invite events stay under the community secret, since a joiner reads them before it has a group.
- **Removal.** A ban, leave or departure moves the epoch when the removal commit applies. A device removed from the group never receives the new epoch's key, so events made after that point stay closed to it. The window is the time before the removal commit lands. The commit waits for the removing device's turn, so it is short.
- **Joiners.** A joiner has no group before it joins, so its `Join` or `JoinVia` goes out with a blank name, under the community secret. Once it is in the group, it sends its name again as a member-sealed `Join`. The blank name is accepted only while member-only control is on, and is refused otherwise.
- **History keys.** After joining, a joiner asks the first three member devices for their epoch keys with `EpochHistory`, and keeps what they send. This is the RPC path, not the welcome. The openmls welcome cannot carry extra data, and an RPC reply travels over the same authenticated, post-quantum TLS as `SecretHistory`, which already sends older community secrets this way. A member answers only an active person, never a storage member.
- **History visibility.** `Payload::HistoryVisibility { setting }`, owner or admin only, defaults to `All`. Under `SinceJoin`, a member answers with the epochs from the one its device entered at, which it learns from the commit that added the device. A member that did not see that commit answers with none. `since_invite` is not implemented. It would need the epoch of each invite, which the DAG does not record.
- **Write keys for storage.** A storage member asks any member device for `WriteKeys`, which answers only storage members, and keeps the write keys. A tag that fails is dropped, and an event whose write key it lacks waits until the key arrives. The tag keeps junk out of seeders that lack the write key, which means outsiders and DHT nodes. A storage member holds the write key, so it can make a tag that checks for any ciphertext it likes. It can push junk that other seeders keep. Members still refuse that junk, since it does not open under `enc`.
- **The switch.** `Payload::MemberOnlyControl { on }`, owner only, latest HLC wins. The owner may turn it on only when every other member device answers `MemberSealedSupport` with `true`. An offline device counts as not supporting it. The owner's status reports how many devices are waiting. The UI shows "Some members need to update" until every device answers.
- **Before the switch.** Events made before the switch keep their community-secret form, and storage members can read them. A new community starts with the switch off. So the owner's own first `Join`, and the first channels, stay readable to storage members until the owner turns the switch on. Turning it on for new communities by default is a product decision that is not made here.
- **Enforcement.** Replay does not check the form of a control event against the switch, so a member that breaks the rule can still send a community-secret event, which storage members read. Honest devices never do. The rule is in the author path, and tests cover it.
- **Locked history.** A device without an epoch key sees its events as `Rejection::Locked`. The view sets `history_locked`, and the UI shows "Waiting for a member to unlock history".

## Compatibility rule for new control payloads

Every client decodes a whole batch of events and a whole events log, so one payload kind an old client does
not know fails the batch, stalls that peer's sync, and makes the community skip loading on restart. So:

- **New control payloads go through the carrier.** The payload is `SealedControl { generation: CARRIER_GENERATION
  (u64::MAX), ciphertext }`, where the ciphertext is a postcard `Carried { kind, body }`. An older client decodes the
  outer event, finds no key for that generation, and shows the event as locked. The carrier's kind is what a newer
  client reads. The member-sealed kind (`kind = 1`) is the first one. A private channel's access events use kind `4`
  (`CARRIED_SEALED`): the body is an ordinary community-secret seal under the current generation, so the carrier hides
  it from an older client and the seal hides it from anyone without that generation's secret, a banned member included
  after a rotation.
- **The carrier is part of the event.** Its bytes are inside the event body, so the event id and signature cover
  them. A relay or peer cannot change the kind without breaking the signature.
- **Old forms keep decoding.** `Payload::MemberSealedControl` is what v0.1.6 wrote. Replay still opens it, and the
  tag check still accepts it. Nothing new writes it.
- **Tolerant reading everywhere.** Sync asks with `FetchRaw`, which answers each event as its own bytes, and skips one
  that does not decode, keeping the rest of the page. A peer that predates `FetchRaw` gets `Fetch`. The events log is
  read with `read_records_skipping`, which keeps an undecodable record on disk for a later version.
- **Settings the owner can change are gated.** `set_history_visibility` is refused while a member device cannot
  answer the support probe, as the member-only switch is.

Residual: a client that predates this rule still stalls on a payload kind added without the carrier, which is why
new kinds must use it.

## Limits

- Commits can only be applied in epoch order. A device that misses a commit fetches the missing ones through `MlsCommits` on its next sync.
- A moderator cannot commit. If no owner or admin is online, joins and removals wait.
- Unban does not re-add the device. The member must request `MlsJoin` again.
- A new device of an existing member must also send `MlsJoin`.
- Gossip carries only the commit announcement, not the commit bytes (see "Who commits"), and its message limit is 128 KiB (`DEFAULT_GOSSIP_MAX_MESSAGE` in `dissent-net`).

## History messages (v0.1.7)

A joiner reads the text of messages from before its join when the server's history setting allows it
(`All`, the default; `SinceJoin` keeps them locked). MLS alone cannot give that, because a device holds
no keys for epochs before it joined. So text gets its own key per epoch.

**Key.** At each epoch a device records `history = export_secret(group, "dissent/history/v1", 32)`, next to
its control keys, in `history_keys.bin` (sealed with the vault, one file per community). Every device in
the epoch derives the same key.

**Form.** A new sealed kind, `SealedKind::HistoryMessage`, carries the text as
`XChaCha20-Poly1305(history key of the event's epoch, plaintext)`, with a random nonce and the associated
data `community id ‖ epoch ‖ "dissent/history-msg/v1"`. The manifest wrapper of attachments is unchanged.
Only plain messages use this kind. Replies, edits, reactions and their rules stay as they are.

The MLS ciphertext is not sent alongside it. Keeping both would not protect the text: anyone who can read
the history copy reads the message, so the MLS copy adds no secrecy against that reader, and it doubles the
size. The cost is real: per-message forward secrecy within an epoch is replaced by per-epoch secrecy. A device
that is later compromised exposes every text of each epoch it was in, not only the ones still in its ratchet.
The threat model records this.

**Push and retries.** When an active member admits a joiner, it sends the joiner the keys the setting allows
(`PushHistoryKeys`, the same rule as `HistoryKeys`), a few times if needed. The joiner accepts them from an active
member only. A joiner that has no keys also fetches them: its first retry is after one second, then the wait doubles
up to a minute, and a member coming online or a new key cuts a wait short. An empty answer does not end the fetch.
**Serving.** `HistoryKeys { community }` returns `(epoch, key)` pairs. The rule is the same as `EpochHistory`:
only an active person, never a storage member; `All` gives every epoch; `SinceJoin` gives epochs from the
requester's join, and none when the responder did not see that join. A joiner asks the first three member
devices and keeps what they send.

**Default.** A server created from this version starts with `SinceJoin`: the creator emits
`HistoryVisibility(SinceJoin)` right after its first channel, as a sealed control event. Absent means `All`, as
before, so a server created earlier keeps the setting its replay gives. Owners can switch any server to `All` in
Server Settings. `All` says what it costs: a new member can read past messages, and those messages are protected
per encryption period rather than per message.

**Compatibility.** A device sends `HistoryMessage` only when every other member device answers
`HistorySupport(true)`. The answer is a flag, kept per community with the member device list it covers. The send
path only reads that flag and never waits on a member. A community with no other member device is ready at once.
Otherwise a missing, stale or differently-scoped answer counts as no: the message is sent in today's form, and a
background check starts. A "yes" is kept for five minutes and a "no" for one minute, and a changed member list
starts a new check at once. A presence beacon from a member of a community that is not ready also starts a check, debounced by half a second, so a device that comes back online is counted at once.

**Restarts and joins.** Each community's last answer is saved in `history_ready.bin`, with the member devices it
covers, so a restart starts from that answer and not from "unknown". When an active member admits a joiner, it
checks the member devices again before the welcome goes out, so the joiner's first message from the admitting
member is in the new form. The check covers every member device, not only the joiner's.

**The join carries the joiner's support.** Right after its join, a joiner publishes `Payload::HistorySupport
{ supported: true }` and pushes it with the join, and a founder does the same after its own join. It is a
control event, carried (`carry_sealed`) and sealed like the others, so an older client shows it as locked. It
speaks only for the device that signed it: replay keeps the latest per device (`CommunityState::declares_history`).
Any member device may send it, a storage member included, on the control channel. Bans, departures and
revocations apply as to any event. A device that does not support history messages sends none.

A member that did not admit the joiner uses it two ways:

- **Widening a yes.** A fresh yes still holds when the member devices change, as long as every device the answer
  did not cover has declared support. The answer is widened to the new list and keeps its age. A departure only
  shrinks the list. So the joiner's first message from that member is in the new form, with no wait.
- **Silence.** In a check, a device's own answer decides. A device that does not answer, or answers with an error,
  counts as supporting history messages only if it declared support. An explicit no wins over an older
  declaration, so a device that went back to an older version is counted out at the next check.

**Declaring once.** A device declares when its replay of a community does not already hold its current setting:
no declaration while support is on, or a last declaration that differs. It checks at startup, after every batch
of events it stores (which is how a newly linked device declares when it first syncs a community), and when its
setting changes. Only an active, unbanned member declares. Its own declaration is replayed at once, and two
triggers at the same moment send one event, so an unchanged setting sends nothing more. A member that joined
before this version declares at its first start after the upgrade. A device without support that never declared
sends nothing, since no declaration already counts as no. A device that does not answer and never declared
counts as unsupported. Old messages stay MLS-only, and a
joiner cannot read them. That cannot be fixed retroactively. A device on an older version that joins after the
switch cannot read `HistoryMessage` text either. It shows as locked, and nothing crashes.

**Re-opening.** When history keys arrive, stored events that are still locked are run through the unseal pass
again, and each one that opens emits a message event, so the view refreshes without a restart.

**Labels.** A locked message sent before the device's join says "Sent before you joined". Any other locked
message says "This message is waiting for a key". The tooltip of the first names the server's history setting.

**What does not change.** Member-only control, the epoch ring for control events, bans and rotations. A ban
or a removal still rotates the epoch, so the banned device gets no keys for later epochs. Storage members
get no history keys, since they are not in the group.

**Seeders.** A seeder never gets history keys or epoch keys, and never serves them. Storage members are
outside the group, so joiners read history only from active members. A seeder's part is ciphertext and the
pictures it can open with the community secret. See `08-files.md`, "What a seeder serves".
