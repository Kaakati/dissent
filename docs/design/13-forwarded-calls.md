# Design 13: forwarded calls beyond the mesh

Status: phases 1 to 4 are built; phase 5 is design only. Nothing here is built unless it
says so. It extends `04-calls.md`, whose
"Peer-SFU", "Stages" and "Tier migration" sections describe what already runs, and
`12-boosts.md`, whose ceilings name a mesh limit per level.

## Where things stand

What runs today, in `dissent-media` and `dissent-core/src/calls.rs`:

- **Tiers.** A voice channel call is P2P for two devices and a mesh for three to six. From seven it
  becomes a forwarded call with one elected forwarder, and back to a mesh at five
  (`TierPolicy::tier_for`). A device migrates after its roster has wanted another tier for ten
  seconds. The new room is staged beside the old one, and a signed tier message keeps devices in
  step. From `cascade_at` devices (`CASCADE_AT`, 50) a forwarded call becomes a cascade: a root and
  child forwarders with standbys (`cascade.rs`). It runs over the network, tested on loopback with
  `cascade_at` lowered to 9.
- **Who forwards.** A participant, or since phase 3 a volunteer member who is not in the call
  (`calls/volunteers.rs`). `elect_forwarder` ranks the call's own devices by announced upload
  capacity, then the spec's rules (not on battery, wired, CPU ok, central, lowest id).
- **End to end.** The forwarder holds no media key. It routes SFrame frames by the key id and passes
  them on unchanged. It sees levels, sizes, timing and sender ids, never content (`04-calls.md`,
  "What a forwarder sees").
- **Failover.** Each participant keeps a pool of four forwarder links. It switches on a link error,
  a failed send, or 500 ms of silence, without a handshake. Measured recovery is 30 ms for a killed
  forwarder and 545 ms for a frozen one.
- **Video through a forwarder.** It is forwarded for the top speakers only, a whole keyframe or
  nothing per downlink. Since phase 2 a sender sends simulcast layers and the forwarder picks one
  per viewer ("Layers per viewer").

So "beyond the mesh" isn't new. What this design adds is forwarders that aren't participants,
layer selection, the boost limits, and volunteers in the cascade.

## Who forwards

A forwarder needs upload, a stable link and no battery limit. Three kinds of device can offer that,
in this order of preference for a given call:

1. **A participant.** This is today's rule, and it stays the default. The forwarder hears and is
   heard. Its own speech enters through an in-process link, so it pays no extra hop.
2. **A volunteer member.** Built in phase 3. A member who turned on "Help Forward Calls" for that
   server (decided: per server, off by default; it is in the server's menu beside the other
   per-server choices, since members have no Server Settings page), whose device is online and in
   no call of its own. They forward the call only: no audio and no video, never listed as in the
   call, shown as "Forwarded by <name>" on the call screen (decided: named, not anonymous).
   "On mains power and wired" is not checked yet: the app does not read the power source or link
   type, so a volunteer reports a wired device on mains, as the election assumes of participants.
3. **The seeder.** An always-on member (`apps/seeder`). It is often the best-connected device a
   server has. A seeder is a storage member. It is outside the MLS group and holds no epoch key, so
   it can't read media, and forwarding needs none. It needs a call role it doesn't have today:
   `JoinAsStorage` members may only join and leave (state.rs). So the seeder forwards only if the
   owner turns on "Seeder may forward calls" (an appended, admin-only control event, off by default).
   Decided: the seeder may forward only with that owner setting. Built in phase 4: the setting is
   `Payload::SeederForwardsCalls { on }`, appended and carried. No call role in replay is needed,
   since forwarding publishes no event: the seeder's engine reads the setting and volunteers on its
   own, and every device in the call checks it before it keeps the seeder's offer.

A **relay host** (`12-boosts.md`) is not a forwarder kind of its own. The relay binary carries
packets, not calls, and making it a media forwarder would put a str0m stack in `apps/relay`. Instead,
a host who also runs the app counts as a volunteer member, but only if they also turn on Help
Forward Calls (decided: hosting a relay and forwarding are separate opt-ins). Hosting a relay is a
tie-break in the election, since their device likely has upload. It breaks ties between volunteers
only, so a call with no volunteers elects exactly as before, the same as released clients do.

**Who may forward is checked on every device**, from the community state it already has, before it
links to a forwarder:

- the device belongs to a current, non-banned member, in the server's MLS group (so it has a
  sender id), or to the seeder while its setting is on;
- for a non-participant, the member has an active "forwarder offer" for this call (below), and
  every device in the call supports volunteers.

A device that fails the check is never linked to, whatever a tier message says: a tier message
naming one is refused. The check runs when an offer arrives, before every link, and on every
membership tick of a volunteer-forwarded call, so a ban or a lapsed offer fails the call over at
once. The volunteer checks the other way too: its relay takes a link only from a current,
unbanned member, and drops a leaf that is banned or leaves the server.

## Offers and election

- **Forwarder offer.** A non-participant offers itself with a signed, short-lived message on the
  call's existing signalling path (`CallMsg`, appended variant `ForwardOffer { room, capacity_kbps,
  wired, on_battery, features, ttl_ms, sig }`). It is signed by its device key under
  `dissent/call-forward-offer/v1`, renewed every `forward_offer_every` (20 s) and dropped after
  three of those (60 s, the most a receiver keeps one) unless renewed. Two changes from the first
  design: the lifetime is relative (`ttl_ms`, not `expires_ms`), so the devices' clocks need not
  agree, and a `ttl_ms` of zero withdraws the offer. And the offer carries the volunteer's
  `Features` bits, since a device that is not live in the call cannot send `Features`.
- **Who gets offers.** A volunteer learns of calls from the presence rosters every device in a
  voice channel call already sends to the server's members. While it is in no call of its own, it
  offers to one call at a time: the one it relays, else the largest it hears of with six or more
  people (a smaller call is a mesh, which no forwarder serves). It sends the offer to every device
  in that call, and withdraws its offer to any other. Joining a call of its own, turning the switch
  off, or closing the app withdraws its offers, so the call fails over at once.
- **One candidate set.** Every device ranks the same set: participants plus live offers
  (`elect_with_volunteers`). As in a tier change today, participants count at the election's
  default capacity, since devices see measured capacities at different times; a volunteer counts at
  its offered capacity. A non-participant gets a bonus of a tenth of its capacity, at most 1 Mbps,
  because it isn't spending upload on its own media. The bonus is capped, so a strong participant
  still wins over a weak volunteer. With no volunteers the ranking is exactly today's.
- **How a volunteer starts forwarding.** The migration that elects it is the usual one: a signed
  tier message names the volunteer's sender id as the forwarder, and each device in the call builds
  its staged room with that forwarder. Every device that migrates also sends its tier message to the
  volunteer. The volunteer builds a relay for the generation it names (the tier message's room,
  roster hash and forwarder give the generation id), and the leaves link to it as to any forwarder.
  The relay is a `Sfu` with no leaf side: no capture, no playout, no key. A device that follows a
  tier message naming a volunteer must hold that volunteer's live offer itself, so every device that
  links agrees on when the offer lapses.
- **Replacement.** A volunteer whose offer is withdrawn or lapses, or who is banned, fails the
  check above. Each device then migrates the call at once to a participant forwarder, elected on
  the roster alone so every device picks the same one, through the same staged migration as any
  tier change. This deviates from the first design: the pool of four forwarder links is not used
  for it, since core links each leaf to one forwarder. Measured on loopback, from the volunteer's
  app closing to every participant on the new forwarder: about 0.7 s. If no candidate's capacity
  covers the call, the call stays forwarded by the best participant. That is today's behaviour, so
  the call never stops for want of a volunteer.
- **A volunteer that stalls.** A volunteer that stays linked but drops frames, broken or hostile,
  is caught by its leaves. Each leaf watches its own receive and send times. One that has heard no
  audio through the volunteer for 5 s reports it to every device in the call
  (`CallMsg::ForwarderStalled { room, forwarder, spoke }`, appended), and again every 2 s while it
  still hears nothing. `spoke` says whether the leaf itself has been sending audio for that whole
  time, with no pause over 1 s. A report counts for 3 s. Once reports from two leaves (a majority
  of any forwarded call, which has at least seven) are fresh and at least one of them spoke, every
  device excludes that volunteer for the rest of the call and fails over as for a lapse. The
  speaker's report is the evidence: it was speaking while another leaf heard nothing, so the
  forwarder dropped it. A quiet call, where leaves hear nothing because nobody speaks, never adds
  up to a stall. The exclusion is kept per call, not per room, so it holds through the failover
  migration and any later one, and stall reports carry over into the new room. Measured on
  loopback, from the volunteer dropping everything to every participant on a participant
  forwarder: about 5.7 s, the 5 s window plus the failover. Known edge: a lone speaker in a call
  where everyone else is silent hears nothing, so a second leaf whose own link fails then looks
  like a stall, and the call moves to a participant; that costs a migration, not the call.
- **Re-election.** Today's `forwarder_since` sustain applies: a better forwarder must stay better
  for its sustain time before the call moves to it, so offers that come and go don't churn the call.
  A call forwarded by a participant moves to a volunteer that wins the election and holds that for
  `reelect_sustain`. A call forwarded by a volunteer keeps it while its offer holds; the capacity
  re-election does not replace a volunteer.

## SFrame end to end

Nothing changes in the cipher layer. A forwarder of any kind:

- holds no SFrame key in its relay, and is never added to the call's key. A volunteer's relay is a
  `Sfu`, which takes no key, beside a mesh that never links and holds a random key. A volunteer is
  a member, though, so it does hold the server's MLS epoch secret that a voice channel's key is
  exported from: a modified client could derive the key and decrypt what it forwards. That is what
  any member could hear by joining the call, but without being listed in it, so the call screen
  names the volunteer ("Forwarded by"). See `06-threat-model.md`. The seeder holds no epoch key at all;
- forwards frames unchanged, so the 9-byte stamp, the auth digests and the replay windows hold
  across it;
- sees what `04-calls.md` lists: levels, sizes, timing, the key id (sender index and epoch bits),
  and RTP headers.

The new forwarder kinds add exposure. A volunteer or the seeder learns who is in a call they're not
in, who speaks and when, and each participant's IP address or relay. It is in `06-threat-model.md`
as "A forwarder who is not in the call". The seeder already learns membership timing, but not call
presence.

## Layers per viewer

Built in phase 2, except where it says otherwise.

- **Simulcast.** A sender that is forwarded sends up to three layers: 180p, 360p, and the ladder's
  top rung within its boost ceiling (`12-boosts.md`). Layer 0 is the smallest. A layer is sent only
  while it is shorter than the top rung, at the top rung's aspect, so a camera on the 360p rung
  sends two layers and one on the bottom rung one (`call_video::simulcast_layers`). The capture
  thread runs the top rung on its main encoder, as before, and each lower layer on its own H.264
  encoder fed a box-scaled copy of the frame (`Frame::scaled_i420`). Each layer numbers its frames
  on its own. The lower layers cost about 150 and 400 kbps, which come off the uplink budget before
  the top rung is planned, so the CPU governor and the ladder see the whole cost.
- **AV1 is simulcast too, for now.** The top rung may be AV1, as before, but the lower layers are
  always H.264. SVT-AV1 has no real-time SVC mode in this build, and a second SVT encoder per layer
  costs too much CPU. A layer's codec rides in each frame's header, and a receiver keys its decoders
  by codec, so a viewer switched from an AV1 top layer to the H.264 base just changes decoders.
  L1T3 SVC can replace the lower AV1 layers once an encoder offers it and is measured.
- **Where the layer id goes.** In the carrier's clear `flags` byte, bits 2 and 3, beside the
  keyframe and screen bits (`video.rs`). The forwarder already reads that byte without a key, and
  it is outside the SFrame ciphertext, so no key is needed. The other two places were worse:
  - An SFrame header extension changes the SFrame layout, which every released receiver parses
    before it decrypts, so a released viewer would drop the frame.
  - An RTP header extension needs a negotiated extension id on every link, and the forwarder
    relays carrier NALs (`send_raw_video`), not RTP packets, so it would have to rebuild it per hop.

  A released reader looks at bits 0 and 1 only, so to it every frame is layer 0. Like the keyframe
  bit, the layer bits are not authenticated: a forwarder could mislabel layers, which only spoils
  the picture, as dropping frames already could.
- **Every camera that is on keeps flowing (decided).** The forwarder forwards every camera and screen,
  whether or not its sender speaks. Before this, video went the same way as audio, only for the top
  three speakers in the last second, so a camera froze for everyone about a second after its owner
  stopped talking or muted. A recent speaker, one of the three who spoke most recently within
  `SPEAKER_HOLD` (5 s, longer than a pause between sentences), gets the layer its tile needs. Every
  other camera gets the base layer. Audio keeps its top-three rule.
- **Per viewer.** A viewer tells its forwarder the width it draws each sender's camera and screen
  at (`TileWidths`, from `Engine::set_video_tile`). For a speaker's stream the forwarder gives it the
  smallest layer at least that wide, and the top layer for a tile wider than every layer. A stream it
  doesn't draw, and any stream of a sender who is not speaking, gets the base layer. The budget is the smaller of `Sfu::set_listener_budget` and the downlink's
  own bandwidth estimate less three speakers' audio. The estimate counts only once it falls below
  the 1000 kbps it starts at: nothing probes a link for headroom, so above that it means nothing
  (phase 1, "Capacity units"). "Pinned" and "thumbnail" preferences are not sent; the tile width
  covers both.
- **A tight budget (decided).** When a viewer's budget can't carry every stream at the layer its tile
  wants, the forwarder lowers the other streams first, one layer at a time, the step that saves most
  first, and lowers the active speaker (the loudest recent sender) only once every other stream is on
  its base layer. This is Discord's choice. The speaker stays sharp, and the rest turn into
  thumbnails.
- **Paused, not frozen (decided).** When even every base layer does not fit, the forwarder pauses
  streams for that viewer only: cameras before screens, the sender who spoke least recently first,
  and never the sharpest speaker's, so a viewer always sees at least one stream
  (`Sfu::paused_for`). It tells the viewer which streams are paused with `PausedVideo`, sent when
  the set changes, to a viewer that announced `FEATURE_PAUSED_VIDEO`. The viewer's call view marks
  them (`ParticipantView::video_paused`, `screen_paused`) and the window keeps the last frame with a
  "Paused to save bandwidth" badge. A paused stream restarts at a keyframe when the budget allows. A
  released viewer is paused the same way, but it is not told, so it keeps the last frame without the
  badge, as before. Only streams with a layer table can be paused, since only they have a known
  cost.
- **Switching.** A switch, up or down, waits for a keyframe of the new layer, and the forwarder asks
  the sender for one with PLI. Until then the viewer keeps getting its old layer, so the picture
  never stops and the decoder never sees two layers mixed.
- **Senders encode only what is watched.** On each membership tick (0.5 s) the forwarder sends each
  sender the mask of layers some viewer is on or is switching to (`WantedLayers`). The sender
  encodes those, plus always the base layer: a released viewer, a cascade child and a forwarder
  taken over on failover all get layer 0, so it must exist. The signalling runs beside the call
  loop, since it is RPCs and the loop carries audio.
- **Mesh stays single-layer.** Each mesh peer sends one stream sized for the weakest link, as today.
  A sender sends one stream until a forwarder asks it for layers.
- **Cascades get the base layer.** A cascade child is not a layer-aware viewer at the root, and a
  forwarder with no layer table for a stream sends its viewers layer 0 only. So viewers below a
  child get 180p from senders below another forwarder. Carrying tables and masks up and down the
  tree is left for later.

## Bandwidth

Using the spec's figures: 720p at about 1.5 Mbps, audio at about 32 kbps with the top three speakers.

| Call | Forwarder down | Forwarder up | Participant up | Participant down |
|---|---|---|---|---|
| 10 audio | 3 × 32 kbps | 9 × 3 × 32 ≈ 0.9 Mbps | 32 kbps | 96 kbps |
| 10, 3 cameras at 720p | 3 × 1.5 Mbps | 9 × 3 × 1.5 ≈ 40 Mbps | 1.5 Mbps (+ two small layers ≈ 2 Mbps) | 4.5 Mbps |
| 10, 3 cameras, viewers at 360p | 3 × 2 Mbps | 9 × 3 × 0.5 ≈ 14 Mbps | 2 Mbps | 1.5 Mbps |
| 25 audio | 3 × 32 kbps | 24 × 96 kbps ≈ 2.3 Mbps | 32 kbps | 96 kbps |

So a forwarder with 20 Mbps upload carries a 10-person call with three 720p cameras only when
viewers take a smaller layer, which is what per-viewer layers are for. A 25-person audio call
fits any home fibre. The election's capacity check uses these figures, and a forwarder that can't
cover the call caps the video layers it forwards, so it doesn't drop audio.

## What the boost mesh limits mean

`12-boosts.md` gives a mesh limit of 6, 6, 8 and 10 by level. It's the most devices a call keeps
as a mesh before it is forwarded:

- `TierPolicy` reads the call's server level. A mesh moves up at `mesh_limit + 1` devices, and a
  forwarded call moves back at `mesh_limit - 1`, so level 0 is today's 7 and 5.
- A bigger mesh only helps with good links, since each peer sends N-1 copies. So above 6 the limit
  applies only while every participant's announced upload covers N-1 copies of its stream at its
  layer. Otherwise the call is forwarded at 7, as at level 0. A boost raises the ceiling but never
  forces a mesh that the links can't carry.
- The level is per app (`12-boosts.md`), so two devices may briefly disagree on the limit. The
  tier machine already handles that: the migrating device signs the tier message, and a device
  whose own policy disagrees still follows a valid tier message for its roster. The staged-room swap
  keeps the call running.

## Compatibility with released clients

- **Tiers.** Released clients already run the mesh and the forwarded tier, with today's 7/5
  thresholds and signed tier messages. A call with only new clients may stay a mesh at 8, under a
  boost. With any released client in the roster, the call uses the level 0 limits, so a released
  client never sees a mesh larger than it expects. Call signalling has no version exchange today,
  and `JoinMsg` is a released struct, so it can't grow. A new appended variant `CallMsg::Features {
  room, bits }` goes to each peer on join. A released client fails to decode it and replies with an
  error, and that error is the signal that the peer is old. Until a peer answers, it counts as old.
- **Non-participant forwarders.** A released client can't decode `ForwardOffer`. A volunteer is not
  in the call, so it cannot read the call's `Features` before it offers: it sends the offer to every
  device it hears of, and a released one answers with a decode error, as it does for `Features`.
  An election that includes a non-participant is used only when every live device sent
  `FEATURE_VOLUNTEERS` (bit 4) in the last 30 s. Otherwise the forwarder is a participant, as
  today, and a volunteer already forwarding is replaced by one.
- **Layers.** A released receiver reads the layer bits as layer 0, and a forwarder sends it layer
  0 only, since it never reported tile widths: two layers would reach its decoder as one broken
  stream. A released sender sends one layer, and the forwarder passes it on to everyone. A released
  forwarder never asks for layers, so its senders send one stream.
- **Events.** The seeder setting is an appended control event in the carrier, so older clients
  show it as locked and ignore it.

## Phased plan

1. **Boost-aware tiers.** Built. `CallMsg::Features` is sent to each live device when it first
   appears and again with every capacity announcement (every 5 s). A refusal is retried on the next
   check, every 0.5 s. `effective_mesh_limit` applies the level's limit only while every live
   device has sent the feature bit in the last 30 s. It also needs every device's announced upload,
   or the election's default when it has announced none, to cover N-1 copies: 32 kbps of audio
   each, plus 1.5 Mbps when any camera or screen is on. Otherwise the call uses level 0's limit.
   **Capacity units.** The announced capacity is kbps of total upload, as the election has always
   read it, released clients included. It used to be measured as the mean per-link audio target
   (16 to 64 kbps), so every measured device failed the election's capacity filter. It is now the
   device's observed send rate summed over its links (`CallStats::sent_kbps`), and the audio
   target stays a separate value. On loopback, a mesh of seven measures about 415 kbps per device
   (six copies), a forwarder of seven about 1080, and a leaf about 70.

   That is a floor: what a device carries, not what it could. str0m's TWCC estimate stays at its
   1000 kbps starting value, because no call sets a desired bitrate, so nothing probes. So a
   forwarder is measured as able to forward, and a mesh device as able to carry its copies. But a
   leaf with spare upload measures only its own stream, and can't win a re-election on capacity.
   Measuring headroom needs probing: `Bwe::set_desired_bitrate` per link, which sends padding up
   to the desired rate. That costs upload on every link, so it is an open choice. Tests: a 7-device call stays a mesh at level 2 with good links, is
   forwarded with a weak link, and is forwarded at level 0 with a released client.
2. **Simulcast and per-viewer layers.** Built, as "Layers per viewer" describes. The layer id is in
   the carrier's clear flags. The forwarder gates incoming frames per layer and chooses per viewer
   (`Sfu::set_sender_layers`, `set_viewer_tile`, `set_layer_aware`, `wanted_layers`). The capture
   encodes the layers asked for (`call_video.rs`). The signalling is `call_layers.rs`: three
   appended `CallMsg` variants, sent only to a device whose `Features` has `FEATURE_VIDEO_LAYERS`:
   - `VideoLayers { room, screen, layers }`, sender to forwarder: its layers as (width, kbps).
   - `TileWidths { room, tiles }`, viewer to forwarder: its whole set of tile widths. Sending it
     makes the viewer layer-aware.
   - `WantedLayers { room, screen, mask }`, forwarder to sender: the layers to encode.

   Compatibility: no existing struct changed, and the variants are appended (the `wire_order`
   tests pin them). A sender stays on one stream until a layer-aware forwarder asks, so a released
   forwarder never sees more than one stream. A released sender announces no layers, and the
   forwarder passes its one stream to everyone. A viewer that never sent `TileWidths` (a released
   one) gets layer 0 only. Tests: unit tests of the layer choice (`sfu.rs`, `layer_tests`) and of
   the capture's layers (`call_video.rs`), in-memory forwarder tests (`tests/video_layers.rs`), and
   networked tests `simulcast_*` in `tests/it/calls.rs`: a narrow tile gets layer 0 while a wide one
   gets a higher layer; a constrained viewer keeps the speaker high and gets the others on the base
   while an unconstrained one gets all high; a released sender reaches everyone and a released
   viewer gets the base only.

   Left: AV1 SVC for the lower layers, layer tables through cascades, probing so the downlink
   estimate can rise above its start, and a CPU measurement of three encoders on a slow laptop.
3. **Volunteer forwarders.** Built, as "Who forwards" and "Offers and election" describe: the
   per-server toggle (`Engine::set_forward_calls`, kept in `forward_calls.bin` on the device), the
   signed `ForwardOffer`, the capped bonus, the volunteer's keyless relay, the "Forwarded by" label
   (`CallView::forwarded_by`), and the threat-model section. Signalling for layers reaches a
   volunteer forwarder too, so viewers still get the layer their tile needs. Tests: unit tests of the
   election with volunteers, the offer signature and the released decode (`calls/volunteers.rs`);
   networked tests in `tests/it/volunteers.rs`: a volunteer outside a call of seven is elected and
   carries its audio and video, at a high layer for a wide tile and the base for a narrow one, and is
   listed by nobody; when it closes its app, the call fails over to a participant (about 0.7 s on
   loopback, against the 600 ms the first plan aimed for) and keeps its audio; a banned member's and
   an outsider's offers are refused and the call stays with a participant; one released client in
   the call keeps the volunteer out though the new devices hold its offer; a volunteer that drops
   every packet (`test_support::drop_relayed`) keeps a quiet call, then is excluded once two members
   speak, the call fails over in about 5.7 s, keeps its audio, and doesn't elect it again though
   its offer is still live. UI driver section `volunteers`: the toggle and the label.

   A fix found on the way: a room swapped in by a migration started no presence clock, so after a
   migration the call's roster stopped being re-sent and lapsed for the server's other members
   (`call_presence_ttl`, 90 s). `swap_in` now starts one.

   Left: reading the power source and link type for the offer, measuring a volunteer's upload
   (it offers the election's default), the same stall check for a participant forwarder, and the
   seeder (phase 4).
4. **Seeder forwarding.** Built. The owner's "Seeder may forward calls" (Server Settings › Boosts,
   off by default) is `Payload::SeederForwardsCalls { on }`: owner only, the latest by HLC wins,
   appended after `ChannelOrder` and carried like it (`control::carry_sealed`), so an older client
   shows it as locked. While it is on, a seeder volunteers in that server as a member who turned on
   Help Forward Calls does, through the same offers, election, relay, failover and stall check.
   What differs, since a seeder is a storage member outside the MLS group:
   - **Its sender id.** It has no leaf index, so it takes the id `device_sid` gives its device. Each
     device in the call refuses its offer if a group member already has that id, and keeps the
     seeder's id beside the group's while its offer holds, through membership ticks and migrations.
   - **The leaves' ids.** It cannot read them from a group, so each leaf tells its volunteer its own
     sender id before it links (`CallMsg::RelayLeaf { room, gen, sid }`, appended). The seeder takes
     an id only from a current member, and refuses one another device already named. A member
     volunteer checks it against its group.
   - **Compatibility.** A seeder counts only while every device in the call sent
     `FEATURE_SEEDER_FORWARDERS` (bit 8), which says it sends `RelayLeaf`.
   - **Its name.** "Forwarded by" uses the name of its storage join, as every name lookup now does.
   - **Turning it off** makes every device's check fail, so the call fails over at once, and the
     seeder withdraws its offer.
   `apps/seeder` shows "forwards calls" per server in its status. Tests: a unit test of the setting's
   permissions (`relay_host_tests.rs`); the networked `volunteers::a_seeder_forwards_calls_only_while_the_owner_allows_it`:
   with the setting off, a seeder's offer is refused and no seeder forwards; the owner turns it on and
   the seeder carries a call of seven, audio and video at a viewer's layer, named "Forwarded by
   seeder"; the owner turns it off and the call fails over to a participant. UI driver checks for
   the owner's toggle.
5. **Volunteers in the cascade.** Live offers become cascade children and standbys before
   participants do. Tests: the loopback cascade test with two volunteers, and losing a volunteer
   child keeps everyone hearing.

## Open questions for the user

1. ~~Should volunteers be opt-in per server, or one switch for all your servers?~~ Decided: per
   server, off by default.
2. ~~Should the seeder forward at all?~~ Decided: only if the server's owner allows it (phase 4).
3. ~~Should a non-participant forwarder be named in the call, or kept anonymous?~~ Decided: named,
   "Forwarded by <name>".
4. Is it acceptable that a boosted call falls back to level 0 limits when one released client joins,
   or should boosts wait until most members have updated?
5. ~~For video, which matters more on a budget?~~ Decided: keep the active speaker sharp, and lower
   the others first.
6. ~~Should a relay host's app volunteer as a forwarder by default when they publish a relay?~~
   Decided: no, hosting a relay and forwarding stay separate opt-ins.
