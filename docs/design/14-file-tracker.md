# Design 14: server file tracker

Status: design reconciled with the user's decisions (below). Built phase by phase; each phase's state is in
"Build plan". Derived from the tracker artboards (TrackerBrowse, TrackerFile, TrackerSeeding, TrackerCategories,
TrackerAdd), the earlier FilesServer artboard, and the code.

A server gets a files area: a catalog of files that members list in categories, and a swarm that keeps them
available. Members download a file from every member who holds it, in parallel, and seed it back. A category
can be linked to a folder on the lister's computer, so its files are listed and seeded from where they are,
without a copy. Members earn reputation from signed receipts for what they deliver.

It builds on `08-files.md` (encrypted blobs, iroh-blobs, `MirrorPolicy`), `05-encrypted-servers.md` (the
carrier, member-only control, storage members), `11-private-channels.md` (keys per generation, `ChannelKeys`),
`12-boosts.md` and `13-forwarded-calls.md` (members that volunteer bandwidth, and the seeder's role).

## Decisions

Made by the user on 2026-10-29.

1. **One server key.** File content is encrypted under the server's files key, per generation, not under a random
   per-file key. Same plaintext under the same generation gives the same ciphertext (see "Encryption"), which is
   what dedupes. The leakage of that choice is recorded below.
2. **The whole catalog for every member.** A new member reads every listing, whatever the history setting.
3. **Custom roles and bots first** (phase 0): permissions per role, roles assignable to members, bots as
   identities that hold roles.
4. **Receipts do not name the file.**
5. **Blob fetch is for members only.** This covers attachments too. It is built after the attachments work in
   flight lands, in coordination with it.
6. **Linked folders are seeded in place**, by a custom server, with no copy.

## Goals and non-goals

Goals: a per-server catalog with categories, tags, filters, sort and search; visible swarm health; parallel
verified downloads; user-controlled seeding (per-file limits, speed caps, a schedule, a disk budget, metered and
battery rules); linked folders seeded in place; dedupe; reputation from receipts with admin-set levels; the
privacy line of `08-files.md` (relays and storage seeders see ciphertext, a hash and a size).

Non-goals: no BitTorrent interop, magnets or public announce; no enforcement against a modified client (download
rules are requests each app keeps for its own member); no streaming playback; nothing over `MAX_FILE_SIZE`.

## What exists today

| Area | What is there | Where |
| --- | --- | --- |
| Encryption | AES-256-GCM in 64 KiB chunks, `FileKey { key, nonce_prefix }`, nonce = prefix ‖ 3-byte index ‖ last flag | `dissent-files/src/crypt.rs` |
| Blob store | iroh-blobs 0.103 `FsStore`; ciphertext under its BLAKE3 hash, pinned by tag, GC hourly, partials kept 24 h | `dissent-files/src/store.rs` |
| Reference | `FileRef { hash, size, cipher_size, name, mime, key, nonce_prefix }` inside an MLS-sealed message | `dissent-files/src/file_ref.rs` |
| Fetch | `Files::fetch_tallied` tries providers in turn, resuming from what is stored; `tally::Observer` credits each 64 KiB ciphertext piece to the provider that completed it | `store.rs`, `tally.rs` |
| Mirror | `Files::mirror(hash, cipher_size, MirrorPolicy)`, 2 GB per file, 50 GB total | `store.rs` |
| Serving | `BlobsProtocol` on the node under `dissent_files::ALPN`, no access check | `dissent-core/src/engine.rs` |
| Attachments | send, download, transfers, source views (`Direct`/`Via relay`), piece counts, mirror opt-in, cleartext manifest, `MirrorHint` | `dissent-core/src/attachments.rs` |
| Seeder | storage member: holds the community secret, no MLS key; mirrors from manifests and hints | `apps/seeder` |
| Keys per generation | private-channel keys, `ChannelKeys` RPC to readers only | `dissent-core/src/private_keys.rs` |
| Carrier | `carry_sealed` (kind 4) with a fixed accepted list in `open_sealed_carried` | `dissent-events/src/control.rs` |
| Ranks | Owner, Admin, Moderator, member; `Role` is only `Admin` and `Moderator` | `dissent-events/src/state.rs` |
| iroh-blobs not used yet | `Downloader` + `SplitStrategy::Split`, remote `observe`, provider `EventMask` (`ConnectMode::Intercept`, `ThrottleMode::Intercept`), `remote().execute_get(stream_pair, request)` over any connection | iroh-blobs 0.103.1 |

## Phase 0: custom roles and bots

### Model

- **Built-in roles stay.** `Role::Admin` and `Role::Moderator`, `RoleGrant` and `RoleRevoke` and their rules are
  unchanged. Every released client keeps agreeing on them.
- **A custom role** is `RoleSpec { name, color, perms, position }`. Its id (`RoleId`) is the id of the
  `RoleCreate` event that made it, as a channel's id is its `ChannelCreate`. `perms` is a `Perms(u64)` bitset;
  unknown bits are kept and ignored, so later permissions do not need a wire change.
- **Position** orders custom roles: a higher number is higher. Every custom role ranks below Moderator.
- **Assignment** is per (member, role), latest `RoleAssign` by HLC wins.
- **A bot** is a root that an admin registered with `BotRegister { root, name, on }`. It joins like anyone,
  through an invite, and is a person (not storage): it reads the catalog and may hold roles. The registration only
  marks it, so the UI shows a bot badge and the member list groups it. Latest by HLC wins; `on: false` unmarks it.

### Permissions

| Bit | Name | What it allows |
| --- | --- | --- |
| 0 | `MANAGE_ROLES` | create, edit, delete and assign custom roles below the holder's highest position, carrying only bits the holder has |
| 1 | `MANAGE_CHANNELS` | create, edit, order, delete channels and set channel access |
| 2 | `MANAGE_SERVER` | icon, banner, profile, relays, history setting, members-host-relays |
| 3 | `MANAGE_MESSAGES` | delete others' messages |
| 4 | `BAN_MEMBERS` | ban and unban members below the holder |
| 5 | `MANAGE_INVITES` | revoke invites |
| 6 | `MENTION_EVERYONE` | mention everyone |
| 7 | `BYPASS_SLOWMODE` | post through slowmode |
| 8 | `MANAGE_FILES` | file categories, removing others' listings, reputation rules and resets |
| 9 | `MANAGE_BOTS` | register and unregister bots |
| 10 | `MANAGE_STORAGE` | storage grants |

- The owner has every bit. An admin has every bit. A moderator has `MANAGE_MESSAGES`, `BAN_MEMBERS` and
  `BYPASS_SLOWMODE`, which is what the moderator rank allows today. A member has the union of their custom roles.
- Owner-only actions stay owner-only: `MemberOnlyControl`, `CommunityRename`, `SeederForwardsCalls`, granting
  `Admin`. No bit grants them.
- **Standing**, for "below the holder": (rank, highest custom position). Owner > Admin > Moderator > any custom
  holder, and among custom holders, the higher top position. Equal standing is not below.

### Replay rules

- `RoleCreate { spec }`, `RoleUpdate { role, spec }`, `RoleDelete { role }`: control channel; the actor needs
  `MANAGE_ROLES`, `spec.position` must be below the actor's standing (any for Admin and Owner), and `spec.perms`
  must be a subset of the actor's bits. Update and delete also need the role itself below the actor. Name 1 to 32
  characters, trimmed. At most 64 live custom roles.
- `RoleAssign { user, role, on }`: control; `MANAGE_ROLES`; the role below the actor; the user a member who is not
  above or equal to the actor (assigning to oneself is allowed only for roles below one's standing). A deleted role
  is refused.
- `BotRegister { root, name, on }`: control; `MANAGE_BOTS`; name as for a role; the root must not be the owner.
- **Effects on existing kinds**: replay honors a custom role's bits on released kinds (`ChannelCreate`, `Ban`,
  `Delete`, ...) only while `CustomRolePerms { on: true }` is in force. That switch is owner only, carried, and the
  owner may turn it on only when every member device answers `RolesSupport` with true, as for member-only control.
  Without it, a released client would reject an event a new client accepts, and the two would replay different
  servers. New kinds (roles, bots, files) always honor custom bits, since released clients see them locked anyway.
- **Revocation of a role's bits** is immediate in replay order: an event is checked against the roles in force at
  its position.

### Compatibility

All six payloads (`RoleCreate`, `RoleUpdate`, `RoleDelete`, `RoleAssign`, `BotRegister`, `CustomRolePerms`) are
appended after `StorageGrant` and carried (`carry_sealed`). `RolesSupport` is an appended RPC. A released client
shows the events as locked and keeps its built-in view of ranks.

### Core and app

- Views: each member's custom roles (id, name, color), bot flag; the community's roles list in position order;
  `my_perms`.
- Engine: `create_role`, `update_role`, `delete_role`, `assign_role`, `register_bot`, `set_custom_role_perms`.
- Bots: `create_bot(community, name)` makes a fresh identity, an invite for it, and a `BotRegister`, and returns a
  bot token (`dissent-bot:` + the root seed in base32 + the invite link). A headless runner uses the token. The UI
  warns that the token is a secret.
- UI: Server settings, Roles section (list, edit, permissions, members), member list role colours and the bot
  badge.

## Concepts

- **Category.** A named bucket with an icon, allowed types, default tags, who may add, who may see it, and
  download rules. Managed with `MANAGE_FILES`. A category's linked folder and auto-import switch are local to one
  computer.
- **Listing.** One file in one category: name, type, size, tags, description, lister, time, fingerprint, files-key
  generation, blob hash and ciphertext size.
- **Fingerprint.** `blake3(plaintext)`, shown as `b3:…`, sealed inside the listing.
- **Blob hash.** `blake3(ciphertext)`, what moves on the wire.
- **Holding, swarm, receipt, reputation.** As in the artboards: who has a listing; its seeders and leechers; a
  downloader's signed credit to seeders; what the credits add up to.

## Encryption, fingerprints and dedupe

The files key of generation `g` is `F_g`, 32 random bytes (see "Keys"). For a file with fingerprint `fp`:

```
key          = blake3::derive_key("dissent/tracker-content-key/v1",   F_g ‖ fp)
nonce_prefix = blake3::derive_key("dissent/tracker-content-nonce/v1", F_g ‖ fp)[..8]
```

Encryption is the existing chunked AES-256-GCM under that `FileKey`. So every file is encrypted under a key
derived from the server key, and the same bytes under the same generation always give the same ciphertext and
blob hash. Two members who add the same file, even at the same moment, make the same blob: one swarm.

- **Dedupe.** Adding a file hashes it first (pass 1). If a listing has that fingerprint, the app uses that
  listing's generation, gets the same blob hash, and seeds that listing ("Already listed by X"). Otherwise it uses
  the newest generation and lists it.
- **Concurrent listings** of the same file are merged in the view by blob hash: the earliest by HLC is shown, the
  others as "Also listed by". The later lister's app withdraws its own listing.
- **Nonce reuse.** A key is bound to one fingerprint, so two different plaintexts never share a key and nonces,
  short of a BLAKE3 collision. The remaining risk is a linked file that changes after it was fingerprinted: the
  in-place server would encrypt new bytes under the old fingerprint's key. So that server checks every chunk group
  it makes against the stored bao outboard **before it sends it**, and stops serving that listing on a mismatch.
  Bytes that do not match never leave the device.

**Leakage of the server key (convergent encryption within the server).**

- Anyone holding `F_g` can test whether a given file is in the server: encrypt the candidate and compare blob
  hashes. Current members already see every fingerprint in the catalog, so this tells them nothing new.
- A member who left keeps the generations they held. They can test candidates against listings made under those
  generations, but they see new listings' blob hashes only if they hold the current community secret, which a
  rotation takes away, and blob fetch refuses them. New files use the new generation.
- A low-entropy file (a template with a few unknown fields) can be brute-forced by an `F_g` holder who sees its
  blob hash. Such a holder can usually just download it, so the bar is the same.
- Storage seeders and relays do not hold `F_g` and can test nothing. A storage seeder can tell that two listings
  are the same file only by seeing the same blob hash, and the view merges those anyway.
- Exposure of `F_g` exposes every file of that generation, not one. A new generation starts at each secret
  rotation (ban or leave), which bounds it.

## Keys: the files key

- Each generation is 32 random bytes. The first admin action that needs it (creating a category) makes
  generation 0.
- `Request::FilesKeys { community }` (appended) returns every `(generation, key)` the responder holds, to an
  active person only, never a storage member, whatever the history setting (decision 2).
- A new generation starts when the community secret rotates. The rotating admin makes it and offers it to the
  persons who remain (`Request::PushFilesKeys`), as the author of a `ChannelAccess` offers channel keys. Until a
  device holds the newest generation it lists under the newest it has.
- Kept in `files_keys.bin`, sealed with the vault, one file per profile.
- A category visible only to some members (artboard: "Visible to Testers") has its own key per generation, served
  by `Request::CategoryKeys { community, category }` to its readers. Phase 5.

## Events

Appended after the roles payloads, pinned in `wire_order.rs`, carried (`carry_sealed`). Two layers:

- **Outer**, under the community secret: what replay checks and what a storage seeder mirrors.
- **Inner**, a `FilesSealed { generation, ciphertext }` inside the outer payload: XChaCha20-Poly1305 under
  `derive_key("dissent/files-sealed/v1", F_g)`, associated data = the event header. Replay never opens it; the view
  does, and checks it.

| Payload | Outer | Inner | Who may publish |
| --- | --- | --- | --- |
| `FileCategorySet { category: Option<CategoryId>, add: AddSpec, inner }` | id, who may add | name, icon, allowed types, default tags, download rules | `MANAGE_FILES` |
| `FileCategoryDelete { category }` | id | — | `MANAGE_FILES` |
| `FileListing { category, blob: [u8; 32], cipher_size: u64, inner }` | category, blob hash, ciphertext size | name, type, MIME, size, fingerprint, generation, tags, description | per `AddSpec` |
| `FileListingUpdate { listing, inner }` | listing | tags, description | the lister, `MANAGE_FILES` |
| `FileListingRemove { listing }` | listing | — | the lister, `MANAGE_FILES` |
| `FileReceipt { tag: [u8; 16], credits: Vec<(RootId, u64)> }` | all | — | any person |
| `ReputationRules { levels, distinct, decay_days, admins_reset }` | all | — | `MANAGE_FILES` |
| `ReputationReset { root }` | root | — | `MANAGE_FILES`, while `admins_reset` |

- `AddSpec { min_rank: u8, roles: Vec<RoleId>, members: Vec<RootId>, min_level: u8 }`: an actor may add when
  their rank is at least `min_rank`, or they hold a listed role, or are listed, or their level is at least
  `min_level` (0 = no level path). "Admins, CI bot" is `min_rank = Admin` plus the bot's role; "Members (Lv 5+)" is
  `min_level = 5`.
- **Replay checks the outer layer only.** The view checks the inner layer and hides a listing whose type is not
  allowed in its category, whose sizes do not agree with `cipher_size`, or whose generation it cannot open yet
  (shown as locked until keys arrive).
- **Receipts do not name the file** (decision 4). `tag` is `blake3::keyed_hash(device_receipt_secret, listing)`
  truncated: the same for one downloader and listing, unlinkable by anyone else. Replay counts a (downloader, tag)
  once, credits each receipt at most `cipher_len(MAX_FILE_SIZE)` in total, and ignores credit to the downloader's
  own root. "Downloaded completely N times" on the file page then comes from holdings (distinct roots ever seen
  complete), not receipts.
- Levels are computed in the view from per-root aggregates and the current time. A level gate in replay
  (`min_level`) uses the listing's HLC wall time as "now", so every peer with the same DAG agrees.

## Swarm state: holdings

Not events. A per-community tracker gossip topic, `blake3("dissent/tracker-topic/v1" ‖ community)`.

- `HoldingsBeacon`, signed by the device key, sealed under the files key (a seeder seals under the community
  secret): `{ community, seq, at_ms, digest, seeding, leeching, up_bps }`. On change at most every 10 s, else every
  5 minutes.
- `Request::Holdings { community, digest }` pulls the full list: `Vec<(blob, Complete | Partial { ranges })>`. For
  active persons and granted storage members only.
- Each device keeps the latest holdings per device with `at_ms`, persisted, for 7 days. Seeders = distinct roots
  with a complete holding in that window; leechers = partial holdings with a beacon in the last 2 minutes;
  "online now" = those with live presence.
- Piece availability: iroh-blobs remote `observe` from online holders, counted per 64 KiB ciphertext piece.

## Downloads

- Many sources at once: iroh-blobs `Downloader` with `SplitStrategy::Split`, fed from holdings; the tally keeps
  per-provider credit for the receipt. Rarest first is a later scheduler on the availability map.
- A provider that sends a bad piece fails bao verification, is dropped, and gets no credit.
- Download rules (ratio after N GB, seed for N h) are kept by the member's own app.
- **Members only** (decision 5): the blobs protocol takes `ConnectMode::Intercept` and serves a device only when
  its root is an active person or granted storage member of a community this device is in, or a party to a direct
  or group conversation with it. The tracker-get server applies the same rule.

## Seeding

- **From the store**: a downloaded listing stays pinned and seeds until its limit ends.
- **In place** (decision 6): an ALPN `dissent/tracker-get/v1` speaking iroh-blobs' GET wire format. For each
  linked file it keeps the bao outboard of the ciphertext (made while encrypting during pass 2, about 0.4 % of the
  size). For a request it reads the plaintext chunks, encrypts them with the listing's derived key, checks them
  against the outboard, and sends them. A downloader reads it with the stock client through `execute_get`, so the
  stream is verified as before.
- **Watching**: on start, every 10 minutes, and on file-system events, each linked folder is scanned; a file is new
  when its (path, size, mtime) is not recorded, and a changed file is re-fingerprinted before it is served again.
  Auto-import lists new files without the dialog.
- **Limits per listing**: Forever, until ratio r, for n hours, until a date. States: Seeding, Paused, Scheduled,
  Queued, Stopped.
- **Global**: upload and download caps, an alternative cap on a weekly schedule, max active seeds, disk kept for
  seeding (`MirrorPolicy`; linked files do not count), metered and battery pauses. Upload caps through
  `ThrottleMode::Intercept` and the same bucket in tracker-get; download caps through a limiter in the fetch path.
  Metered and battery come from OS code in the app crate, passed to core as settings; the battery signal also
  fills `on_battery` in volunteer offers.
- **Priority**: when upload slots are full, requests queue by the requester's level.

## Storage seeders

They read the outer layer only: blob hash and ciphertext size of listings in everyone-visible categories, which
they mirror within `MirrorPolicy`. They do not learn names, types, tags, fingerprints, keys or holdings. A
restricted category's outer layer carries no blob for them (phase 5).

## Privacy

| Party | Sees | Does not see |
| --- | --- | --- |
| Members | Categories, listings, tags, listers, fingerprints, holdings, receipts' credits | Linked folder paths; which file a receipt is for |
| Storage seeders | Category ids, who may add, listing ids and listers, blob hashes and sizes, receipts' credits, which devices ask for blobs | Names, types, tags, fingerprints, keys, holdings |
| Relays | Connection metadata and ciphertext | Everything else |
| Everyone else | Nothing | — |

For `06-threat-model.md`: holdings reveal to members which files a member keeps and when they are online; a
receipt published soon after a holding turns complete can be linked to it by timing; reputation is gameable by
colluding members within the per-receipt bounds; the server-key leakage above.

## Build plan

Every phase ends green on `cargo test -p` and `cargo clippy -p -- -D warnings` for the crates it touches, with
networked tests `#[ignore]` and run through `scripts/fwtest.sh`.

0. **Roles and bots.** Events, replay, `CustomRolePerms` switch and `RolesSupport` probe, core API and views, bot
   identities and tokens, Server settings Roles UI.
1. **Tracker events.** Payloads, replay, `AddSpec`, receipts aggregation, `wire_order`.
2. **Files key and catalog.** Key generations and RPCs, inner seal, derived content keys, add with dedupe, catalog
   view, Browse and Add UI.
3. **Swarm.** Topic, beacons, `Holdings`, parallel download, receipts, File page and My seeding UI.
4. **Seeding controls.** Limits, caps, schedule, queue, OS signals, reputation rules UI, Files section in Server
   settings.
5. **In place and access.** Watcher, outboards, tracker-get server, members-only blob fetch (with the attachments
   work), restricted categories.
6. **Docs and fixture.** `08-files.md`, `06-threat-model.md`, `upgrade_compat` fixture.

## Gap table

**exists** / **partial** / **missing** as of the first draft.

### TrackerBrowse

| UI element | Backend | Status |
| --- | --- | --- |
| Tabs Browse, My seeding, Uploads with counts | catalog view; seeding list; my listings | missing |
| Search | local search over opened names and tags | missing |
| Categories with counts, linked-folder mark | `FileCategorySet` + local folders | missing |
| Type, tag and health filters | type class, tags, seeders count | missing |
| Sortable table (name, category, size, seeders, leechers, completed, added, uploader) | catalog + holdings | missing |
| Your standing, level progress and hint | reputation aggregates + rules | missing |
| Ratio, seed time, uploaded, downloaded | local counters | partial (`StoreStats`) |
| Perks | level + `AddSpec.min_level` + priority | missing |

### TrackerFile

| UI element | Backend | Status |
| --- | --- | --- |
| Header, tags, description | listing view | missing |
| Download from N seeders in parallel | parallel fetch from holdings | partial (in-turn fetch) |
| Seed from my folder, fingerprint matches | scan + fingerprint + derived key | missing |
| Piece availability map | `observe` bitfields | missing (`PIECE_BYTES` exists) |
| Rare pieces fetched first | rarest-first scheduler | missing |
| Facts (fingerprint, size, pieces, category, folder, encryption) | listing view | missing |
| Swarm peers: route, has %, rate, level | holdings + `observe` + source views + reputation | partial |
| Activity: completed, first seeded, also listed by, my upload and ratio | holdings history, dedupe merge, counters | missing |
| Integrity | bao + tally exist; credit removal | partial |

### TrackerSeeding

| UI element | Backend | Status |
| --- | --- | --- |
| Summary tiles | counters, provider events, reputation | missing |
| Pause all / resume all, per-file pause, stop | seeding state machine | missing |
| Table with status, ratio, uploaded, peers, limit | per-listing state and counters | missing |
| Seed limit popover | per-listing limit | missing |
| Upload and download caps | throttle and limiter | missing |
| Alternative schedule | settings | missing |
| Max active seeds | queue | missing |
| Disk kept for seeding | `MirrorPolicy`, store stats | partial |
| Unmetered only, pause on battery | OS signals | missing (`on_battery` hard-coded) |

### TrackerCategories

| UI element | Backend | Status |
| --- | --- | --- |
| Files section in Server settings | new section | missing |
| Category table, New, Edit, Delete | `FileCategorySet`, `FileCategoryDelete` | missing |
| Linked folder, Change | local setting + picker | partial (picker exists) |
| Allowed types, default tags, download rules | inner fields | missing |
| Who can add (Admins, CI bot, Members Lv 5+, Moderators+) | `AddSpec`, custom roles, bots | missing (phase 0) |
| Visible to (Everyone, Testers) | category keys + custom roles | missing |
| Levels and reputation policies, reset in audit log | `ReputationRules`, `ReputationReset` | missing |

### TrackerAdd

| UI element | Backend | Status |
| --- | --- | --- |
| Found since the last scan, stays in place | watcher, scan records, tracker-get | missing |
| Ready, Fingerprinting n%, Already listed by X, Type not allowed | pass 1 progress, dedupe, allowed types | missing |
| Tags, category, description, seed after adding | inner fields, seed limit | missing |
| Signed by you, members only | carrier + files key | missing |
| Batch CTA | batch add | missing |

### FilesServer

Multi-source attachment download: partial (verification, resume and source views exist; parallel sources do not).
Phase 3's parallel fetch serves attachments too.

## Open questions

1. Expected scale: the holdings design is sized for about 1,300 files and 150 seeders per server, not 100k files.
2. Where the Files area lives: a server-level view beside channels (planned), or a channel kind.
3. Level decay ("fades after 30 days without seeding"): planned as halving per 30 days since the seeder's latest
   credited receipt.
4. Receipt volume: one event per finished download; compaction is not designed yet.
