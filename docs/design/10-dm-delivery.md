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
- Rate limits are not covered by a test yet.
- A message refused by policy shows as `sent_direct` or `queued_inbox` until the sender drops it.
  Refusals are not yet reported as `failed` to the sender from an inbox.

## Delivery status

Each message has one status, shown as `DmItem.delivery`:

| Status | Meaning |
|---|---|
| `sending` | Written, not yet sent to any device or inbox. |
| `sent_direct` | A device of the recipient acknowledged the packet. |
| `queued_inbox` | Deposited at an inbox. The recipient has not acknowledged it yet. |
| `delivered` | The recipient device acknowledged the message, through a receipt. |
| `failed` | Refused, or expired, or a deposit was refused. The sender may retry. |

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
her. Covered by a loopback test with relays off. A second test checks that a deposit from a sender
over the rate limit is refused.

## Open questions

- Multi-device membership. A conversation holds one MLS device per member today, so a second device
  of the same user does not yet get the group. An inbox does not fix that, because MLS needs each
  device to join separately.
- Inbox trust. A user's inbox sees envelope sizes, timing and the recipient root. It does not see
  plaintext. The design accepts that leak for offline delivery.
