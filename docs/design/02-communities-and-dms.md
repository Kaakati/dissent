# Design 02: multiple servers, channels, cross-server users, DMs

Shared contract for all agents. "Server" in the UI = "community" in code.

## User-facing behaviour

1. A user can **create** a server (they become its owner), which starts with a `general` channel.
2. The owner (and any member) can **create an invite link** `dissent://invite/...` and share it out of band.
3. A user can **join many servers** from invite links. A left rail lists them all; selecting one shows its channels and member list.
4. Admins can create more channels. Messages can be sent, edited (own), deleted (own, or mods).
5. Clicking any member in any server opens their **profile**: display name, their handle in each server you share, mutual servers, and a **Message** button.
6. **Direct messages** are 1:1, sent device-to-device over a direct iroh connection (QUIC/TLS end to end between devices; relays only see ciphertext). Undelivered DMs stay in an outbox and are retried.
7. Users who share no server can still DM by exchanging a **contact link** `dissent://contact/...`.

## Mentions

A message in a server channel can mention people. The text stays plain `@Name`. The list of who is
mentioned travels in the sealed payload, in a trailer extension tagged `mentions` (tag 2) with a
postcard body `Mentions { users: Vec<[u8; 32]>, everyone: bool }`. A reader that does not know the tag
skips it, and so does a reader whose body does not decode, so older clients show the message as text.

- **Who can be mentioned.** A user counts only if they are a member of the community and not banned,
  as the reader sees the community. Ids of other people are kept out of the message's views.
- **`@everyone`.** It counts only when the author held the *mention everyone* permission when the
  message was accepted. That is the owner and admins by default. The check reads the permission state
  at that point in the replay (`MessageView.mention_all`), so a later demotion does not change a
  message already accepted.
- **Counts.** A channel's unread mention count is the number of messages newer than its last
  mark-read that mention this device. Marking a channel read is local: it is stored on this device
  (`mention_reads.bin`) and nobody else sees it.
- **Alerts.** A mention alerts even in a muted channel, but not under Do Not Disturb. That rule is
  in the UI.
- **Direct and group messages.** They carry mentions the same way, in the same trailer extension of
  the sealed payload, so the wire format is unchanged and older clients show the text. A person counts
  only if they are a member of the conversation. There is no `@everyone`: the flag is dropped on send
  and ignored on arrival. Your own message is never highlighted for you. An edit is plain text, so a
  message keeps the mentions it was sent with. A conversation's unread mention count
  (`ConversationSummary.mentions`) is the number of its unread messages that mention this device, and
  marking it read clears it. A mention alerts through a muted DM or group, as in a channel. The
  capability is `dm_mentions`. See `crates/dissent-core/src/dm_mentions.rs`. Files sent to a DM or
  group do not carry mentions yet (`send_dm_files` takes none).

## Server settings

The owner opens **Server Settings** from the server menu. It is a full-page view with a left nav:
Server Profile, Privacy and Boosts, then People (Members, Invites, Bans). There is no Roles page and no Delete Server,
because the engine has neither yet. Members who are not the owner do not see the entry.

Server Profile is a form beside a live preview card. The card is the same component the invite card
uses. The name, banner, description and traits wait in an unsaved bar until **Save Changes**, which
publishes one event for each part that changed. Leaving with unsaved changes shakes the bar instead
of closing. The icon applies at once, through the existing icon flow.

- **Name.** `Payload::CommunityRename { name }`, owner only. The name must already be trimmed, must not
  be empty, and may be at most 64 characters (`MAX_COMMUNITY_NAME_CHARS`, the create dialog's limit).
  Replay rejects anything else as `Malformed`. The rename with the highest HLC wins, as for the icon,
  so concurrent renames converge. Until a rename is accepted, the genesis name stands, and the
  genesis itself never changes. `Community::name()` returns the current name, so the views, the DHT
  record and the member's own invite preview all follow it.
- **Banner.** `Payload::CommunityBannerSet { banner: Option<Banner> }`, owner and admin, like the icon.
  `Banner::Preset(id)` names one of ten gradients (`BANNER_PRESETS`; white, pink, red, orange, yellow,
  purple, blue, teal, green, dark), which the UI draws (`ui-next/src/lib/banners.ts`).
  `Banner::Image(Picture)` is a 16:9 WebP or PNG, at most 960 by 540 and 512 KB (`MAX_BANNER_BYTES`),
  cropped in the avatar editor's banner shape. It is stored, fetched and decrypted into `avatars/`
  like the icon. The views carry `banner` (the file) and `banner_preset`. The sidebar shows the banner
  at the top of the channel column, with the name over it, and the banner collapses into the plain
  header as the channel list scrolls.
- **Profile.** `Payload::CommunityProfileSet { description, traits }`, owner and admin. The description
  is trimmed and at most 300 characters. There are at most five traits. Each trait is a trimmed label
  of 1 to 24 characters with an optional emoji (`state::valid_profile`). An emoji is exactly one
  extended grapheme cluster of at most 32 scalars (`normalize_trait_emoji`), so skin tones, ZWJ
  sequences, flags and keycaps pass, and two emoji do not.
  Replay **rejects** a profile over any cap as `Malformed`, rather than trimming it, so every peer
  shows the same profile or none. The engine refuses it before sending.
- **Compatibility.** The three payloads are appended to `Payload`. They are sealed like every other
  control payload: under the community secret, or under the epoch key with member-only control on.
  So the outer event is the `SealedControl` or carrier form that older clients decode. A released
  client that holds the key cannot decode the inner kind, and rejects that one event as `Forged`.
  This is silent, and sync does not stall, so **older clients ignore these changes**. They keep the
  genesis name and show no banner or profile. The carrier is not used for these payloads, since it is
  for new outer variants. A rename stays under the community secret even with member-only control on,
  like invite events, because the invite preview hands it to a joiner as proof of the name.
- **DHT record.** After a rename, the founder's device republishes the community record with the new
  name. It does this whether it made the rename or received it.
- **Invite preview.** The released `SignedPreview` keeps the genesis name, which older clients check
  against the genesis. Newer clients ask `Request::InvitePreviewFull` first. It answers with the same
  preview plus a `PreviewExtra { rename, banner, profile }`, signed by the same device. The joiner
  opens the rename with the invite secret and checks that the genesis owner signed it. If both hold,
  the card shows the new name. If either fails, the card shows the genesis name. A peer that cannot
  decode the new request answers with an error, and the joiner falls back to `InvitePreview`. The
  card shows the banner, description and traits, and "Est. <month>" from the genesis date.
- **Limits.** An owner who rotated to a new root cannot prove a rename this way, so invite cards show
  the genesis name until the joiner is in. The banner and profile are not verified beyond the
  answering member's signature, as icons are not. Once joined, the DAG decides.

## Channel order

Owners and admins drag channels in the sidebar to reorder them. The order belongs to the whole server, so it is a
signed, replicated control event, unlike the server rail's order, which is each person's own (`set_server_order`).
There are no categories, so a server has two lists, text channels and voice channels. A channel moves only within
its own kind.

- **Payload.** `Payload::ChannelOrder { text: Vec<ChannelId>, voice: Vec<ChannelId> }`, both top first, appended to
  `Payload`. It is on the control channel and comes from the owner or an admin: whoever may create, change and delete
  channels. Replay rejects it from anyone else as `NoPermission`. It rejects it as `Malformed` off the control channel
  or with more than `MAX_ORDERED_CHANNELS` (500) ids in all. The order with the highest HLC wins, so concurrent
  reorders converge.
- **Resolving it** (`CommunityState::channel_order`). Each list keeps the ids that are live channels of that kind, in
  the order given, and skips the rest: unknown or deleted ids, a voice id in the text list or the reverse, and
  repeats. These don't reject the event, since a channel can be deleted while an order is in flight. The channels the
  list leaves out follow in creation order, which is the create event's `(HLC, id)`. So a channel made after the
  last reorder goes at the end of its kind. With no order at all, both lists are in creation order. Before this,
  views listed channels by id, which looked random. `community_view` lists the text channels, then the voice
  channels, each in this order.
- **Compatibility.** The payload is sealed like other control, then wrapped in the carrier (`carry_sealed`, kind
  `CARRIED_SEALED`), as a private channel's access events are. With member-only control on, it uses the member-sealed
  carrier instead. A client from before the carrier finds no key for `CARRIER_GENERATION` and shows the event as
  locked. A client that knows the carrier but not this kind fails to decode the inner payload and rejects that one
  event as `Forged`. Either way, sync doesn't stall, and **older clients keep their own order**, which is by id.
- **Engine.** `Engine::reorder_channels(community, text, voice)` checks the role and that every id is a channel of
  that kind, listed once, then publishes. The desktop command is `reorder_channels { communityId, text, voice }`,
  behind the `channel_order` capability.
- **UI** (`components/shell/channel-reorder.tsx`). Rows drag with pointer events, as the rail's do. A line shows where
  the channel will land. Alt+Shift+Up and Down move the focused channel, the same keys as the rail, since Alt+Up and
  Down already switch channels. The new position is announced. The channel menu has Move Up and Move Down. The order
  applies at once. If publishing fails, it goes back and a toast says so. People who can't manage channels get no
  drag, no keys and no menu items. A muted channel hidden by Hide Muted Channels keeps its place among the others.

## Channel categories

Owners and admins group channels into categories and arrange them as they like. A category can hold text and
voice channels together, and channels can also sit outside any category. This builds on the channel order above.

- **Payload.** `Payload::ChannelLayout { entries: Vec<LayoutEntry> }`, appended after `HistorySupport`, holds the
  whole layout, top first. `LayoutEntry` is `Channel(ChannelId)` (outside any category) or `Category(LayoutCategory
  { id: [u8; 16], name, collapsed, channels })`, where `channels` is top first and may mix kinds. `collapsed` is how
  the category starts for someone who never opened or closed it. The latest layout by HLC wins, and it replaces
  the one before it as a whole. `LayoutEntry` is a new enum, so its variant order is pinned in the wire-order test.
- **Category ids.** A category id is 16 random bytes, made by the engine the first time the category is placed.
  The layout is one event that replaces the last, so there is no event that "creates" a category whose id could
  be borrowed. A random id stays the same across layouts, so a rename or a move keeps the category, and each
  person's open or closed state follows it. Collisions are negligible at 128 bits, and a layout with two
  categories of the same id is rejected anyway.
- **Rules** (`check`). The event must be on the control channel, from someone `CommunityState::may_manage_channels`
  allows: the owner and admins today. That function is the hook for a custom role with a "manage channels"
  permission later. `state::valid_layout` caps it at 50 categories with distinct ids, each named with a trimmed,
  non-empty name of at most 64 characters, and 500 channel ids in all. A layout over a cap is rejected as
  `Malformed` as a whole, and from anyone else as `NoPermission`.
- **Resolving it** (`CommunityState::channel_layout`). Each channel shows once, at its first place. An id that is
  not a live channel, or that comes again, is skipped without rejecting the layout. Categories keep their place
  even when empty. Live channels the layout leaves out, such as one made after it, follow at the end, outside any
  category, in creation order. Without a layout the function returns `None`, and `ChannelOrder` decides as before.
  While a layout exists it supersedes `ChannelOrder`, so an order sent later by a client from before layouts
  changes nothing on current clients.
- **Older clients.** The payload is carried (`carry_sealed`), so a client from before the carrier shows it as
  locked, and one that knows the carrier but not this kind rejects it as `Forged`. Sync does not stall. With each
  layout, the engine also publishes the `ChannelOrder` it amounts to: its channels top to bottom, text and voice
  apart. So a client that knows `ChannelOrder` keeps roughly the same order, without the categories.
- **Engine.** `Engine::set_channel_layout(community, entries)` checks the role, that every channel is live and
  placed once, and the caps. It gives a new category its id, then publishes the layout and the derived order.
  `community_view` carries `layout` (or `None`) and `can_manage_channels`, and lists `channels` in layout order.
  The layout in the view is the viewer's own: channels they cannot read are left out, and so is a category whose
  channels are all hidden from them. An empty category shows only to people who may arrange channels, so they
  can fill it. The desktop command is `set_channel_layout { communityId, entries }`, where an entry is
  `{ kind: "channel", id }` or `{ kind: "category", id?, name, collapsed, channels }`. It is behind the
  `channel_categories` capability.
- **UI** (`lib/channel-layout.ts` for the moves, `components/shell/channel-tree.tsx` for the sidebar).
  - **Without a layout.** The sidebar shows the two groups it always had, Text channels and Voice channels. The
    first arrangement turns them into real categories with those names.
  - **Headers.** A header opens and closes its category. Each person's state is kept on their device
    (`localStorage`, per server and category). A closed category still shows the channel that is open.
  - **Dragging.** People who may arrange channels drag a channel within a category, into one (drop on its
    header, which highlights), out of one, or between channels outside categories. The upper part of a header
    drops above the category instead. A category drags by its header. A line shows where an entry will land.
  - **Keyboard.** Alt+Shift+Up and Down move the focused channel one step through every place it can be: within
    its category, out of it at either end, and into the category next to it. On a header, they move the
    category. Each move is announced.
  - **Menus.** The channel menu has Move Up, Move Down and Move to Category (each category, or No Category). The
    header menu has Create Channel, Create Category, Rename Category, Move Up, Move Down and Delete Category.
    Deleting asks first, and leaves the channels outside any category, where the category was. The server menu
    has Create category.
  - **Creating inside a category.** A category's + makes the new channel at the end of that category.
  - **Applying.** Each change applies at once and is published as a whole layout. If the publish fails, the old
    layout comes back with a toast.
  - **Others.** People who cannot arrange channels get no drag, no keys and no menus, and can still open and
    close categories.

## Animated banners

A GIF with more than one frame stays animated, as a server banner or a personal one. Avatars and
icons stay still.

- **Editor.** The crop is shown on the first frame as usual, but Apply doesn't draw it on a canvas,
  which would keep one frame. It sends the original bytes and the crop rect to the engine
  (`prepare_animated_banner`). The rect is in source pixels of the picture after its quarter turns.
  The editor shows a spinner until the engine answers.
- **Engine** (`animated_banner.rs`, in a blocking task):
  - It decodes every frame with its delay, rotates, crops, and scales each frame to at most 960 by
    540, keeping 16:9.
  - It writes a GIF that loops forever (`NETSCAPE2.0`, loop count 0). The output holds only frames
    and the loop block, so comments and other metadata in the source are dropped.
  - It writes a still JPEG of the first frame as the poster.
  - Limits: at most 200 frames and 2 MB. Over that, it first drops every other frame, doubling
    the delays, then lowers the width in steps (960, 720, 540, 400, 320). If it still doesn't fit,
    it refuses with a clear error.
- **Payload.** `Banner::Animated { image, poster }` is appended to `Banner`. The GIF may be up to
  2 MB (`MAX_ANIMATED_BANNER_BYTES`), and the poster is held to the still banner cap. Still images
  keep 512 KB. An older client can't decode the new variant, so it rejects that banner event and
  shows no banner, silently.
- **Showing it.** The views carry `banner` (the GIF) and `banner_poster`. The UI shows the poster
  under `prefers-reduced-motion: reduce`, and while the window doesn't have focus, so a banner
  nobody watches doesn't animate.

## Profile banners

A person sets a banner in Settings › My identity, under the avatar. It uses the same presets, the
same 16:9 crop and the same caps as a server banner: at most 960 by 540 and 512 KB. A pick is
published at once. It shows behind the profile card, the DM panel and the full profile. With no
banner, those keep the person's hue.

- **Payload.** `Payload::ProfileBannerSet { banner: Option<Banner> }`, appended and gen-sealed like
  `ProfileUpdate`, published to every community the person is in. Only the author sets their own,
  since replay keys it by author. Replay rejects an unknown preset or an oversized picture as
  `Malformed`, and the highest HLC wins. `ProfileUpdate` and the direct `SignedProfile` are unchanged.
- **Contacts.** `Request::ProfileBanner(SignedBanner)` is an appended request, signed by the device
  under its own domain. It goes directly to contact devices, as `Request::Profile` does for the
  picture. It is accepted only from a contact who isn't blocked, and the newest HLC wins. A device
  from before banners cannot decode it and answers with an error.
- **Storage.** A picture is stored, fetched and decrypted into `<profile>/avatars` like an avatar.
  This device's own banner and the ones contacts sent directly are kept sealed in `banners.bin`, which
  a profile copy re-seals with the other sealed files. The views carry `banner` (the file) and
  `banner_preset` on `MeView` and `UserView`. The newest banner from any community or the direct
  path wins.
- **Compatibility.** Older clients ignore it: they reject the sealed payload as forged, silently,
  and answer the direct request with an error.

## Addresses

Users, servers and devices have a short, checkable form to read out or paste, in the style of Bitcoin addresses:

| Kind | Prefix | Example |
|---|---|---|
| user (`RootId`) | `dsu1` | `dsu1qx7f…` |
| server (`CommunityId`) | `dss1` | `dss1…` |
| device (`DeviceId`) | `dsd1` | `dsd1…` |

- The format is bech32m (BIP-350). The payload is the full 32-byte public key, so nothing is hashed or truncated, and the checksum catches typos. An address is about 62 characters.
- Screens show the short form (first 8 and last 6 characters, `dsu1qx7f…k3f9q2`). The full address is in the tooltip and is what "Copy user address" and "Copy server address" put on the clipboard. My identity and the full profile also show a QR code.
- Addresses are a display and input form only. State, storage, keys, contact and invite links, and the wire keep the lowercase base32 form (`Display`). `FromStr` accepts both forms, so a pasted address converts to base32 before it reaches the backend.
- Adding by address works while the person allows it. Their user record is published on the Mainline DHT under their root key (`dissent-discovery`, `user_record.rs`), listing their device certs, which are verified against the address. The "Let people add me by address" switch (on by default) withdraws the record when off. Contact links keep working either way.
- The shareable contact link is the short form, `dissent://contact/<address>`, while the record is published. The long card form (`dissent://contact/<base32>`) still parses and works, and is what is shared while the switch is off.
- Only a device that holds the root key publishes the record. See `06-threat-model.md` for what publishing reveals.
- Code: `crates/dissent-identity/src/address.rs` (encode, decode, prefixes), `apps/desktop/ui-next/src/mock/address.js` (the same encoding in JavaScript; `apps/desktop/ui-next/src/mock/address.test.js` checks the two agree on fixed keys).

## Already implemented (do not change unless your task says so)

- `dissent-identity`: see CLAUDE.md. Plus `Genesis { community, owner, name, created_ms, sig }` with `Genesis::create(&CommunityKey, owner, name, created_ms)` and `.verify()`. This binds the owner to the community key; invites and sync carry it.
- `dissent-events`: `Event`, `EventBody`, `Payload`, `Dag`, `replay(owner, &dag) -> CommunityState`, `HlcClock`.
- `dissent-net`: `Node::{spawn, id, addr, remember, ping, subscribe, shutdown}`, `PeerAddr` (Display/FromStr), `TopicId::for_channel`.
- `dissent-discovery`: `Invite`, `CommunityRecord`, `Discovery::{publish, resolve}`.

## Additions per crate

### dissent-events (agent: events2)

```rust
// event.rs
Payload::Join { display_name: String }   // CONTROL channel; re-sending updates the name
Payload::Leave                           // CONTROL channel
// state.rs
pub struct Member { pub display_name: String, pub joined: Hlc, pub left: bool }
CommunityState.members: BTreeMap<RootId, Member>   // owner is NOT implicit; creator emits Join like everyone
MessageView.edited: bool
// Rules: Join/Leave allowed for anyone not banned (Leave sets left=true; a later Join clears it).
// Messages do NOT require membership (avoids a new ordering race).

// wire.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvent { pub event: Event, pub cert: DeviceCert }
impl SignedEvent { pub fn verify(&self) -> Result<()>; pub fn encode(&self) -> Vec<u8>; pub fn decode(b: &[u8]) -> Result<Self>; }

// dm.rs
pub struct DmId(pub [u8; 32]);            // base32 Display/FromStr like EventId
pub struct DmBody { pub from: RootId, pub to: RootId, pub device: DeviceId, pub hlc: Hlc, pub text: String }
pub struct DmMessage { pub body: DmBody, pub id: DmId, pub sig: Signature }
impl DmMessage { pub fn sign(body: DmBody, device: &DeviceKey) -> Self; pub fn verify(&self, cert: &DeviceCert) -> Result<()>; }
pub struct SignedDm { pub dm: DmMessage, pub cert: DeviceCert }  // verify/encode/decode like SignedEvent
// domains: id = blake3(b"dissent/dm/v1" ++ postcard(body)), sig over b"dissent/dm-sig/v1" ++ id
```

### dissent-net (agent: net2)

Generic request/response on ALPN `b"dissent/rpc/0"`, one bi-stream per request, `u32` big-endian length prefix, max 16 MiB each way.

```rust
pub trait RpcHandler: Send + Sync + 'static {
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> futures_lite::future::Boxed<Vec<u8>>;
}
NodeConfig.rpc: Option<Arc<dyn RpcHandler>>          // NodeConfig stays Clone; implement Debug manually if needed
impl Node { pub async fn request(&self, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>>; } // dials by id (uses remembered addrs / iroh discovery); 15 s timeout
```

### dissent-discovery (agent: discovery2)

```rust
pub struct Invite { pub genesis: Genesis, pub peers: Vec<String>, pub secret: Option<[u8; 16]> }  // replaces community+name; Invite::parse verifies genesis
impl Invite { pub fn community(&self) -> CommunityId; pub fn name(&self) -> &str; }
pub struct ContactCard { pub cert: DeviceCert, pub display_name: String, pub peers: Vec<String> }
impl ContactCard { pub fn to_url(&self) -> Url /* dissent://contact/<base32 postcard> */; pub fn parse(s: &str) -> Result<Self> /* verifies cert */; }
```

### dissent-core (new crate, agent: core) — the engine, no tauri dependency

RPC wire (postcard):
```rust
enum Request {
    Genesis { community: CommunityId },                                   // joiner checks it against the invite
    Reconcile { community: CommunityId, msg: Vec<u8>, from: PeerAddr },   // one negentropy round; responder answers from its own events
    Fetch { community: CommunityId, ids: Vec<EventId> },                  // up to 500 ids; reply holds the ones it has, causal order, capped at 4 MiB
    Push { community: CommunityId, events: Vec<SignedEvent> },            // initiator's events the responder lacks; verified and stored like gossip
    Addr(Box<SignedAddr>),                                                // a device's signed address, sent on start and on change
    Dm { dm: Box<SignedDm>, from: PeerAddr },
}
enum Response { Genesis(Genesis), Reconcile { msg: Option<Vec<u8>> }, Events(Vec<SignedEvent>), Ack, Error(String) }
```
Set reconciliation uses the `negentropy` crate. Items are `(hlc.wall_ms, event id)`, frame limit 256 KiB. The initiator
runs rounds until done, learning `need` (held by the peer, not by us) and `have` (held by us, not by the peer). It then
calls `Fetch` for `need` and `Push` for `have`. Used for joins, gap fills after a pending event, and anti-entropy every 30 s.

Gossip: one topic per community: `TopicId::for_channel(&community, &ChannelId::CONTROL.0)`, payload = postcard `GossipMsg`:
`Event(SignedEvent)` or `Addr(SignedAddr)`. `SignedAddr` is signed by the device key over `b"dissent/addr/v1" || postcard((addr, ts_ms))`.
Receivers keep the newest address per device and persist it in `addresses.bin`.
// chisle: spec says one topic per channel; per-community keeps subscriptions bounded for users in many servers.

Every received event: `verify()` (cert.root == author, sig) → store cert → `Dag::insert` → if `Pending { missing }`, run a sync with the sender → `Replayer::on_insert` → emit `EngineEvent`. Events stamped more than 5 minutes ahead of the local clock are dropped.

Disk layout under `data_dir`:
```
profile.bin                         postcard { display_name }
contacts.bin                        postcard Vec<ContactCard>
addresses.bin                       postcard Vec<StoredAddr { addr: PeerAddr, ts_ms }>, one per device. ts_ms 0 = unsigned
                                    (invite, contact, DM sender); otherwise the time the device signed the address.
                                    A signed entry is never replaced by an unsigned one, and only a newer signed one replaces it.
own_addr.bin                        postcard SignedAddr, our latest announced address. Its ts_ms keeps announcements
                                    increasing across restarts.
communities/<id>/genesis.bin
communities/<id>/community.key      founder only, sealed under the vault (32-byte seed inside; see 06-threat-model finding 7)
communities/<id>/events.log         repeated [u32 BE len][SignedEvent postcard]; replayed on start; appended without a sync, synced at most every 250 ms (06, durability)
dms/<peer-root-id>.log              repeated [u32 BE len][SignedDm postcard]
dms/outbox.bin                      postcard Vec<SignedDm> not yet acked
```

Public API (all `async` methods return `Result<_, CoreError>`; views are `Serialize` with snake_case fields and ids as base32 strings):

```rust
pub struct Engine;  // Clone
Engine::start(identity: Identity, data_dir: PathBuf, net: NodeConfig) -> Result<(Engine, tokio::sync::broadcast::Receiver<EngineEvent>)>
fn subscribe(&self) -> broadcast::Receiver<EngineEvent>
fn me(&self) -> MeView { root_id, device_id, suffix, display_name }
async fn set_display_name(&self, name: String)          // persists; re-sends Join in every community
fn contact_link(&self) -> String
async fn add_contact(&self, link: &str) -> UserView

fn communities(&self) -> Vec<CommunitySummary { id, name, owner, my_role }>     // my_role: "owner"|"admin"|"moderator"|"member"
async fn create_community(&self, name: String) -> CommunitySummary              // CommunityKey + Genesis + Join + ChannelCreate "general"; publish DHT record best-effort
async fn create_invite(&self, community: CommunityId) -> String                  // Invite{genesis, peers:[my addr], secret:None}.to_url()
async fn join_community(&self, invite: &str) -> CommunitySummary                 // remember invite peers, Sync with each, subscribe gossip, emit Join
async fn leave_community(&self, community: CommunityId)                          // emits Leave, unsubscribes, deletes local dir
fn community_view(&self, community: CommunityId) -> CommunityView {
    id, name, owner, my_role,
    channels: Vec<ChannelInfo { id, name }>,
    members: Vec<MemberInfo { root_id, suffix, display_name, handle: Option<String>, role, banned }>,
}
async fn create_channel(&self, community, name: String) -> ChannelInfo
fn channel_messages(&self, community, channel: ChannelId, limit: usize) -> Vec<MessageItem {
    id, author, author_name, text, ms, edited, deleted }>                      // newest `limit`, oldest first
async fn send_message(&self, community, channel, text: String) -> MessageItem
async fn edit_message(&self, community, channel, message: EventId, text: String)
async fn delete_message(&self, community, channel, message: EventId)
async fn moderate(&self, community, action: ModAction, user: RootId)           // ModAction: Ban|Unban|GrantAdmin|RevokeAdmin|GrantModerator|RevokeModerator
async fn claim_handle(&self, community, handle: String)
// Each mutating call returns CoreError::Rejected(reason) if replay rejected the new event.

fn user_profile(&self, user: RootId) -> UserView {
    root_id, suffix, display_name, is_contact,
    handles: Vec<{ community_id, community_name, handle }>,
    mutual_communities: Vec<{ id, name }> }
fn conversations(&self) -> Vec<ConversationSummary { peer, display_name, last_text, last_ms }>   // newest first
fn dm_messages(&self, peer: RootId, limit: usize) -> Vec<DmItem { id, from, text, ms, pending }>
async fn send_dm(&self, peer: RootId, text: String) -> DmItem                  // deliver to every known device of peer; else outbox (retry every 30 s)
async fn shutdown(self)

#[derive(Clone, Serialize)] #[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    CommunitiesChanged,
    CommunityUpdated { community_id: String },
    Message { community_id: String, channel_id: String },
    Dm { peer: String },
}
```

Known devices of a user = devices from certs seen in any community, DMs or contact cards.
Display name of a user = latest Join display_name in any shared community, else contact card name, else `anon·<suffix>`.

### apps/desktop (agents: ui, wire)

Tauri commands = one per Engine method, same names: `me`, `set_display_name`, `contact_link`, `add_contact`, `list_communities`, `create_community`, `create_invite`, `join_community`, `leave_community`, `community_view`, `create_channel`, `channel_messages`, `send_message`, `edit_message`, `delete_message`, `moderate`, `claim_handle`, `user_profile`, `list_conversations`, `dm_messages`, `send_dm`, plus existing `identity_status`, `identity_export_phrase`, `identity_import_phrase`, `app_version`.
JS passes args camelCase (Tauri default): `communityId`, `channelId`, `messageId`, `user`, `peer`, `text`, `name`, `link`, `invite`, `limit`, `handle`, `action` (`"ban"|"unban"|"grant_admin"|"revoke_admin"|"grant_moderator"|"revoke_moderator"`). Ids are base32 strings.
Backend forwards every `EngineEvent` to the window as Tauri event `"dissent://event"` with the JSON above.
