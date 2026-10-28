# Design 13: forwarded calls beyond the mesh

Status: design only. Nothing here is built unless it says so. It extends `04-calls.md`, whose
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
- **Who forwards.** Only a participant. `elect_forwarder` ranks the call's own devices by announced
  upload capacity, then the spec's rules (not on battery, wired, CPU ok, central, lowest id).
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
2. **A volunteer member.** A member who opted in under Settings › Voice ("Forward calls for my
   servers when I'm idle"), whose device is online, on mains power and wired, but not in the call.
   They join the call's room as a forwarder only: no audio and no video, never listed as in the
   call, shown only as "forwarded by <name>" in the call's route line.
3. **The seeder.** An always-on member (`apps/seeder`). It is often the best-connected device a
   server has. A seeder is a storage member. It is outside the MLS group and holds no epoch key, so
   it can't read media, and forwarding needs none. It needs a call role it doesn't have today:
   `JoinAsStorage` members may only join and leave (state.rs). So the seeder forwards only if the
   owner turns on "Seeder may forward calls" (an appended, admin-only control event, off by default).

A **relay host** (`12-boosts.md`) is not a forwarder kind of its own. The relay binary carries
packets, not calls, and making it a media forwarder would put a str0m stack in `apps/relay`. Instead,
a host who also runs the app counts as a volunteer member. Hosting a relay is shown as a tie-break
hint in the election, since their device likely has upload.

**Who may forward is checked on every device**, from the community state it already has, before it
links to a forwarder:

- the device belongs to a current, non-banned member, or to the seeder while its setting is on;
- for a non-participant, the member has an active "forwarder offer" for this call (below).

A device that fails the check is never linked to, whatever a tier message says.

## Offers and election

- **Forwarder offer.** A non-participant offers itself with a signed, short-lived message on the
  call's existing signalling path (`CallMsg`, appended variant `ForwardOffer { room, capacity_kbps,
  wired, on_battery, expires_ms }`). It is signed by its device key and dropped after 60 s unless
  renewed. Participants already announce capacities the same way.
- **One candidate set.** Every device ranks the same set: participants plus live offers. It uses the
  same `elect` as today, so all devices agree without a vote. A non-participant gets a small bonus
  in the ranking, because it isn't spending upload on its own media. The bonus is capped, so a
  strong participant still wins over a weak volunteer.
- **Replacement.** The pool of four forwarder links stays. A volunteer or seeder that leaves, sleeps
  or stops renewing its offer drops out of the candidate set, and the pool fails over as today. If
  no candidate's capacity covers the call, the call stays forwarded by the best participant. That
  is today's behaviour, so the call never stops for want of a volunteer.
- **Re-election.** Today's `forwarder_since` sustain applies: a better forwarder must stay better
  for its sustain time before the call moves to it, so offers that come and go don't churn the call.

## SFrame end to end

Nothing changes in the cipher layer. A forwarder of any kind:

- holds no SFrame key and no MLS epoch key, and is never added to the call's key;
- forwards frames unchanged, so the 9-byte stamp, the auth digests and the replay windows hold
  across it;
- sees what `04-calls.md` lists: levels, sizes, timing, the key id (sender index and epoch bits),
  and RTP headers.

The new forwarder kinds add exposure. A volunteer or the seeder learns who is in a call they're not
in, who speaks and when, and each participant's IP address or relay. That has to go into
`06-threat-model.md` before phase 2, as "A forwarder who is not in the call". The seeder already
learns membership timing, but not call presence.

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
- **Per viewer.** A viewer tells its forwarder the width it draws each sender's camera and screen
  at (`TileWidths`, from `Engine::set_video_tile`). The forwarder gives it the smallest layer at
  least that wide, and the top layer for a tile wider than every layer. A stream it doesn't draw
  gets the base layer. The budget is the smaller of `Sfu::set_listener_budget` and the downlink's
  own bandwidth estimate less three speakers' audio. The estimate counts only once it falls below
  the 1000 kbps it starts at: nothing probes a link for headroom, so above that it means nothing
  (phase 1, "Capacity units"). "Pinned" and "thumbnail" preferences are not sent; the tile width
  covers both.
- **A tight budget (decided).** When a viewer's budget can't carry every stream at the layer its tile
  wants, the forwarder lowers the other streams first, one layer at a time, the step that saves most
  first, and lowers the active speaker (the loudest recent sender) only once every other stream is on
  its base layer. This is Discord's choice. The speaker stays sharp, and the rest turn into
  thumbnails. The base layer is the floor: a budget too small for every base layer still gets them.
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
- **Non-participant forwarders.** A released client can't decode `ForwardOffer`. So offers go only
  to devices that announced the feature bit, and an
  election that includes a non-participant is used only when every participant supports it.
  Otherwise the forwarder is a participant, as today.
- **Layers.** A released receiver reads the layer bits as layer 0, and a forwarder sends it layer
  0 only, since it never reported tile widths: two layers would reach its decoder as one broken
  stream. A released sender sends one layer, and the forwarder passes it on to everyone. A released
  forwarder never asks for layers, so its senders send one stream.
- **Events.** The seeder setting is an appended, gen-sealed control event, so older clients reject it
  as forged and ignore it.

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
3. **Volunteer forwarders.** The settings toggle, `ForwardOffer`, the candidate bonus, the "forwarded
   by" line, the threat-model section. Tests: a volunteer that is not in the call forwards a
   10-person call with no plaintext seen. Its loss fails over to a participant within 600 ms. A
   banned member's offer is refused.
4. **Seeder forwarding.** The owner setting, a forwarder-only role for the seeder in `apps/seeder`,
   and the replay rule that allows it. Tests: the seeder forwards only while the setting is on.
5. **Volunteers in the cascade.** Live offers become cascade children and standbys before
   participants do. Tests: the loopback cascade test with two volunteers, and losing a volunteer
   child keeps everyone hearing.

## Open questions for the user

1. Should volunteers be opt-in per server, or one switch for all your servers?
2. Should the seeder forward at all, given it then learns who is in each call and who speaks? Or
   should forwarding stay with people only?
3. Should a non-participant forwarder be named in the call ("forwarded by Ada"), or kept anonymous
   in the UI? It's visible to the call's members either way, through signalling.
4. Is it acceptable that a boosted call falls back to level 0 limits when one released client joins,
   or should boosts wait until most members have updated?
5. ~~For video, which matters more on a budget?~~ Decided: keep the active speaker sharp, and lower
   the others first.
6. Should a relay host's app volunteer as a forwarder by default when they publish a relay, or stay
   a separate opt-in?
