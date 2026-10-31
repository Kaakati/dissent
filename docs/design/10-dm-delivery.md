# 10. Offline delivery of direct messages through inboxes

Status: built in dissent-core (`inbox.rs`, `groups.rs`, `contacts.rs`). Deviations from this draft are listed under "Deviations". Builds on `02-communities-and-dms.md` (direct messages) and the MLS
conversations in `crates/dissent-core/src/groups.rs`.

## Problem

A direct message or a group DM reaches a person only while one of their devices is online and
reachable. Sender and recipient often are not online together. An inbox is an always-on device that
holds sealed messages until the recipient collects them. It can be:

- another device of the user's own;
- a seeder the user runs (`apps/seeder --inbox-for <address>`);
- a friend's always-on computer that agreed to hold the user's mail.

An inbox cannot read the messages. What it does learn is in "What an inbox learns".

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

- An inbox accepts an envelope only for a root it serves: its own root while its inbox switch is on,
  and each root its owner chose to host (see "Choosing an inbox").
- It caps storage at 50 MB per recipient root, and at 200 MB for everyone it serves together. A deposit
  that would exceed either cap is refused.
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

## Choosing an inbox

Built in `inbox.rs` and Settings > Offline delivery (`ui-next/src/components/settings/inbox-page.tsx`).

**Consent comes from the host.** A device holds mail only for the people its owner chose:

- `set_serve_inbox(true)` holds mail for the device's own root, for the other devices of the same person.
- `host_inbox_for(root, true)` holds mail for another person, for at most `MAX_HOSTED` (16) people.
  - In the app it is a tick per contact under "Hold messages for others".
  - A seeder takes `--inbox-for <address>`, which can be repeated, and applies exactly that list on every
    start.
  - Turning a person off drops what is held for them.
  - The list is kept in `inbox_hosts.bin`, apart from `inbox.bin`, so older files still load.

Listing a device as an inbox does not make it one. A device that does not serve the person refuses
their deposits and key packages.

**Refusals look the same.** The refusal does not show whether the device is an inbox for anyone:
- A deposit or key packages for a person the device does not serve get `NOT_SERVED` ("this device does
  not hold mail for that person"), on a device that is no inbox as on an inbox for other people.
- A collect gets an empty list.
- A key-package take gets "no key package for that user", also when the person's packages ran out.
- The other reasons (invalid, expired, rate limited, full) come only after the person is known to be served.

**Notices.** A change to a device's hosting list sends the person a signed notice, `ContactMsg::InboxHost {
from_card, on, ts }`.
- It is appended to `ContactMsg` and travels through the contact outbox, retried until the person's device
  answers. A device from before this cannot decode it and refuses it, and the notice then leaves the outbox.
  Setting what is already set sends nothing, so a seeder that applies its flags on each start notifies
  only on a real change.
- The receiver keeps the newest notice per device, at most 20, in `inbox_notices.bin`.
- The app shows an in-app notice (`EngineEvent::InboxHostChanged`). Settings > Offline delivery lists the
  notices, with "Use as inbox" for a device that started and is not yet chosen.
- A notice reaches the person only when the host knows their devices: from a shared server, a contact, or
  the key packages they deposited there.

**Invisible hosts.** A device that holds mail for a person keeps answering that person's inbox requests
while invisible (see `06-threat-model.md`, "Invisible mode"), so their mail is not held up by the host's
status.

**Probe.** `Request::InboxProbe(Probe { cert, ts_ms, sig })` is appended to the requests.
- **The request.** The asking device signs `(root, device, ts_ms)` under `dissent/inbox-probe/v1`. The
  timestamp must be within five minutes.
- **The answer.** `Response::InboxProbe(ProbeAnswer { serving, envelopes, key_packages, bytes })` counts
  what is held for the cert's root.
- **Strangers.** A device that does not serve that root, or a probe that does not verify, gets the default
  answer: not serving, nothing held. So the answer tells a stranger nothing, not even whether the device
  hosts anyone else.

**Picking.** Settings asks for the devices that offer (`inbox_candidates`):
- The candidates are our own other devices, our contacts' devices and the devices of our communities'
  storage members (seeders), up to 32.
- Each is probed at the same time, with a 5 s limit, and only those that answer `serving` are shown.
- A device can also be added by its id.
- `set_inboxes` saves the list (at most 4), announces it to every contact in the contact messages, and
  deposits key packages with each inbox.

**Status.** `inbox_status` probes each chosen inbox. Settings shows one line per inbox, which says one of:
- what it holds for us: messages waiting, bytes, and key packages left for new conversations;
- that it does not hold our mail, because its owner has not agreed or has stopped;
- that it is not answering.

**Reaching a friend's inbox.** The sender dials the inbox by device id.
- A sender who shares a server with the inbox, or is its contact, gets its address through peer exchange.
- Otherwise the dial relies on the node's address lookups: the device's DHT and number0 records. So a
  friend's computer that is in no group with the sender is reached only when those lookups are on.

## What an inbox learns

An inbox holds MLS ciphertext and cannot read it. Its host does learn, for each person it serves:

- **Who the person is,** and the ids of the devices that collect. Each collect and probe is signed by the
  collecting device, and a probe carries its cert.
- **Who writes to them.** Each envelope carries the sender's `DeviceCert`, so the host learns the sending
  device and its root, the size of each packet and when it arrived. The host can build the person's
  correspondence graph for the time they are offline.
- **When the person is offline and when they come back,** from the deposits and the collects. Collects run
  on every maintenance pass.
- **Each packet's clear fields.** The packet is a `ConvPacket` in postcard, and only the MLS parts are
  sealed:
  - its kind (welcome, commit, message, receipt);
  - its conversation id, which groups the packets of one conversation;
  - for a welcome, the conversation's name, its creator and every member's root.

  So an inbox learns who is in each group DM that someone adds the person to while they are away.

It does not learn the text of messages, or the person's contacts.

It can drop or delay mail, which shows as `queued_inbox` that never becomes `delivered`, and it can
refuse. It cannot forge mail, because every packet is signed and sealed by its sender.

A friend's computer is therefore a trust choice: the friend learns whom the person talks to while they
are away. Settings says so next to the picker ("What an inbox learns"). Looking for inboxes probes every
candidate, so each of them learns that the person looked.

## Deviations

- Key packages are not sent in the first message. The creator takes one from the other side's devices
  or inbox, which is the rule above.
- There is no signed `InboxList`. The list is an unsigned `Vec<DeviceId>` inside the signed contact
  messages (request, accept, and the `Inboxes` update). It is as authentic as the contact message that
  carries it. There is no separate `seq`: the newest contact message wins.

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
| Storage at an inbox for everyone it serves | 200 MB |
| People one device hosts an inbox for, besides itself | 16 |
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

- Inbox trust. An inbox sees the sender of each envelope, its size and timing, and the recipient root
  (see "What an inbox learns"). It does not see plaintext. The design accepts that leak for offline
  delivery. A sealed-sender envelope, which would hide the sending device from the inbox, is not built.
