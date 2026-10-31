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
| Presence privacy | Online status is shown from signed beacons, sent every 30 s to community members and direct contacts. Invisible sends none, and going invisible sends one offline beacon so peers drop you at once. Invisible also holds back the other signs of being online (see "Invisible mode"). Status is per device, not per account. |
| Typing privacy | Typing signals are signed by the device, kept in memory only and never stored. A public channel's go on the community topic, to members. A private channel's go direct to its readers' devices, and a receiver drops one that came on the topic or from a non-reader. A DM's or group's go direct to its members. Invisible sends none. |
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

Two holes in that audience are closed (`request_gates.rs`, `announce.rs`):

- **Joining hands out no addresses.** `Genesis` and `Reconcile` answer with members' signed addresses
  only when the requester is a current, unbanned member. Before, anyone holding the community secret
  (a leaked or revoked invite link, a departed member within the grace period) got up to 64 members'
  addresses with their direct IPs, without ever publishing a `Join`. A joiner now gets the genesis
  and the events it needs, and fetches the addresses again once its `Join` is in. It then pushes its
  `Join` to every member it can reach.
- **Old DM peers stop getting our address.** Address announcements went to every root that ever sent
  an accepted legacy direct message, for ever. They now go only to peers still in scope: a contact,
  or a member of a community we are in.


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

**Storage grants (phase 1).** A storage join still needs no grant, so the gap in `05-encrypted-servers.md`
("Storage joins") stays open until phase 2: a departed member can rejoin as storage under a fresh root. What phase
1 adds is visibility. Every seeder is listed in Server Settings › Members as granted or not, and an ungranted
seeder asks the owner and admins for a grant, so a seeder nobody recognises stands out. A grant is a signed
control event under the community secret, so only an owner or admin can issue one. Its ask reveals nothing new:
the seeder's root and device are already in the member list. An ask is kept in memory only, from a current,
unbanned storage member, and a person's ask is refused.

A seeder is a member that keeps the records of its communities' members and answers lookups from
members. It learns what any member learns: device ids and addresses of the members it holds, and
which device each member asks about. A seeder cannot forge a record, because devices sign their own,
and a stale record is bounded by its expiry. A seeder can withhold records, which only costs
availability; the dial then falls back to the DHT or another member. Answers from a seeder are
labelled `seeder` in the dial stats, so the user can see when one was used.

Relays are not lookup points in this version. A relay forwards QUIC packets it cannot read, and it
holds no records. A relay that also acted as a lookup point would need the same signed-record
design, and it is not built.


## Invisible mode: what still shows that you are online

Invisible is meant to look like being offline. While it is on (`invisible.rs`):

- **No beacons, no typing.** One offline beacon goes out when you turn it on.
- **Receipts are held.** A message that reaches you is stored and shown, but its delivery receipt waits
  until you are visible again, so the sender does not see it delivered at once. Held receipts are kept
  in memory: a restart drops them, and those messages simply never show as delivered to the sender.
- **No profile sync** with a contact whose beacon arrives.
- **No address announcements.** A new address is signed and kept, but not announced on community
  topics, not sent to contacts and not pushed as a peer-exchange record. The user record is not
  republished. All of it goes out when you are visible again.
- **No answers to others.** A request from a device yours has not dialled in the last 10 minutes
  gets no reply: the connection is closed, as when a device goes away. Your own other devices are
  always answered. A device you contacted recently is answered too, since it already saw you online:
  the members of a call you joined, the peer of a sync you started, the recipient of a message you
  sent. A request that is not answered is retried by its sender as for an offline device: a direct
  message goes to your inbox, which your device collects as usual.
- **Except mail this device holds for others.** A device that holds offline mail for a person (a friend
  it hosts, or its own inbox switch) still answers the inbox requests for that person while invisible:
  deposits, collects, acks, key packages and probes. Hosting is consent to that.
  - **The cost:** anyone who writes to a hosted person, or knows their root and this device's id, can
    tell this device is up.
  - **What it does not show:** other requests stay unanswered, including inbox requests for people
    this device does not host (`inbox::hosted_inbox_request`).

What still shows you are online, by design or because the app cannot hide it:

- **What you send.** A message you send, a reaction, a call you join, a server you open: whoever
  receives it knows you were online when you sent it. Requests your device makes also reach their
  recipients, which is why those devices are then answered.
- **Gossip.** Your device stays on its communities' gossip topics, so its gossip neighbours keep a live
  connection to it, and community messages keep arriving.
- **The network.** A device that dials yours by id still completes a connection with it, and so learns
  it is reachable and its address (`P-05` in the privacy audit), even though no request is answered.
- **Community records.** A server's DHT record is republished by its members as before.

## Hide my IP address: who still sees it

With "Hide my IP address" on (Settings › Network, `NetworkSettings::hide_ip`), the node runs with
`NodeConfig::relay_only`: it binds no IP socket at all, so every connection goes through one of its
relays, and no direct path can form. A peer that dials this device, by its id or from a link, reaches
the relay, and learns the relay's address, never this device's. Signed addresses and peer-exchange
records list only the relay URL, so a co-member's or contact's cache holds no IP either.

It also turns off what would talk to strangers from this device's own socket:

- **The Mainline DHT.** Both iroh's address lookup and the engine's own client (community records, the
  user record, short links) are off, since a DHT query shows the asker's IP to the nodes it asks, next
  to the keys it looks up. Short invite links and address links therefore don't open; full links do.
  This device publishes no community or user record, so other members keep those current.
- **LAN discovery.** Off, since its beacons go out on the local network.

Who still sees the IP:

- **The relay.** It sees the IP of each device that uses it, and which devices exchange traffic, when
  and how much (see "Relay for my servers"). It never sees content, which is end-to-end encrypted.
  Every relay in the list is measured from this device, so each one sees the IP; pinning or turning off
  relays shortens that list.
- **number0's directory**, while "Find devices through number0's directory" is on, as for any device
  that uses number0's relays.

The cost is latency and throughput: calls, file transfers and syncs all go through a relay hop. The
setting is off by default and applies when the engine restarts, which the app asks about first since
it ends a call in progress. Tested by `tests/it/hide_ip.rs` (two relay-only engines join a server,
exchange channel and direct messages, and hold no IP address for each other) and
`tests/it/relay_calls.rs` (calls over the relay).

## Who learns what from a device's answers

A device answers requests from any device that dials it. These rules keep the answer from telling a
stranger more than the request already did (`request_gates.rs`):

- **One refusal.** A stranger asking about a community gets the same refusal, `access denied`,
  whether this device is in that community or not, and whether the token was wrong or missing. The
  relay check and peer exchange refuse the same way for an unknown device and one out of scope. A
  call refuses a join, a tier message or a forwarder offer from someone out of scope with the same
  `not in this call` a device not in that call sends, and keeps no address for them.
- **Key packages.** A key package carries the device's certificate, so it ties a device id to its
  person. `KeyPackages` (for the member that adds a joiner to a community's group) is answered only to
  a current, unbanned member of that community. `DmKeyPackage` is answered only to a device whose
  person this device already knows from a certificate it was shown (a contact, a co-member, someone
  who asked to be a contact or to join a group), and who is not blocked. That person knows whose
  device this is already; whether they may then message this person is still the DM policy's call
  when the message arrives. A stranger who only knows a device id gets nothing. Under the `Everyone`
  policy a device this one does not know may ask too, since anyone may then message this person. Each
  requester gets at most 12 an hour, so a pool cannot be drained.
  An inbox's `TakeKeyPackage` already names the person it asks for, and the inbox does not know whom
  its owner accepts, so it serves anyone, within the same per-requester limit and 60 an hour in all.
- **Version probes.** `DmDevicesSupport`, `HistorySupport` and `RotationSupport` are answered only to
  a device in scope (a contact, or a member of a community we are in). A stranger gets the refusal,
  which reads like an old client, so it cannot tell the version.
- **Relay checks** are counted against the requester's limit before the "busy in a call" answer, so
  asking again and again cannot track this device's calls.

What a stranger can still learn: that the device is online, since it answers at all, and its address,
since it dialled it (`P-05` in the privacy audit).

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
- whether the named invite is live, or unknown to the answering member (a fresh invite whose event
  has not reached it yet)

A **revoked or expired** invite id gets no preview: the member answers "access denied", the same reply
a wrong secret gets, so the holder of a dead invite learns neither the counts nor that the server
still exists (privacy audit P-16). Members from before this change still answer such an invite with
its status.

It shows no member name, device id, channel, or message. The answer carries the community's own
genesis, so the name and date check against the community key (the genesis names the owner's root,
as the community's own record does). It is signed by a key made for that one answer and certified by
a root made for it, then thrown away, so the answer names neither the answering member's root nor its
device cert, and two answers cannot be linked by their key (P-16). Released clients check that cert
and signature as before, and both hold. The requester still dials the answering device, so it learns
that device's id and address from the connection, as anyone who dials a device does. Each requester
gets ten previews a minute from one device, and a requester reuses a preview for 60 seconds.

Who learns what:

- **Invite holders:** the name, icon, counts and invite status above, while the invite is live.
- **Strangers who know only the community id:** nothing. The request has no valid token, so it is
  refused.
- **A banned device:** refused, as for every community request.
- **DHT nodes:** the sealed record only. They learn that a key exists, as for community records.
- **The answering member:** learns the requester's device id and address, as it does when joining.

The online count comes from presence beacons. It is a count, not a list, is shown to invite holders
only, and can be stale by up to the presence window. It is not coarsened: a holder of a live invite
can poll it about once a minute per answering device.

**Who is in a voice channel (privacy audit P-30).** Each device in a call re-sends its roster to the
server's members, and a roster lists everyone that device sees in the call. A member could list
people who are not there, to fake a crowd or to make a channel with a user limit look full. Now a
person is shown in a voice channel, and counted against its user limit, only from their own device's
report (`Heard::own_entry`), and only while they are a member who is not banned. The same rule builds
the roster of a live DM or group call shown in its lobby, where a person must also be in the group. A
participant whose device cannot reach a viewer directly is not shown to that viewer until it can: the
cost of trusting only first-hand reports. A member can still claim to be in a channel themselves.

**Addresses in links that leave the members (privacy audit P-03).** A long invite, a contact link (the
form used when adding by address is off), the card in a contact request or accept, a contact link
re-shared from someone's card, a group invite and a group join request all go to people who may share
no server with the sender, and can be forwarded or posted in public. Each one carries the device id,
the relay URL and the device's public addresses only (`communities::shareable_addr`). LAN (RFC 1918),
VPN and carrier-grade NAT (100.64/10), link-local, IPv6 unique-local, loopback and other local
addresses are left out, so a posted link does not map the sender's network. A public address does
still go in, so whoever holds the link learns the sender's public IP, as they would by dialling the
relay-reachable device. Direct local addresses reach co-members only through the signed-address
announcements and peer exchange above. A node that runs on loopback alone (tests and local
multi-instance checks) keeps its loopback addresses, its only ones. Links from older versions still
parse and join with whatever they carry.

**Invite cards in chat (privacy audit P-04).** A preview dials the devices and relay that an invite
names, and anyone can mint a long invite naming their own. So an invite link in a message shows a
neutral card, "Invite to a server", and nothing is fetched or dialled until the viewer clicks Show
preview. A preview already shown in this session shows again without a click. A preview dials only
the public reachability of a long invite's peers, never a local address the invite names, so a forged
invite cannot make a viewer probe a local network. Joining still uses every address the invite
carries. Opening a `dissent://invite/` link from outside the app shows its preview at once, since
opening it is the click.

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

## Photo and video metadata: what a sent file carries

Audit finding P-28. A photo went out with its EXIF block (the GPS position where it was taken, the camera and its
serial number, the owner's name, dates), and a phone video with its recorded location and device, so anyone who could
read the message could learn where, when and with what it was made, and often by whom. The file name said as much
("IMG_20261029_1530.jpg", "Maya's birthday.jpg").

**A per-file choice.** Before a file is sent, the Share dialog and each staged file in the composer show "Remove
identifying details (location, device, dates, names)". It starts on for photos and for videos that name the device or
place they were recorded with, and off for music and audio (their artist and album tags stay), for other videos, and
for documents and every other file. Settings › Contacts & privacy › "Remove identifying details from photos and videos
by default" sets the starting state for photos and videos; the user can change it for each file. The engine decides
the defaults (`inspect_files`, `attachments.rs`), so they are the same in every window.

**What "on" does.** The file is sent as a copy, written to the data folder's `staging` folder and deleted once it is
encrypted, under a generic name: `photo.jpg`, `video.mp4`, `audio.m4a` or `file`, with a short random suffix when one
send has several. The file on disk never changes, and the sender's own message shows it under its own name. The copy
keeps what a viewer needs: the pixels and samples (never re-encoded), dimensions, rotation and colour
(`dissent-core/src/scrub/`). Per format:

- **JPEG:** Exif, XMP, IPTC and Photoshop data, MPF and JFXX previews, comments and every other application segment
  are dropped. JFIF is kept without its thumbnail, and a rotation the Exif block asked for is kept in a new Exif
  block that holds the Orientation tag alone.
- **PNG:** only the chunks a viewer needs are kept (image, palette, transparency, gamma, colour, density, animation);
  text, `eXIf`, `tIME` and unknown chunks such as C2PA go.
- **WebP:** `EXIF`, `XMP ` and unknown chunks go, and their flags are cleared.
- **GIF:** comment extensions and application extensions (XMP among them) go, except the two that make it loop.
- **TIFF and DNG:** in place. In the image directories the make and model, software, dates, artist, host computer,
  copyright, description, document name, XMP, IPTC, Photoshop data and DNG's camera names, serial number, original
  file name and private data are zeroed; the Exif, GPS and interoperability directories are zeroed whole, MakerNote,
  serial numbers, owner name and unique id included.
- **HEIC, HEIF and AVIF:** in place. The Exif and XMP items, found through `iinf` and `iloc`, are zeroed, and a
  top-level XMP box becomes a zeroed `free` box. Rotation is in item properties and stays.
- **MP4, MOV, M4V and M4A:** in place, so no chunk offset moves. Every `udta` and `meta` box becomes a zeroed `free`
  box (location, make, model, software, dates, author, and every Apple and Android key with them); creation and
  modification times in `mvhd`, `tkhd` and `mdhd` become 0; handler names and each video sample entry's compressor
  name and vendor are zeroed; a top-level XMP box becomes `free`.
- **Matroska and WebM:** in place. The title, date, muxing and writing applications, every track's name, all tags and
  all attachments become zeroed `Void` elements.
- **Colour profiles** (in JPEG, WebP and TIFF): the colour data stays; the description, copyright, device maker and
  model text, the header's device and creator fields, date and profile id are cleared, since a calibration profile is
  often named after the display or its owner.

**When the details cannot be removed.** A photo in a format not handled here, or whose structure is not one these
readers accept, is decoded and sent as a PNG of its pixels alone when the image crate can decode it (JPEG, PNG, GIF and
WebP). Anything else, such as a BMP or SVG picture, an AVI or a live WebM recording of unknown size, an MP3, or a
document, cannot be cleaned: when "on" was chosen for it, sending asks "Couldn't remove details from X. Send as is /
Cancel". The engine refuses the send itself if the copy cannot be made, so nothing goes out with details the user asked
to remove.

**What this cannot remove.** What is in the picture or the recording itself (faces, places, a voice, a reflection,
a screen); the camera sensor's noise pattern (PRNU), which can tie a photo to the camera that took it when someone
holds other photos from it; and traits of the compression and of the processing a phone applies. A split ICC profile
(more than one APP2 chunk, rare) and a compressed PNG `iCCP` profile keep their text. The exact file size, and the
message's own time, stay visible to anyone who can read the message.

## Image thumbnails: what a preview reveals

A sender may attach a thumbnail to an image: a JPEG of at most 320 pixels on its long side, under
64 KB, with EXIF and all other metadata dropped. It is stored and encrypted as its own blob, under its
own key, and the message carries its key and hash inside the same sealed text as the file. Beyond
the file itself, a thumbnail adds one more blob and its size to what a holder of the message can
see, and the size tells roughly how detailed the picture is. A recipient fetches it on arrival,
from the sender's devices, so the sender's devices learn that the recipient opened the message, as
they already do for a file that is fetched. Seeders hold the ciphertext of the main file only, and
do not mirror thumbnails. The sealed text, not the blob, is what the keys protect.

A video gets the same kind of thumbnail, made from its first decoded frame: the same caps, the same blob and the
same sealed reference. The frame is decoded and re-encoded as a JPEG, so the video's container metadata (creation
time, device, location) is not carried. Anyone who can read the message sees that frame without downloading the
video, as they see a picture's preview. A sender who does not want the opening frame shown has no switch yet.

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

The member that posts a picture sends the community's seeders the hash and ciphertext size of each thumbnail and
preview picture in that message. A seeder then keeps that ciphertext, within its `MirrorPolicy`, so a member who joins
later can fetch it while the sender is offline.

Audit finding P-12. Every member that fetched a thumbnail or preview picture used to hint it too, so a seeder learned
which member device fetched which media, and when. **Now only the sender hints**, once, as it posts. A fetch tells the
seeders nothing. A thumbnail is hinted only when its file is marked `seed` (the sender let seeders keep a copy), so a
file kept off the seeders keeps its thumbnail off them too. Preview pictures carry no such mark and are hinted by the
sender. A private channel's media is never hinted, as before.

- **A seeder learns** the hash and size of the blob, and that the sender shared it. The manifest already gives a
  seeder the same for attachments, so this adds no new kind of leak. It no longer learns who else fetched it.
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

## A forwarder who is not in the call: what a volunteer sees

A member can turn on "Help Forward Calls" for one server (off by default). While their device is in
no call of its own, it may be elected to forward a voice channel call in that server that it is not
in (`docs/design/13-forwarded-calls.md`, phase 3). Every participant's media then goes through it.
Its relay holds no media key and forwards SFrame frames unchanged, so it cannot read what is said
or shown. For as long as it forwards, it learns:

- **Who is in the call.** Every leaf links to it, so it learns each participant's device key and,
  through the server's member list, who they are. A participant forwarder learns this too, but a
  volunteer is not in the call: it learns the roster of a call it chose not to join.
- **Who speaks, and when.** The audio-level byte of each frame, the frame timing and sizes, and
  which senders send video and at which layer, as any forwarder does (finding 6).
- **Each participant's address.** The IP address of each leaf's link, or the relay it comes
  through.

The volunteer is not listed as a participant, so the call screen names it: "Forwarded by <name>".
Signalling already showed it to the call's devices. What limits the exposure:

- **Members only.** Each device in the call accepts an offer only from a current, unbanned member
  of the server, in its MLS group, and checks again before it links and on every membership tick.
  A ban fails the call over to a participant at once. An outsider's or a banned member's offer is
  refused. The offer is signed by the volunteer's device key and lapses after 60 s unless renewed.
- **Opt-in, and separate.** Volunteering is per server and off by default. Hosting a relay for the
  server does not turn it on. A seeder forwards only while the owner's "Seeder may forward calls" is
  on, which is off by default and only the owner can change.

**What a seeder hears of calls (P-12).** A seeder is in no call. It used to get every voice channel's roster
(people, mute and deafen state, start time) and every join, which carries the joiner's addresses, because calls went
to every member device, storage members included. Now a join never goes to a seeder, and a seeder that sends one is
refused. A call's roster goes to a seeder only while the owner's "Seeder may forward calls" is on and the call has
enough people to need a forwarder (six), since a seeder hears a call only to offer to forward it. That roster names
who is in that one call, without mute, deafen or start time. With the setting off, or for a smaller call, a seeder
hears of no call at all.

**A seeder that forwards** learns the same: the call's roster, who speaks, and the participants'
addresses, which it otherwise never sees, since it is in no call. It holds no MLS epoch key, so
unlike a member volunteer it cannot decrypt the media even with a modified client. It learns each
leaf's sender id from the leaf (`RelayLeaf`); a member could name another member's id first and
make the seeder refuse that member's link. That disrupts the call, as a hostile participant
already can.
- **Older clients.** A call with a device that does not support volunteers never uses one.

**Residual risk: a volunteer is a member, and members hold the key.** A voice channel's key is
exported from the server's MLS group, which every member is in. An honest volunteer never derives
it: its relay takes no key, and its unused mesh holds a random one. But a modified client could
derive the key and decrypt the media it forwards. That is no more than a member could hear by
joining the call, except that it is not listed among the participants. The "Forwarded by" label is
what keeps it visible. A participant who doesn't want a member outside the call to carry it can
see who does, and the server's admins can ban a member who abuses it. A volunteer can also degrade
the call by dropping frames. Its leaves catch that: once two of them have heard no audio through it
for 5 s, one of them while speaking itself, every device excludes the volunteer for the rest of the
call and moves to a participant forwarder. A volunteer that drops only some frames, or only one
leaf's, stays below that and is not caught. A participant forwarder is not checked this way yet.

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

## Typing in private channels: who learns who types

Audit finding P-14. Channel typing went on the community topic, so every member, storage members included, learned
who was typing in a private channel and when, even without being able to read it.

**Now readers only.** A private channel's typing signal goes direct to each device of the channel's readers, the same
way DM and group typing does (`dissent-core/src/typing.rs`). A receiver accepts it only when it came direct, the typer
is a member who can read the channel, and so can the receiver. One that arrives on the topic is dropped. A device that
cannot read the channel refuses to send one. Who may read comes from the receiver's own replay of the channel's
latest access change, not from the signal.

**Still visible.** Readers see who types, as in any channel. The direct connection itself shows that two devices are
talking, as any direct message does. An older client still broadcasts its private-channel typing on the topic.
Upgraded receivers ignore it, but non-readers on older clients still show it.

## Blob fetch: who gets a file's ciphertext

Audit finding P-13. A blob is a file's ciphertext under its BLAKE3 hash. The blob server used to serve any device
that named a hash, and nothing ever unpinned what a device shared or mirrored, so a deleted message's file stayed
fetchable for as long as a device that held it ran.

**Now members only.** Each device's blob server checks every connection and every `get`, `get_many` and `observe`
against the scopes the blob belongs to (`dissent-core/src/blob_gate.rs`, and `08-files.md`, "Who may fetch a
blob"). A public channel's blobs go to the server's members, a private channel's to its readers, a conversation's
to its members. The check runs per request, so a ban takes effect on an open connection. A device we hold no
certificate for is refused, and asked for its signed record so its retry can pass. The record proves only which
root the device belongs to; membership comes from our own state.

**Unpinned once nothing refers to it.** A deleted message's blobs are unpinned on the sender, on every member, and
on seeders that mirrored them, unless another live message, manifest or picture still names them. So are the blobs
of a left server or group, and the pictures of a removed contact we share nothing else with. A sweep at start
catches what changed while the device was off (`dissent-core/src/blob_release.rs`). The store's garbage collection
then removes the bytes on its next hourly run.

**What remains:**
- *Public server art.* A server's icon and banner (still, animated and poster) are served to anyone, because invite
  cards show them before the reader joins. Their keys travel in the invite link and in control events. A stranger
  who learns such a hash gets ciphertext only, but can tell the blob exists and its size.
- *Shared pictures.* People's avatars and profile banners are served to contacts and to anyone who shares a server
  or a conversation with us, so a member of one of our servers can fetch the avatar we use in another, if it
  learns the hash. Each picture's key travels with the event or the signed profile that names it, so the blob alone
  shows nothing.
- *Ciphertext size and holders.* Every member who may fetch a blob learns its size (the plaintext to within one
  64 KiB chunk), and the providers it asked learn who fetched it and when. A seeder sees the hash, size and fetchers
  of what it mirrors, as before.
- *Deleted files are gone only where the delete arrived.* Unpinning follows the `Delete` each device receives. A
  device that is offline keeps the blob until its next start, a device that copied the plaintext keeps the
  plaintext, and a hint a seeder accepted is never released, because a hint names no message. A device that runs an
  older version still serves and keeps everything it pinned. Bytes stay on disk until the next garbage collection,
  within an hour.
- *Fresh uploads at start.* The start-up sweep keeps any file this device shared in the last day, so a message still
  on its way out keeps its file. A message deleted in that window has its file unpinned by the delete itself, or by
  the sweep a day later.

## Offline inboxes: what a host learns

A person picks inboxes in Settings > Offline delivery: another device of their own, a seeder they run, or a friend's
always-on device. A device holds mail only for people its owner chose: the inbox switch for its own root, a tick per
friend, or a seeder's `--inbox-for`. Details are in `10-dm-delivery.md`, "Choosing an inbox".

The host cannot read the mail. It learns:

- the person's root and the ids of their collecting devices;
- the sending device and root of each envelope, with its size and its time, so it learns whom the person hears from
  while away;
- when the person goes offline and comes back;
- each packet's clear fields: its kind, its conversation id, and for a welcome the group's name, its creator and every
  member's root.

It can drop, delay or refuse mail, but it cannot forge it.

These rules limit who learns what:
- **Probes.** A probe (`InboxProbe`) is signed by the asking device. A device that does not serve the asker
  answers exactly as one that serves no one, so the answer shows a stranger nothing.
- **Refusals (fixes audit L1).**
  - A deposit or key packages for a person this device does not hold mail for get one refusal, `NOT_SERVED`,
    whether this device is no inbox at all or an inbox for other people.
  - A collect on such a device gets an empty list.
  - A take finds "no key package for that user", whether the person is not served or their packages ran out.
  - A sender can see that this device holds nothing for that person, but not whether it is an inbox.
- **Start and stop notices.** When a device starts or stops holding a person's mail, it tells them with a signed
  contact message (`ContactMsg::InboxHost`). The notice reveals nothing new: the person learns that their
  host agreed or stopped, which they would see in its probe answer anyway.
- Looking for inboxes probes the asker's own devices, contacts and community seeders, so each of them learns the
  person looked. Settings says so, and lists what an inbox learns, next to the picker.

## Website handles: who learns who looks at a handle

Audit finding P-11. A server handle shaped like a domain, such as `@example.com`, is checked by looking up the TXT
record at `_dissent.example.com`. Each lookup reaches the viewer's DNS resolver and, through it, the domain's own name
servers. The app used to look up every such handle whenever it was drawn, in the member list and in message headers,
and asked again every few minutes. A lookup the system resolver failed to answer went to Cloudflare without asking. So
whoever runs a domain, or Cloudflare, could see roughly when someone in a server was looking at that person.

**Now checked on request only.** Drawing a handle never looks it up: the member list and message headers show only a
result this app already has (`website_handle_status` without `verify`). A lookup runs when the user opens that person's
profile card or full profile, or points at the handle in the member list. A result is reused for hours: a pass shows
the mark for 24 hours and is looked up again, on the next request, after 6; a failure after 1 hour; a lookup that could
not finish after 10 minutes. Every place a check can start says "Checking this asks your DNS resolver about
<domain>".

**Public resolver is a setting.** Only the system resolver is asked by default. Settings › Contacts & privacy › "Ask a
public DNS resolver when mine does not answer" (`dns_public_fallback` in `shell.json`, off by default) sends a lookup
the system resolver failed on to Cloudflare as well, and the hint then says so.

**Still visible.** A check still tells the user's resolver, and the domain's name servers, that this network asked
about that domain at that time. A domain owner who shares few servers with the viewer can still guess who opened
their profile. Results are kept for this run only, so the first profile opened after a restart looks the handle up
again. The resolver itself is whatever the system uses, often the ISP's, and the lookup is plain DNS unless the
system is set up for encrypted DNS.

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
2. **Phases 1 and 2 fixed; phase 3 partly fixed, the DHT record and control payloads are sealed, and reaction emoji are sealed under MLS.** Invites carry a per-community secret, which salts the gossip topic and keys a token on every community request. Each ban rotates the secret: the new one is sealed under the group in a `SecretRotation` event, and storage members and joiners fetch it from an active member. A banned device is refused every community request, even with a valid token. A retired secret works for 30 days, so invites made before a ban keep working for that long. When two admins rotate at once, the replay keeps the rotation with the lowest event id, and every device adopts it. The losing secret is retired, with grace, like any other. Before the fix, anyone who knew a community id
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
  **Reaction emoji:** a reaction is sealed under MLS (`SealedKind::React` and `Unreact`), like message text, so only members see the emoji. Its target and author stay in clear. Member-only control does not apply to it, because a reaction is not a control event. Reactions from before this sealing carry their emoji in clear.
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
   loudly, through the audio-level byte. So does a volunteer forwarder, a member outside the call,
   which also learns the call's roster and addresses (see "A forwarder who is not in the call").
   Message metadata (author, time, channel) is visible to members, as in any group chat.
7. **Fixed: data at rest.** The identity file, the MLS state, the decrypted content log and each founder's
   community signing key (`community.key`) are sealed with XChaCha20-Poly1305 under a per-file subkey of the data
   key. The community key's label names its community, so a copied file doesn't open in another community's folder.
   The data key is held in the OS credential store: Windows Credential Manager, macOS Keychain or Linux Secret
   Service. Argon2id passphrase derivation is the fallback for headless machines. A plaintext MLS snapshot, content
   log or community key from before this change is sealed on first open. A plaintext community key is exactly 32
   bytes, and any other file that doesn't open is refused and left as it is. A wrong key refuses to start the engine
   rather than overwrite the state. A portable copy re-seals all of these under its own passphrase, so the USB copy
   doesn't depend on this machine's keychain. Malware running as the user is out of scope.

   **The rest of the store (P-06, P-15).** Every other file of a profile is sealed too, under the store subkey
   (`at_rest.rs`), with XChaCha20-Poly1305 and a label naming the file's place in the profile. So a file copied over
   another doesn't open. This covers DMs and group DMs (texts, edits, reactions, members, the outbox), saved
   messages and reminders, community secrets (`access.bin`), file keys, names and types (`attachments.bin`),
   contacts, requests and blocks, notes and nicknames, read times, server and rail preferences, guardians and
   devices, and the decrypted pictures: avatars, icons, banners, thumbnails and link-preview pictures.

   - **Format.** A `.bin` file is a 4-byte header and one sealed value. A log is a header and length-prefixed
     sealed records. A sealed file that doesn't open is an error, never read as plaintext.
   - **Migration.** The first start of this version seals every plaintext file of a profile before anything
     reads it. Each file is replaced atomically, and a copy of each waits in `unsealed-backup/` until all are
     sealed. That copy is then removed and the marker `store-sealed.v1` is written. A start that is interrupted
     runs again: sealed files are skipped, and readers accept either form. `tests/it/upgrade_compat.rs` checks
     this on a v0.1.20 profile and asserts that none of its text is left on disk.
   - **No plaintext copies.** Writes go through a temp file that holds the sealed bytes. Pictures are decrypted
     in memory (`Files::fetch_bytes`), never through a plaintext file. The window can't read a sealed picture,
     so the desktop app serves it through its own `dimg` scheme. That scheme opens only the picture folders and
     answers with `Cache-Control: no-store`.
   - **Permissions.** On Unix the data folder and the profile are kept to their owner: folders 0700, files 0600,
     on every start and for every new file.

   **What stays readable on disk:**
   - **Ciphertext others relay anyway.** These are the events log and the MLS commit log, and the blob store,
     which holds ciphertext.
   - **The store's shape.** Every file's name, size and times are visible. So are the folder names, which are
     community, conversation and peer ids, and the attachment ids of thumbnails.
   - **Downloads.** Files the user downloads are plaintext, in the folder they chose.
   - **Short-lived copies.** A photo whose metadata is being removed has a plaintext copy in `staging/` until it
     is encrypted. The original is on the disk anyway.
   - **The desktop app's own settings.** `network.json` (relay choices) and `website-handle.json` (the user's
     handle, which is public) stay plaintext.
   - **The window's storage.** The window keeps read markers, mutes and layout choices in localStorage, keyed
     by server, channel and conversation ids. It is in the WebView2 or WebKit profile, outside the data folder,
     so a portable profile leaves it on the host PC. The portable unlock screen says so. That profile also holds
     the webview's HTTP and GPU caches. Pictures are served `no-store`, so they shouldn't land there, but that
     isn't verified on every platform.
   - **SSD remnants.** On an SSD, the plaintext of a migrated file can survive in freed blocks until they are
     reused. Sealing doesn't wipe them.
8. **Fixed, with what remains documented: blob fetch and retention (P-13).** Blobs are served to members only, per
   scope and per request, and a deleted message's blobs are unpinned wherever the delete arrives. What remains is
   listed in "Blob fetch: who gets a file's ciphertext": public server art, shared pictures, ciphertext sizes visible
   to members, and devices the delete has not reached.
9. **Fixed, with what remains documented: website handle lookups (P-11).** A handle is looked up only when the user
   opens a profile or points at it, results are reused for hours, and the public resolver fallback is an opt-in
   setting. What remains is listed in "Website handles: who learns who looks at a handle".
10. **Fixed: what seeders learn of calls and media fetches (P-12).** A seeder gets no join, and a call's roster only
   while the owner lets seeders forward and the call is big enough to need a forwarder; the roster then carries no
   mute, deafen or start time. Only a message's sender hints its media to seeders, and a thumbnail only for a file
   marked `seed`. See "Seeder hints" and "What a seeder hears of calls".

## How the fixes are verified

Each fix lands with an adversarial test:
- A device outside a blob's scope, a banned member and a non-reader of a private channel are refused the blob; a
  fresh joiner is served on its retry (`tests/it/blob_gate.rs`).
- A deleted message's file is unpinned on the sender and on a seeder that mirrored it, and a live message's file
  stays pinned (`tests/it/attachments.rs`).
- A website handle status call without `verify` starts no lookup, and a recent result is reused rather than looked up
  again (`website_handle_cmds.rs` unit tests). Drawing the member list and messages looks nothing up, and pointing at
  a handle looks it up once (driver section `websitehandles`).
- A seeder hears no call while forwarding is off, nor a call too small to forward once it is on
  (`tests/it/voice_presence.rs`), and still forwards a call of seven once the owner allows it
  (`tests/it/volunteers.rs`). A picture kept off the seeders leaves nothing on the seeder, even after a member
  fetched its thumbnail, while a seeded picture's thumbnail still reaches a later joiner from the seeder
  (`tests/it/mirror_hints.rs`).
- A malicious member commits an outsider's device, and every honest peer rejects it.
- A welcome into a group with a non-member is refused.
- A peer without the invite secret can't sync or read control events.
- A replayed SFrame frame is dropped.
- An accepted rotation raises a warning.
