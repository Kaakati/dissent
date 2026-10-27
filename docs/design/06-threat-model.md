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
| Links from outside the app | A `dissent://` link (from a browser, chat or file) only chooses a confirm dialog. Joining a server, adding a contact and approving a device need the person's press in that dialog, and a link never does them on its own. Links over 4 KB and with control characters are dropped. A device link fills the code box and approves nothing. |
| Community relays | Set by owners and admins in a signed control event. Carried in invites and the DHT record, and dialled only over https with publicly trusted certificates. At most 4 per community and 30 per device, shared with the relays the device adds. |
| Presence privacy | Online status comes only from signed beacons, sent every 30 s to community members and direct contacts, never from traffic. Invisible sends none, but a peer may still show the last beacon for up to 60 s after you go invisible. Status is per device, not per account. |
| Address lookup | A user record is signed by the root key that is the address, and every device cert in it is verified against that root. A forged record, a cert from another root, or a packet signed by anyone else is refused. |

## Address discovery: what publishing reveals

Adding someone by address needs their user record on the Mainline DHT, stored under their root key
(`dissent-discovery`, `user_record.rs`). Unlike community records, this record is **public**: anyone
who holds the address can resolve it. Publishing it reveals:

- **That the address uses Dissent.** Any DHT node that stores or serves the record, and anyone who
  watches lookups for that key, learns that the key holds a Dissent record.
- **The device ids** of every device the root has certified, each with its issue time, and one dial
  address for the root device. The dial address is the public form the communities use: no direct IPs.

It does not reveal a display name, contacts, communities, messages or the device's network location
beyond that public form. The root key is the address, so every device in the list is linked to one
person by anyone who resolves the record.

Controls:

- **The switch.** "Let people add me by address" is on by default. Off publishes an empty record that
  replaces the old one on the DHT, so the address stops resolving within minutes. Contact links keep
  working either way.
- **Forgery.** Each device cert is checked against the address, and the packet must be signed by it.
  A stranger who copies the layout cannot publish under someone else's address.
- **Replay.** A DHT node can keep serving an older, validly signed record until a newer one replaces
  it. So a device that was revoked can still appear for up to an hour, the republish interval, and
  the DHT's own expiry. Revocation for messages is unaffected: it is still enforced by the signed
  device-revocation event.
- **Linked-only setups.** Only a device that holds the root key publishes. The root device lists the
  certs it issued, so a linked device needs no request to be included. If no device holds the root
  key, nothing is published and adding by address is unavailable; people add by contact link.

## Peer exchange: what sharing addresses reveals, and to whom

Peer exchange moves signed address records between devices, so a reconnect usually needs no DHT
lookup. A record holds the device id, its relay URL, its direct IP addresses and port, a sequence
number, an expiry of 24 hours, and the device cert that ties the device to its root. The device key
signs it. Anyone who relays a record cannot change it, so a relay or seeder can withhold a record
but cannot forge an address.

Who receives what:

| Exchange | Sent to | What it reveals |
|---|---|---|
| Own record, pushed when our address changes and re-signed every 10 minutes | Devices of members of our communities, and of our contacts | Our device id and direct IP addresses, to people who share a group with us. Never to the DHT. |
| Digest, sent when we connect to a gossip neighbour | That neighbour, if in scope | Device ids and sequence numbers of the records we may share with it. |
| Records of a third device | Only requesters who share a community with that device | Another member's device id and addresses, to its co-members. A contact who shares no community with a device never gets its address from us. |
| Live lookup for device X | Connected neighbours that are in scope and share a community with X | The asker's device id and X's id, to people who already share a group with X. Anyone else is not asked. |
| Lookup answer | The asker only, and only if the record is visible to it | The record. Other askers get an empty answer, so the reply does not show whether X exists. A device outside our groups is refused. |

Two rules hold throughout. A device learns a record only if its root is in one of its groups, so a
stranger's record never enters the cache. A device answers a lookup only for a requester in its
groups. Both checks use the signed cert in the record, not the sender's word.

Limits, against abuse and against a cache that grows without bound:

- The cache holds at most 4096 records. The least recently used go first.
- At most 4 devices per root. A fifth evicts that root's least recently used device.
- A peer may push once per 10 seconds. Refused pushes do not restart the wait.
- A record that fails verification, is expired, lives longer than 24 hours, or is not newer than the
  one held is dropped.

What the design does not change: direct IP addresses already reach community members and contacts
through the existing signed-address announcements, which predate peer exchange. Peer exchange keeps
that audience. It adds the rule that a third device's record goes only to its co-members.

### Local network discovery (opt-in, off by default)

With "Find people on the same network" on, a device sends a multicast beacon on UDP port 47320.
The beacon carries a fresh random nonce and one token per community key. A token is a keyed hash of
the ten-minute window and the nonce, under a key only that community's members hold.

- **What an observer on the network sees.** A beacon's values are random, and no two beacons repeat
  them, so an observer cannot link beacons from one device, track it across windows, or tell which
  community a beacon belongs to. The observer does see that a Dissent device sends beacons on that
  port, and that device's IP address.
- **Device ids stay hidden until a handshake.** A device answers a beacon only with a challenge,
  proving it holds the community key. The beaconing device then reveals its id only to a peer that
  proved the key in return. Each side answers a fresh nonce from the other, so a recorded message
  cannot be replayed to a new peer to reveal an id. A stranger without the key learns nothing.
- **What a member on the network learns.** Other members of the same community on the LAN learn each
  other's device ids and IP addresses. They are already members, so this matches what they learn
  over the network. A device that has left a community stops being recognised once the key rotates.
- **Off in loopback test mode.** The node never binds the multicast socket when loopback is set.
- **Known limit.** LAN addresses stay in the lookup until the engine restarts. There is no expiry for
  them yet.

### Seeders

A seeder is a member that keeps the records of its communities' members and answers lookups from
members. It learns what any member learns: device ids and addresses of the members it holds, and
which device each member asks about. A seeder cannot forge a record, because devices sign their own,
and a stale record is bounded by its expiry. A seeder can withhold records, which only costs
availability; the dial then falls back to the DHT or another member. Answers from a seeder are
labelled `seeder` in the dial stats, so the user can see when one was used.

Relays are not lookup points in this version. A relay forwards QUIC packets it cannot read, and it
holds no records. A relay that also acted as a lookup point would need the same signed-record
design, and it is not built.


## Invite links and previews: what they reveal

A short invite, `dissent://invite/<dss1 address>.<secret>.<generation>[.<invite id>]`, carries the
community's address and its invite secret, and nothing else. The DHT record that names the peers and
relays is sealed under that secret, so someone with only the address finds no record and no peers.
Whoever holds the link can join, so the link is a bearer credential, like the long form. A short
link resolves while its secret is current or inside the 30-day grace after a rotation: during that
grace the founder keeps the record under each retired secret too.

The preview (`Request::InvitePreview`) is answered only to a device that proves the invite secret,
with the token every community request carries. The answer shows, and only to that holder:

- the community name, its creation date, and a reference to its icon
- the member count and the online count, as numbers only
- whether the named invite is live, revoked, expired, or unknown to the answering member

It shows no member name, device id, channel, or message. The answer carries the community's own
genesis, so the name and date check against the community key, and it is signed by the answering
device, whose cert is root-signed. Each requester gets ten previews a minute from one device, and a
requester reuses a preview for 60 seconds.

Who learns what:

- **Invite holders:** the name, icon, counts and invite status above.
- **Strangers who know only the community id:** nothing. The request has no valid token, so it is
  refused.
- **A banned device:** refused, as for every community request.
- **DHT nodes:** the sealed record only. They learn that a key exists, as for community records.
- **The answering member:** learns the requester's device id and address, as it does when joining.

The online count comes from presence beacons. It is a count, not a list, is shown to invite holders
only, and can be stale by up to the presence window.

A server's **name, banner, description and traits** (Server Settings) reveal no more than the
genesis name and icon already do, and they reach the same people. They are sealed control events, so
DHT nodes and outsiders see none of them. Members see all of them. Storage members see the rename,
which stays under the community secret. They do not see a banner or profile sealed under the epoch key
while member-only control is on. Invite holders see all of them on the card. The owner vouches for
the name through the signed rename, which the joiner opens with the invite secret. A member cannot put
a false name on a card: without a valid rename, the card shows the genesis name. The answering member
can still serve an older rename, and it can **misstate the banner, description and traits** in a
preview, as it can the icon and counts, since only its own signature covers them. Once joined, the
DAG decides. The founder republishes the DHT record with the new name, sealed under the invite secret
as before. An uploaded banner, like an icon, is fetched from members' devices, so those devices learn
that someone fetched it. A preset banner is a number and fetches nothing.

A **profile banner** reaches the same people as the profile picture: members of the servers the person
is in, sealed under each community's control key, and their contacts, directly. DHT nodes and
outsiders see neither the banner nor that it changed. A picture banner is fetched from the person's
devices, so they learn who fetched it, as for an avatar. Only the person's own devices can set it,
since replay keys it by author and the direct form is device-signed.

## Image thumbnails: what a preview reveals

A sender may attach a thumbnail to an image: a JPEG of at most 320 pixels on its long side, under
64 KB, with EXIF and all other metadata dropped. It is stored and encrypted as its own blob, under its
own key, and the message carries its key and hash inside the same sealed text as the file. Beyond
the file itself, a thumbnail adds one more blob and its size to what a holder of the message can
see, and the size tells roughly how detailed the picture is. A recipient fetches it on arrival,
from the sender's devices, so the sender's devices learn that the recipient opened the message, as
they already do for a file that is fetched. Seeders hold the ciphertext of the main file only, and
do not mirror thumbnails. The sealed text, not the blob, is what the keys protect.

## Mentions: what a mention reveals

A mention puts the mentioned person's id into the sealed payload of the message, so only the members who
can read the message learn who was mentioned. The id is a root key, which every member already knows
from the member list, so a mention reveals nothing a member does not already see. What a mention
changes is who gets an alert: a forged `@everyone` from an ordinary member is ignored, because the
reader checks the author's permission when the message was accepted. A mentioned person's device also
learns the message was addressed to them, as it does for a reply. Marking a channel read is kept on the
device that marked it and is never sent to anyone.

## Mutual friends: what a contact list reveals

A person may share their contact list, so that a contact sees which of their own contacts they have in
common with that person. The list is answered only over the direct connection (`Request::Friends`), and
only to a device whose root is an accepted, unblocked contact of the person asked. Everyone else gets
nothing, and so does an older version, which does not know the request. The viewer intersects the list
with their own contacts on their own device, so the mutual list is never computed on anyone else's.

What the list contains: root ids of contacts, and nothing else. No display names, notes, nicknames or
communities. Names and pictures are looked up on the viewer's device from what the viewer already knows.

Who learns what:

- **Contacts of the sharing person:** the whole contact list, not only the overlap with the viewer. This
  is the cost of the feature, and why it is a setting. "Show my friends list to my friends" is on by
  default. Off sends the list to nobody, and profiles show no mutual friends line from that device.
- **The viewer, who is also a contact:** the whole list arrives in the answer, because the answer is
  the list, not the overlap. The app shows only the overlap, and does not show or store the rest.
- **Strangers, server members who are not contacts, and blocked people:** nothing. The answer has no
  list in it, so it does not show whether they are a contact.
- **The sharing person:** learns that a contact opened their profile, because the request reaches them.
  It is sent only between contacts.

Limits and controls:

- A requester may ask six times a minute per device. The viewer keeps an answer for two minutes, and
  a "shares nothing" answer for thirty seconds, so turning the setting on or off shows soon.
- A change to a contact list, or to the setting, is seen by viewers within those two minutes. It is not
  pushed.

Why private set intersection was not used. It would let each side learn only the overlap. We skipped it
because:

- The viewer is a contact of both people, so the overlap is already something they may know. What PSI
  would hide is the rest of the list, and the setting is the decision about whether that is shared at all.
- PSI needs both devices online and a round of protocol for every profile view, which costs time and
  bandwidth on a path users open often, for a gain the setting already covers.
- A contact who shares their list reveals it to every contact anyway. PSI would only stop a viewer from
  seeing the rest of a list the sharer chose to show them.

## History messages: what a history key exposes, and to whom

A history key is the key of one epoch for message text (`docs/design/05-encrypted-servers.md`, "History
messages"). Whoever holds an epoch's key reads every plain message sent in that epoch.

- **Members of that epoch** already held it, in effect, through MLS. Nothing changes for them.
- **A joiner** gets the keys of the epochs its server's history setting allows: all of them under `All`,
  or those from its own join under `SinceJoin`, which is the default for new servers. It asks the first three member devices, and
  only an active person is answered. The owner's setting decides this, as it decides control events.
- **Storage members** are outside the group, so they hold no history keys and are never served one.
- **Banned and removed devices** keep the keys of the epochs they were in, as before. A ban rotates the
  epoch, and the banned device is not served the keys of later epochs. Nothing new reaches it.
- **The member who answers** learns which joiner asked, as it does for `EpochHistory`.

What this weakens. Forward secrecy is per epoch for these messages, not per message. The MLS ratchet gave
per-message secrecy, and a device that was later compromised exposed only the keys still in its ratchet.
Now a device that held an epoch's key exposes every text of that epoch. New servers default to
`SinceJoin`, so joiners get none of this. An owner who chooses `All` accepts the cost. `SinceJoin` limits
what joiners receive. It does not restore per-message secrecy for members.

Old messages keep their MLS form and stay locked for joiners. Nothing can open them retroactively.

Member-only control, the epoch keys of control events, and the rule that bans and removals rotate the epoch
are unchanged.

## Durability: what a power cut can drop

Events in a community's events log are written without a sync. A group flush syncs the logs at most every 250 ms. It also runs before any MLS state that depends on those events is written, and on shutdown. A crash of the app loses nothing, because the bytes are already with the operating system. A power cut can lose the events written in the last 250 ms. Peers hold the same events, and a device asks for what it lacks when it next syncs with them, so an event is lost for good only if every copy is lost in the same window.

A record cut short by a power cut is dropped when the log loads, and the records before it still load. The next append lands after the cut, so the framing stays aligned.

Sender generations of the MLS ratchet are durable the same way: a sender reserves each block of 100 generations, synced, before its first send past the block, and resumes past the block after a crash. So a crash never makes the sender reuse a generation. Receivers accept the skipped generations within their forward distance of 1000.

## Seeder hints: what a seeder learns about thumbnails and previews

A member that posts a picture, or fetches a message's thumbnail or preview picture, sends the community's seeders
the hash and ciphertext size of each blob. A seeder then keeps that ciphertext, within its `MirrorPolicy`, so a
member who joins later can fetch it while the sender is offline.

- **A seeder learns** the hash and size of the blob, and that a member of the community shared it. The manifest
  already gives a seeder the same for attachments, so this adds no new kind of leak.
- **A seeder does not learn** the thumbnail's content, its key, its name, the message's text, or which of the
  community's members will read it. It cannot open the blob, and it holds no key for it.
- **Only members can hint.** A seeder answers a hint only from a device whose root is a member of the community,
  and only within its per-device limit of 30 hints a minute. A non-member gets an error.
- **Other members hold the same blob.** A member that is online may also serve a thumbnail, so a seeder's copy is
  one of several. The seeder can see the requests it receives, as any provider can.

## Relay for my servers: what a host sees

A member can host a relay for the servers they belong to (`packaging/README.md`). The host's relay
carries only sealed traffic. It cannot read messages, files or calls, because those are end-to-end
encrypted, and the relay holds no keys for them. What the host does see is metadata:

- **Who connects.** Each client's IP address, and its device key, which is how the relay admits it.
  Only keys on the server's member list are admitted, so the host learns the device keys of the
  server's members and nothing about strangers, who are refused at connection time.
- **Who talks to whom.** The relay routes each packet to a destination device key, so it learns which
  two devices exchange traffic and when.
- **How much.** Byte counts and timing per connection. The bandwidth cap is enforced on these counts.
- **Membership changes, indirectly.** When a device is banned or leaves, the host's allow-list changes.
  The host learns that membership changed, not why.

The host can check reachability itself: "Check my relay" asks a member on another network to connect to
the relay and reports the result. The service does not check its own reachability, since it cannot see the
internet from inside its network. A ban closes the banned device's open connections at once, with
iroh-relay's `Clients::disconnect`, and the relay keeps running for everyone else. Nothing is restarted, so
other members see no disruption. The relay cannot tell which community a packet belongs to, only which
device keys it moves between.

## Relay choice: who picks relays, and what the list reveals

A device uses up to 30 relays (`MAX_RELAY_LIST`). Each one comes from one of three sources, and Settings >
Network shows which:

- **Yours.** Added in Settings. At most 16. Stored only in `network.json` on this device.
- **Community.** Named by a `SetRelays` control event. Only the owner or an admin can publish one; the
  engine rejects it from anyone else (`events/state.rs`). At most 4 per community, and they share the
  node's list with the rest.
- **number0.** The public defaults, on unless turned off.

Duplicates are merged, and the first source keeps the label (your own choice before a community's, a
community's before number0's). Pins and turn-offs are local. They are never sent to a community or a
relay. A community relay cannot be turned off on this device yet. The node adds it on join, and only its owners and admins can remove it.

**number0's device directory.** With "Find devices through number0's directory" on (the default), a device
that uses number0's relays publishes its device ID and addresses to number0's DNS directory (`dns.iroh.link`),
and resolves peers through it. number0 sees both. Turning it off stops that. Custom relays never use the
directory, whatever the switch says. The Mainline DHT and the LAN find peers either way.

**Measurement reveals your address to every listed relay.** iroh's net report probes each relay in the
list from this device, so each one sees this device's IP address, even a relay that never becomes home.
A longer list therefore tells more operators when and from where this device is online. Turning off
relays you do not trust shortens that list. The home relay is the one that carries traffic, and it
already learns the same things listed under "Relay for my servers".

**Choosing the home relay.** iroh picks the home relay from the latencies it measures, and moves only
when another relay is clearly faster: a new best latency must be under two thirds of the current one.
iroh does not keep a connection open to the second-fastest relay. If the home relay fails, iroh measures
all of them again and picks afresh, so failover takes one measurement round. Dissent shows the
measurements; it does not choose the home relay itself.

**Bad certificates are not used.** A relay must present a certificate that the public roots trust for
its name. One that fails the TLS check never completes a connection, so it is never home. It stays in
the list with no latency. An identity check (the device key, on the relay's member list for a hosted
relay) still applies on top, as above.

Tests: `dissent-net/src/relay_rank.rs` covers the merge, the cap and the ranking. `dissent-net/tests/relay_home.rs`
runs two loopback relays, one behind a proxy that adds delay, and checks that the faster one becomes home.

**Relay checks connect to an address a peer picks.** A `RelayCheck` makes a member's app connect to a
relay URL that the asker publishes for this server. The responder refuses unless the member uses server
relays, the asker publishes that exact URL, the URL is https, and every address its name resolves to is
public (no loopback, private, link-local, CGNAT or multicast address). The rebinding caveat: the responder
resolves the name for that check, and the connection resolves it again, so a name whose answer changes
between the two can point the connection at another address. That is acceptable, because the relay
connection needs a valid TLS certificate for the name. A rebound address on the member's network fails the
handshake, so all such a check can learn is that a TCP connection was attempted. A member that is in a call
refuses, so a check never causes an audio gap.

## Findings

1. **Fixed: unauthorized MLS commits are refused.** MLS lets any group member commit, so a
   malicious or compromised member could have added an outsider's device and honest devices would
   have encrypted to it. Now every peer checks each commit against the event DAG before merging,
   and refused commits are logged and never applied. Commits travel as signed announcements, and
   peers fetch the bytes and verify their hash. The checks:
   - the author must be a current member, and the add, removal and welcome rules in `docs/design/05-encrypted-servers.md` must hold
   - every added device must belong to a current, non-banned member
   - removals must target only banned or departed members, or the committer's own stale devices

   Joiners apply the same check to a welcome, so they're never welcomed into a group containing
   non-members.
2. **Phases 1 and 2 fixed; phase 3 partly fixed, the DHT record and control payloads are sealed, emoji is open.** Invites carry a per-community secret, which salts the gossip topic and keys a token on every community request. Each ban rotates the secret: the new one is sealed under the group in a `SecretRotation` event, and storage members and joiners fetch it from an active member. A banned device is refused every community request, even with a valid token. A retired secret works for 30 days, so invites made before a ban keep working for that long. When two admins rotate at once, the replay keeps the rotation with the lowest event id, and every device adopts it. The losing secret is retired, with grace, like any other. Before the fix, anyone who knew a community id
   can join its gossip topic or sync it, and read control events: display names, handles, roles,
   bans, membership timing and reaction emoji. DHT nodes learn the community id and the plaintext
   name in the community record. **Fix:**
   - Derive the gossip topic and an access key from a per-community secret carried in invites.
   - Require proof of that secret, or known membership, for sync RPCs.
   - Encrypt control payloads and the record's name under the access key.
   - Seal reaction emoji under MLS.

   Decided and phased: old invites stop working; phase 1 gates the gossip topic and sync RPCs
   behind the invite secret; phase 2 rotates the secret on every ban; phase 3 encrypts control
   payloads, the DHT record name and reaction emoji.
   **Phase 3, DHT record: done.** The record is sealed with XChaCha20-Poly1305 under a key derived
   from the current community secret, with the community id as associated data. The community key
   signs the record inside the ciphertext, so a secret holder cannot forge it. The DHT key is derived
   from the same secret, so a stranger who knows only the community id does not find the packet. The
   record follows each rotation, because the publisher derives from the current secret. A holder whose
   invite names a retired secret looks in the wrong place, and its join falls back to the invite's own
   peers.
  **Phase 3, control payloads: done.** The membership, role, name, handle, channel, relay and key
  rotation payloads (`Join`, `JoinAsStorage`, `Leave`, `Ban`, `Unban`, role grants and revokes,
  `HandleClaim`, `ChannelCreate`, `VoiceChannelCreate`, `SetRelays`, `KeyRotation`, `DeviceRevoke`)
  are sealed as `SealedControl`. Each is encrypted with XChaCha20-Poly1305 under a key derived from
  the community secret of the generation it was made in. The event's header (community, channel,
  author, device, parents, hlc and generation) is the associated data, so a ciphertext cannot be
  moved to another event. The event id and signature cover the ciphertext, so relays, gossip hops
  and DHT nodes verify authorship without a key. Replay opens each payload with the secrets of every
  generation the device holds, as `Keys.history`. A joiner or storage member fetches the older
  generations from an active member with `SecretHistory`, which a banned device cannot get. A
  payload whose generation it lacks stays locked until the key arrives, and the replay is rebuilt
  then. Message content was already under MLS, and the rotation payload stays readable, because it
  carries the keys.
  **Phase 3, member-only control: done, under the owner's switch.** With `MemberOnlyControl { on: true }`, new control events are sealed under the MLS epoch's key, exported from the community group, and not under the community secret. A storage member is never in the group, so it cannot open them, and a removed device loses the key when the removal commit moves the epoch. The owner can turn the switch on only when every member device supports it. Replay opens both forms, so old DAGs keep replaying. See `docs/design/05-encrypted-servers.md`, "Member-only control".
  **What remains in phase 3:**
  - *Before the switch.* Events made before the owner turns it on stay under the community secret, so storage members read them. A new community starts with the switch off, and its first events are readable to storage members until the owner turns it on.
  - *Invite events.* They stay under the community secret, so storage members read them, including invite ids and expiry. Storage members already hold the secret that invite links carry.
  - *A member can break the switch.* Replay does not check the form of a control event against the switch. A member that sends a community-secret event while the switch is on exposes that one event to storage members. Honest devices never do.
  - *Storage members push junk.* A storage member holds the write key, so it can make tags that pass for any ciphertext. Other seeders keep such junk. Members refuse it, because it does not open under the epoch key.
  - *Residual metadata under the switch.* Anyone with the DAG sees each member-sealed event's author root and device, channel, parents, hlc, the epoch number and the payload length. The epoch number counts membership changes, and storage members see it with the time. The kind of the payload is hidden.
  - *The window before a removal commit lands.* A ban, leave or departure moves the epoch when the removal commit applies. Until then, the removed device still holds the current key, and it can read events made in that epoch. The commit waits for the removing device's turn, so this is short but not zero.
  - *A member can send a wrong epoch key.* A joiner stores the keys that members send, and an epoch it already has keeps its key. A bad key leaves that epoch's events locked for the joiner, and the member cannot read or forge anything. Joiners check nothing against the DAG here, the same as older secrets.
  - *Leaving does not rotate the community secret.* Under the switch, a departed member loses the epoch keys. Before the switch, it keeps the community secret until the next ban rotation, as before.
  - *Two admins rotating at once.* Events sealed under the losing secret open only on devices that held it. Other devices show them as locked, so their replay can differ until they fetch it. The history fetch reaches only the devices that answer, so this is a known gap.
  - *Residual cleartext metadata for old events.* Anyone with the DAG sees the event id, author root and device, channel, parents, hlc, the generation and the payload length of a community-secret control event. Clear kinds remain: message content kinds, `Delete`, reaction emoji, and `SecretRotation`. A rotation names the banned roots it covers, so a banned root's id and the time of the ban are visible, though not the ban's author or reason.
  **Still open in phase 3:** reaction emoji. Member-only control does not cover it.
3. **Fixed: key-change warnings.** The engine raises `IdentityChanged` when someone you share a
   server with or have messaged rotates their key through recovery, or when a person you've
   messaged gains a new device. The change is recorded on their profile, and the app shows a
   notice to re-verify the safety number.
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
7. **Fixed: data at rest.** The identity file, the MLS state, the decrypted content log and each founder's
   community signing key (`community.key`) are sealed with XChaCha20-Poly1305 under a per-file subkey of the data
   key. The community key's label names its community, so a copied file doesn't open in another community's folder.
   The data key is held in the OS credential store: Windows Credential Manager, macOS Keychain or Linux Secret
   Service. Argon2id passphrase derivation is the fallback for headless machines. A plaintext MLS snapshot, content
   log or community key from before this change is sealed on first open. A plaintext community key is exactly 32
   bytes, and any other file that doesn't open is refused and left as it is. A wrong key refuses to start the engine
   rather than overwrite the state. A portable copy re-seals all of these under its own passphrase, so the USB copy
   doesn't depend on this machine's keychain. Malware running as the user is out of scope.

## How the fixes are verified

Each fix lands with an adversarial test:
- A malicious member commits an outsider's device, and every honest peer rejects it.
- A welcome into a group with a non-member is refused.
- A peer without the invite secret can't sync or read control events.
- A replayed SFrame frame is dropped.
- An accepted rotation raises a warning.
