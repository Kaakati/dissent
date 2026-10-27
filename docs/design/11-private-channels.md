# Design 07: private channels

Status: the two-step form is in v0.1.10. The one-event creation form and files in private channels are in the next checkpoint, after it. A private channel is visible only to chosen members and roles, and its messages are readable only by them. It builds on `05-encrypted-servers.md` (member-only control, epoch keys) and `06-threat-model.md` (finding 2).

## What a private channel is

A channel whose owner or admin has set an access list. Its messages are sealed under a per-channel key, so a community member who is not on the list reads none of them, even though every community member is in the MLS group. Owners and admins always have access, so they can manage it.

## Permission events

Appended to `Payload`, in the same way as the member-only variants:

- `ChannelAccess { channel, private: bool, roles: Vec<Role>, members: Vec<RootId> }`. Owner and admin only. The latest accepted HLC per channel wins. `private: false` returns the channel to public.
- `ChannelCreatePrivate { name, roles, members }`. Creates a channel and its access in one event. Owner and admin only. See "Creating a private channel in one event".

Readers of a private channel, as replay computes them: the owner, every admin, every member holding a listed role, and every listed member who has not left or been banned. Replay checks each post and each access change against this state, so every peer agrees without any key.

## Message content

Messages in a private channel are `PrivateSealed { kind, generation, ciphertext }`. `kind` is the existing `SealedKind`, so targets stay in clear for permission checks. The text is XChaCha20-Poly1305 under the channel key of `generation`, with the header as associated data. Replay never opens it.

A private channel's key is random, and each generation has its own key. Keys are distributed the way epoch history is: an allowed member device holds the keys it was given, and answers `ChannelKeys { community, channel }` for requesters that the replay of its own DAG shows as allowed, and only those.

A device that holds a message but not its key asks its readers for it. The request is retried with a backoff of 1, 2, 4 and so on up to 60 s, and a member coming online or a change of access wakes it early. The retry stops when the device has the key, or when it is no longer a reader. A reader is never left locked until restart.

## Files

A file in a private channel is sent the way a text is: the file key, name, MIME type and thumbnails are inside the text, and the text is sealed under the channel key. Nothing about a file is in clear.

The cleartext manifest that public channels write (`DFM1`: each file's ciphertext hash, size and seed flag) is not written for a private channel. The reasons:

- The hash and size of a file are visible to everyone in the DAG who reads the event, including storage members who cannot read the channel. The manifest would tell them a file exists, how big it is, and what its blob is called.
- Seeders mirror a file only from a manifest, and they hold no key for a private channel. So no seeder mirrors a private file, whatever its mirror policy says. A seeder never learns the hash.
- The thumbnail hints the sender and the readers send to seeders (`MirrorHint`) are not sent for a private channel, for the same reason.

The readers get the file from the sender, or from another reader who has it, through the usual blob transfer. That transfer is addressed by hash, which only readers hold.

Trade-off: a private file has no seeder, so it is available only while the sender or a reader who has it is online.

## Revocation

Every accepted `ChannelAccess` that removes a reader starts a new generation. The generation counter is part of the replayed state, so every peer agrees on it. The author of the change creates the new key and offers it to the members who remain. Messages made from then on use the new generation. Anyone who was a reader before keeps the keys they held, so old messages stay readable to them. This is the same rule as the epoch model in doc 05.

Residual: a revoked member still holds the old key, and can read messages made before the revocation. A responder whose DAG has not yet seen the revocation may still hand the new key to the revoked member. The window is the time for the change to reach that responder.

## Newcomers and history

A newcomer added to a private channel gets keys by the community's history setting (doc 05):

- `All`: every generation the requester is allowed to read.
- `SinceJoin`: only the current generation, and nothing older. Messages from older generations stay locked to them. Adding a reader under `SinceJoin` starts a new generation, so the newcomer gets only that one. Messages from before the join stay in the old generation.

Joining the community does not give any private channel's keys. A member gets a channel's keys only after a `ChannelAccess` or `ChannelCreatePrivate` lists them.

## Creating a private channel in one event

Creating a channel and setting its access used to take two events: `ChannelCreate`, then `ChannelAccess`. Between them, the channel was public on the wire, and a peer could post into it before the access event arrived. The engine refused local posts in that window, but it could not stop a remote one.

The fix is one event, `ChannelCreatePrivate { name, roles, members }`, which makes the channel and its access together. Its id is the id of the event, as for `ChannelCreate`. Replay applies it as a `ChannelCreate` followed by a `ChannelAccess` with the same HLC, so no peer ever sees the channel without its access.

Wire form. The event is a control payload, so it is carried, following the rule in doc 05:

- Without member-only control: `SealedControl { generation: CARRIER_GENERATION, ciphertext }`, where the ciphertext is a postcard `Carried { kind: CARRIED_SEALED, body }`, and `body` is the community-secret seal of `ChannelCreatePrivate` under the current generation: an ordinary `SealedControl { generation: g, .. }`, the same seal as other control. Only a holder of generation `g`'s secret can open it, so a banned member after a rotation cannot read the name or the members.
- With member-only control: the member-sealed carrier (`CARRIED_MEMBER`), so storage members cannot read the name or the members. The member-sealed inner is the same payload.

An older client decodes the outer event, has no key for `CARRIER_GENERATION`, and shows the event as locked. It is not a new plain kind, so it cannot fail a batch. A newer client opens the inner seal with its ring, and accepts the payload only when it is a `ChannelCreatePrivate` or a `ChannelAccess`. The carrier refuses to carry a carrier, and refuses any other payload.

Old form. `ChannelCreate` and `ChannelAccess` events from before this change still replay as they did. A channel made the old way is readable as it was, and its access can still be changed by `ChannelAccess`.

Failure. The event either is accepted in full or is not stored, so there is nothing to roll back. The author then creates the channel's key, as the two-step form did. Until the key exists, posts to the channel wait for it, as any key request does.

## Compatibility

The rule for new control payloads (doc 05): a new kind goes through `SealedControl` with `CARRIER_GENERATION`, so an older client shows it as locked and not as a batch failure. The carried kinds are:

- `1` (`CARRIED_MEMBER`): member-sealed control under member-only, in doc 05.
- `3` (`CARRIED_PRIVATE`): a private channel's message text, sealed under the channel key. The clear fields are the kind and generation.
- `4` (`CARRIED_SEALED`): the community-secret seal of a private channel's access event, `ChannelCreatePrivate` or `ChannelAccess`. The carrier hides it from older clients; the seal hides it from anyone without the current generation's secret.

`ChannelAccess` written before this carrier was a plain sealable payload, sealed under the community secret. Replay still reads that form. A device writes the carried form when member-only control is off, and the member-sealed form (`CARRIED_MEMBER`) when it is on. Under member-only control, storage members cannot read either event.

## What is hidden, and from whom

- Message text: sealed to the channel key, hidden from everyone off the list.
- Files in a private channel: sealed with the text. No cleartext manifest, no seeder mirror, and no mirror hint. See "Files".
- The channel's name and existence: the channel is created by `ChannelCreatePrivate`, a control event. It is sealed under the current community-secret generation, as every other control event is. Under member-only control, storage members do not read it, but every community member does. Without member-only control, the name is readable by holders of the current generation, which includes storage members, and by nobody else. A banned member after a rotation is not one of them. The UI hides a private channel from people off the list, but that is a display rule, not secrecy. This is documented, and the design does not claim more.
- Access events: the list of roles and members is sealed in `ChannelCreatePrivate` and `ChannelAccess` alike, under the same seal as other control. It is replayed by holders of the current generation's secret, and by no one else.
- The reader list: `channel_access` returns it only to readers and to owners and admins.

## Implementation order

1. Events: `ChannelAccess`, `PrivateSealed`, and the generation counter in state. Replay tests for who may read, who may post, and the generation rule.
2. Core: keys per channel and generation, sealing and opening, `ChannelKeys` RPC, offering keys on revocation, history by setting.
3. Networked tests: an allowed member reads; a non-member cannot see or read; revoking cuts off new messages; a newcomer added later reads by the history setting.
4. UI (ui-next only): the "Private Channel" toggle in the create-channel modal, a member and role picker in channel settings, a lock icon in the sidebar.
5. One-event creation: `ChannelCreatePrivate`, its carrier, replay, and the engine call that the UI uses.
6. Files in private channels, with the decisions in "Files".
