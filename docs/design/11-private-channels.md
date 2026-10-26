# Design 07: private channels

Status: design, not yet implemented. A private channel is visible only to chosen members and roles, and its messages are readable only by them. It builds on `05-encrypted-servers.md` (member-only control, epoch keys) and `06-threat-model.md` (finding 2).

## What a private channel is

A channel whose owner or admin has set an access list. Its messages are sealed under a per-channel key, so a community member who is not on the list reads none of them, even though every community member is in the MLS group. Owners and admins always have access, so they can manage it.

## Permission events

Appended to `Payload`, in the same way as the member-only variants:

- `ChannelAccess { channel, private: bool, roles: Vec<Role>, members: Vec<RootId> }`. Owner and admin only. The latest accepted HLC per channel wins. `private: false` returns the channel to public.

Readers of a private channel, as replay computes them: the owner, every admin, every member holding a listed role, and every listed member who has not left or been banned. Replay checks each post and each access change against this state, so every peer agrees without any key.

## Message content

Messages in a private channel are `PrivateSealed { kind, generation, ciphertext }`. `kind` is the existing `SealedKind`, so targets stay in clear for permission checks. The text is XChaCha20-Poly1305 under the channel key of `generation`, with the header as associated data. Replay never opens it.

A private channel's key is random, and each generation has its own key. Keys are distributed the way epoch history is: an allowed member device holds the keys it was given, and answers `ChannelKeys { community, channel }` for requesters that the replay of its own DAG shows as allowed, and only those.

## Revocation

Every accepted `ChannelAccess` that removes a reader starts a new generation. The generation counter is part of the replayed state, so every peer agrees on it. The author of the change creates the new key and offers it to the members who remain. Messages made from then on use the new generation. Anyone who was a reader before keeps the keys they held, so old messages stay readable to them. This is the same rule as the epoch model in doc 05.

Residual: a revoked member still holds the old key, and can read messages made before the revocation. A responder whose DAG has not yet seen the revocation may still hand the new key to the revoked member. The window is the time for the change to reach that responder.

## Newcomers and history

A newcomer added to a private channel gets keys by the community's history setting (doc 05):

- `All`: every generation the requester is allowed to read.
- `SinceJoin`: only the current generation, and nothing older. Messages from older generations stay locked to them.

Joining the community does not give any private channel's keys. A member gets a channel's keys only after a `ChannelAccess` lists them.

## Compatibility

Today an older client that meets an unknown payload fails to decode the whole batch. The sync from that peer stalls, and a restart skips the community. The new variants above have the same problem. The fix is under discussion (encode new payloads inside `SealedControl`, so old clients see them as locked). Until that is decided, private channels cannot ship.

## What is hidden, and from whom

- Message text: sealed to the channel key, hidden from everyone off the list.
- The channel's name and existence: the `ChannelCreate` event is a control event. Under member-only control, storage members do not read it, but every community member does. In a community without member-only control, the name is readable by everyone in the DAG. The UI hides a private channel from people off the list, but that is a display rule, not secrecy. This is documented, and the design does not claim more.
- Access events: the list of roles and members is replayed by everyone who reads control events.

## Implementation order

1. Events: `ChannelAccess`, `PrivateSealed`, and the generation counter in state. Replay tests for who may read, who may post, and the generation rule.
2. Core: keys per channel and generation, sealing and opening, `ChannelKeys` RPC, offering keys on revocation, history by setting.
3. Networked tests: an allowed member reads; a non-member cannot see or read; revoking cuts off new messages; a newcomer added later reads by the history setting.
4. UI (ui-next only): the "Private Channel" toggle in the create-channel modal, a member and role picker in channel settings, a lock icon in the sidebar.
