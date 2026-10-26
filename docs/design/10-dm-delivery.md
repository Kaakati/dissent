# 10. Offline delivery of direct messages through inboxes

Status: built in dissent-core (`inbox.rs`, `groups.rs`, `contacts.rs`). Deviations from this draft are listed under "Deviations". Builds on `02-communities-and-dms.md` (direct messages) and the MLS
conversations in `crates/dissent-core/src/groups.rs`.

## Problem

A direct message or a group DM reaches a person only while one of their devices is online and
reachable. Sender and recipient often are not online together. An inbox is an always-on device that
holds sealed messages until the recipient collects them. Inboxes are typically the user's own seeder
(see `apps/seeder`). An inbox can read only ciphertext and the recipient's root id.

## Inbox list

A user lists the devices that serve as their inboxes in a signed `InboxList`:

```rust
pub struct InboxList {
    pub root: RootId,
    pub devices: Vec<DeviceId>,   // at most 4
    pub seq: u64,                 // each new list has a larger seq; the highest wins
    pub issued_ms: u64,
    pub cert: DeviceCert,         // the device that signed it
    pub sig: Signature,           // domain b"dissent/inbox-list/v1"
}
```

The list travels inside every signed contact message the user sends: a request, an accept, and an
"inboxes" update sent to each contact when the list changes. It is not a field on `ContactCard`,
because a new field would change the postcard layout and break every existing `dissent://contact/`
link. Address announcements could carry it later.

## Deposit

When direct delivery fails for a device, the sender deposits the sealed packet at the inboxes of
that recipient:

```rust
Request::Deposit(Box<Envelope>)

pub struct Envelope {
    pub recipient: DeviceId,      // the device the packet is for; the inbox stores by device
    pub recipient_root: RootId,
    pub id: [u8; 32],             // blake3 of the packet, the same id a direct copy would have
    pub expires_ms: u64,          // now + 30 days at most
    pub packet: Vec<u8>,          // a postcard ConvPacket; sealed by MLS, opaque to the inbox
    pub from: DeviceCert,
    pub sig: Signature,           // the sender device signs the fields above
}
```

Inbox rules:

- An inbox accepts an envelope only for a root that lists it in its `InboxList`, and only when the
  inbox has seen that list.
- It caps storage at 50 MB per recipient root. A deposit that would exceed the cap is refused.
- It keeps an envelope for at most 30 days, and drops it at `expires_ms`.
- It rate-limits each sender device to 60 deposits an hour. A refusal tells the sender the cap was hit,
  so the sender's queue can mark the packet `failed`.
- It verifies the sender's cert and signature before storing anything.

## Collect and ack

A device collects its own envelopes:

```rust
Request::Collect { device: DeviceId, ts_ms: u64, sig: Signature }   // sig over the fields, by `device`
Response::Envelopes(Vec<Envelope>)
Request::Ack { device: DeviceId, ids: Vec<[u8; 32]>, sig: Signature }
```

`Collect` must be signed by the device it names, with `ts_ms` within five minutes of the inbox's
clock. Without that check, anyone could collect another person's mail by replaying an old request.
After the recipient processes an envelope, it sends `Ack` and the inbox deletes it. Unacked envelopes
stay until they expire.

## Key packages in inboxes

A first conversation needs an MLS key package from the other person before a group can be made. An
inbox holds up to five key packages per device, deposited by that device:

```rust
Request::DepositKeyPackages { device: DeviceId, packages: Vec<Vec<u8>> }   // signed by `device`
Request::TakeKeyPackage { root: RootId }    // returns one package and removes it
```

Each package is single-use. The sender tries the other person's devices first, then their inboxes. The
welcome that results is deposited at the inbox too, so the recipient joins when it next collects.

## Who creates the group

Whoever has something to say first creates the 1:1 group. The lower root does not decide it, because
that would stall a message whenever the lower root was offline. If both sides create at once, the
lower root's group wins. The other side drops its own group, and its unsent messages are sealed again
in the winning group.

## Deviations

- Key packages are not sent in the first message. The creator takes one from the other side's devices
  or inbox, which is the rule above.

## Delivery status

Each message has one status, shown as `DmItem.delivery`:

| Status | Meaning |
|---|---|
| `sending` | Written, not yet sent to any device or inbox. |
| `sent_direct` | A device of the recipient acknowledged the packet. |
| `queued_inbox` | Deposited at an inbox. The recipient has not acknowledged it yet. |
| `delivered` | The recipient device acknowledged the message, through a receipt. |
| `failed` | Refused, or expired, or a deposit was refused. The reason is kept. The sender may retry with `retry_message` for 30 days after the refusal, the envelope lifetime. |

"Delivered" means a receipt came back from the recipient device. An inbox ack does not count. A
receipt is a signed `Receipt { id, device, sig }` sent from the recipient to the sender.

The UI contract in `07-visual-system.md` Part 6.8 also wants a `queued` boolean and `pending` for
"sending". Both derive from `delivery`: `queued` is `delivery == "queued_inbox"`, and `pending` is
`delivery == "sending"`.

## Limits

| Limit | Value |
|---|---|
| Storage per recipient root at an inbox | 50 MB |
| Envelope lifetime | 30 days |
| Deposits per sender device per hour | 60 |
| Key packages per device at an inbox | 5 |
| Inbox devices per user | 4 |
| Collect timestamp window | 5 minutes |

## Test plan

Bob is offline, and his seeder engine is his inbox. Alice sends Bob a message, and then goes offline
too. Bob comes back, collects the envelope from the seeder, decrypts it, acks it, and the seeder
clears its copy. Alice's status moves from `queued_inbox` to `delivered` once Bob's receipt reaches
her. Covered by loopback tests with relays off. A sender who fills an inbox's hourly limit gets its next
message refused: it shows `failed` with the inbox's reason, and the others stay queued. A retry
(`retry_message`) seals the message's stored text again under the group's current epoch, so a group that
moved on still accepts it; a test checks that the retried packet is new ciphertext and that Bob reads it.
The retry queues the packet for the device it was refused for. A message whose text is not kept (a deleted
one, or a reaction or an edit) is sent as it was sealed. The test does not drive a group through an epoch
change, because a 1:1 conversation has no cheap action that makes one. After 30 days a refused message stays
`failed` with its reason, and has no Retry.

## Multi-device conversations

A direct conversation is an MLS group, and MLS adds devices, not people. Every device of each party must
be in the group, or that device never gets the conversation. This section covers the devices that come
and go after the group is made.

**Who changes the group.** Each party adds and removes only its own devices. A device of Bob's is added by
Bob's devices, and a device Bob revokes is removed by Bob's devices. Alice's devices do the same for Alice.
Nobody can add a device that its root has not certified, so the party a device belongs to is the only one
that can add it. A commit that adds or removes any other root's device is refused. Messages keep flowing
whichever party is offline. A device that is linked is added once its own party's devices run a pass, even
when the other party is offline.

**Concurrent commits.** Two commits at the same epoch would split the group, since an accepted commit cannot
be undone. So the two parties take turns by root. The party with the lower root commits at once. The party
with the higher root records the epoch on one pass, and commits on a later pass only if the epoch has not
moved since. If the other side committed in the meantime, the higher side sees a new epoch and waits again.
The window that is left is the time a commit takes to reach the other side. It is small, and a collision
inside it is the known limit of this rule. The test "both parties link a device at the same time" runs both
syncs together and checks that all four devices read the conversation.

**Adding.** Every few minutes, in the maintenance pass, each party's devices compare the conversation's devices
with the devices their own root has certified, minus any device revoked in a community. A device that is certified and not yet
in the group is added with a commit, and gets a welcome. A device is added only if it answers
`DmDevicesSupport(true)`, so a peer on an older version keeps its single device. The devices the group
was made with stay as they were. Our own other devices are added the same way, at the next pass after they
link, so a device that we link gets the conversation and sees the messages we send from the others. A
conversation made after a device linked includes that device from the start, as today.

**Removing.** A device that a revocation names is removed with a commit, which rotates the epoch. The
revoked device no longer receives messages sent after that. Messages it received before stay with it, as
they do for any member who leaves.

**History for a newly linked device.** A device added later gets no keys for the epochs before its add,
so it reads messages from after it was linked. The same rule as joiner history applies (`05`, "History
messages"): the default is `SinceJoin`, and a conversation has no setting to change it, so a linked device
starts from its link. Our own messages from the other devices are visible to it from then on.

**Delivery.** "Delivered" means a receipt from any device of the recipient, since receipts already name
the recipient's root. Each device of the recipient can acknowledge, and the first one moves the message to
`delivered`.

**Compatibility.** A peer with one device sees no change. A peer whose devices all predate this section
keeps its devices as they are, and gets no new ones.

**Commit rule.** A direct conversation accepts a commit only from one of its parties, and only when every device
it adds or removes is certified by the committer's own root. Before this, a direct conversation refused every
change. A welcome from one of our own devices is accepted whatever the DM policy says, since the policy is about
contacts, and our own root is never one.

**Security.** Each party changes only its own devices. A device added is one that the member's root
certified. A removed device is one a revocation named, and it is removed by a commit, so the removal rotates
the epoch. Nothing here lets an outsider join a group: every added device is certified by a member root,
and the adding party checks the support answer before the add. The one new exposure is that a linked device reads
the conversation's messages from its link on. That is what linking a device is for, and the user accepts it
when linking.

## Open questions

- Inbox trust. A user's inbox sees envelope sizes, timing and the recipient root. It does not see
  plaintext. The design accepts that leak for offline delivery.
