# Design 14: server file tracker

Status: design reconciled with the user's decisions (below). Built phase by phase; each phase's state is in
"Build plan". Phases 0 and 1 are built, phase 2 is built in its first form (copies files into the store; the
in-place server is phase 5). Derived from the tracker artboards (TrackerBrowse, TrackerFile, TrackerSeeding, TrackerCategories,
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
| Serving | `BlobsProtocol` on the node under `dissent_files::ALPN`, members only through `blob_gate` (see `08-files.md`, "Who may fetch a blob") | `dissent-core/src/blob_gate.rs` |
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
  characters, trimmed. At most 250 live custom roles.
- `RoleAssign { user, role, on }`: control; `MANAGE_ROLES`; the role below the actor; the user a member who is not
  above or equal to the actor (assigning to oneself is allowed only for roles below one's standing). A deleted role
  is refused.
- `BotRegister { root, name, on }`: control; `MANAGE_BOTS`; name as for a role; the root must not be the owner.
- **Effects on existing kinds**: replay honors a custom role's bits on released kinds (`ChannelCreate`, `Ban`,
  `Delete`, ...) only while `CustomRolePerms { on: true }` is in force. That switch is owner only, carried, and goes on
  under "Switches older apps cannot follow" below.
  A released client rejects an event a new client accepts while the switch is on, and the two replay different servers
  until that client updates. New kinds (roles, bots, files) always honor custom bits, since released clients see them
  locked anyway.
- **Revocation of a role's bits** is immediate in replay order: an event is checked against the roles in force at
  its position.

### Compatibility

All six payloads (`RoleCreate`, `RoleUpdate`, `RoleDelete`, `RoleAssign`, `BotRegister`, `CustomRolePerms`) are
appended after `StorageGrant` and carried (`carry_sealed`). `Request::Features` / `Response::Features(u64)` is an
appended, generic feature probe; bit 0 is custom roles. A released client shows the events as locked and keeps its
built-in view of ranks.

### Switches older apps cannot follow

`CustomRolePerms`, `MemberOnlyControl` and `HistoryVisibility` change what a device on an older app can follow. One member
who never updates or never logs in again must not be able to hold a server hostage, and nobody should be surprised. So
`Engine::gate_status` asks every member device at once (`GateStatus`) and sorts them:

| Answer | Means | Effect |
| --- | --- | --- |
| The probe's yes | Current | Nothing. |
| An answer without the bit, or `Response::Error("bad request: ...")`, which every release has sent for a request it cannot decode | Online and too old | Blocks by default. The owner may override. |
| No answer, a failed dial, or another error | Offline or out of reach; nobody can tell its version | Named in a warning, and does not hold the switch back. |

Going ahead always takes an explicit act. `set_custom_role_perms_confirmed`, `set_member_only_confirmed` and
`set_history_visibility_confirmed` take the device ids the owner confirmed, and refuse if a device is listed that is not
among them, so a device that turned up after the owner read the warning is not skipped silently. The plain forms confirm
nothing. The UI's confirmation names the people and says what they will see until they update.

What a returning device sees:

- **On a current app**, nothing unusual: it syncs and reads everything.
- **`CustomRolePerms`, on an app from before it.** The switch shows as locked, so it keeps the old rules: it refuses
  channels, deletions and bans made through a custom role, and what was said in those channels. It sees a different
  server. Updating recovers it, since refused events stay in the log and replay runs again from them with the switch
  readable (`a_returning_old_app_diverges_on_custom_bits_and_updating_recovers_it`).
- **Member-only, on an app from before it.** Control events sealed under the epoch show as locked (members, names, roles,
  channels, settings changed since). Updating recovers them once its device has the epoch keys, through `MlsCommits` and
  `EpochHistory` from any online member.
- **Files, on an app from before the tracker.** See "No gate on files" under "Events": nothing to confirm.

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

- Each generation is 32 random bytes. Its number is `(secret generation << 32) | random u32`, so two admins who
  make one at once make two distinct generations, never two keys under one number; the newest is the highest. The
  first file manager who needs one and finds none at any member makes it (when creating a category).
- `Request::FilesKeys { community }` (appended) returns every `(generation, key)` the responder holds, to an
  active person only, never a storage member, whatever the history setting (decision 2).
- A new generation starts when the community secret rotates. The rotating admin makes it and offers it to the
  persons who remain (`Request::PushFilesKeys`), as the author of a `ChannelAccess` offers channel keys. Until a
  device holds the newest generation it lists under the newest it has.
- Kept in `files_keys.bin` in the community's folder, sealed like every store file (`at_rest`).
- A category visible only to some members (artboard: "Visible to Testers") has its own key per generation
  (`tracker::restricted`). Its spec, its listings' fields and its files are sealed and encrypted under that key, and
  its listings carry no blob in clear (replay refuses one that does). Category keys share the ring with the files
  key, by generation; `scopes` records which category a generation belongs to, so `FilesKeys` never hands one out.
  `Request::CategoryKeys { community, category }` answers a person who may see it in the responder's replay, and
  `PushCategoryKeys` offers a new generation to the readers. A new generation is made on every spec change and after
  every secret rotation. A device answers `Holdings` with a restricted category's blobs only to its readers. Whether
  a category is restricted is fixed when it is made.

## Events

Appended after the roles payloads, pinned in `wire_order.rs`, carried (`carry_sealed`). Two layers:

- **Outer**, under the community secret: what replay checks and what a storage seeder mirrors.
- **Inner**, a `FilesSealed { generation, ciphertext }` inside the outer payload: XChaCha20-Poly1305 under
  `derive_key("dissent/files-sealed/v1", F_g)`, associated data = `"dissent/files-sealed-aad/v1" ‖ community ‖
  author ‖ kind`, so a body cannot move to another server, author or event kind. (The event header is not final
  when the body is sealed, so it is not the associated data.) Replay never opens it; the view does, and checks it.

| Payload | Outer | Inner | Who may publish |
| --- | --- | --- | --- |
| `FileCategorySet { category: Option<CategoryId>, add: AddSpec, visible: Option<ReaderSpec>, inner }` | id, who may add, who may see | name, icon, allowed types, default tags, download rules | `MANAGE_FILES` |
| `FileCategoryDelete { category }` | id | — | `MANAGE_FILES` |
| `FileListing { category, blob: Option<([u8; 32], u64)>, inner }` | category; blob hash and ciphertext size for an everyone-visible category, `None` (enforced) for a restricted one | name, type, MIME, size, fingerprint, content generation, blob, tags, description | per `AddSpec` |
| `FileListingUpdate { listing, inner }` | listing | tags, description | the lister, `MANAGE_FILES` |
| `FileListingRemove { listing }` | listing | — | the lister, `MANAGE_FILES` |
| `FileReceipt { tag: [u8; 16], credits: Vec<(RootId, u64)> }` | all | — | any person |
| `ReputationRulesSet { rules: ReputationRules { levels, distinct, decay_days, admins_reset } }` | all | — | `MANAGE_FILES` |
| `ReputationReset { root }` | root | — | `MANAGE_FILES`, while `admins_reset` |

- **No gate on files.** Every kind above is carried, so an app from before the tracker sees them as locked, agrees on
  everything else and cannot diverge (`an_old_app_sees_tracker_events_as_locked_and_agrees_on_the_rest`). A server starts
  sharing files whoever is offline or out of date (`an_old_app_or_an_offline_member_does_not_stop_a_server_sharing_files`).
  `FEATURE_TRACKER` is still answered, and nothing depends on it. If every other holder of the files key is offline, a
  file manager makes a new generation, and a member reads what the online holders can give it. A new member needs one
  holder online to read the catalog.
- `AddSpec { min_rank: u8, roles: Vec<RoleId>, members: Vec<RootId>, min_level: u8 }`: an actor may add when
  their rank is at least `min_rank` (0 member … 3 owner, 4 `NO_RANK` = no rank path), or they hold a listed role, or
  are listed, or their level is at least `min_level` (0 = no level path). `MANAGE_FILES` always may. "Admins, CI
  bot" is `min_rank = 2` plus the bot's role; "Members (Lv 5+)" is `min_rank = NO_RANK, min_level = 5`.
- Replay keeps who wrote each category's spec in force (`CategoryState.author`), since its sealed body names them.
- A listing update's body is sealed with the lister in its associated data, whoever writes it, so every reader opens it the
  same way. The lister and holders of `MANAGE_FILES` may write one, as replay checks; the listing's page has the control.
  The listing still shows its lister, and does not say who edited it last.
- Bounds: sealed fields at most 16 KiB, at most 256 roles or members in a spec, 64 credits per receipt, a receipt at
  most 11 GiB in total, at most 10 levels.
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

Not events, and not gossip: each device asks (built in phase 3, `tracker::swarm`).

- `Request::Holdings { community, digest: Option<[u8; 32]> }` → `Response::Holdings(Option<(digest, Vec<(blob,
  bytes held)>)>)`, `None` when the asker's digest is current. A device answers active persons and granted storage
  members of that server only. What it reports: the blobs it seeds (whole, at the listing's ciphertext size) and its
  tracker downloads in progress (partial).
- Every device asks the online member devices of each server that has listings at most every 30 s
  (`HOLDINGS_EVERY`): from the 30 s maintenance tick, and when the Files area opens. Answers are kept in memory with
  their time. Seeders = distinct roots with a whole holding heard in the last 7 days, plus listers not heard from
  yet, plus this person if it seeds; leechers = partial holdings heard in the last 3 minutes. "Downloaded completely"
  = distinct roots ever heard holding it whole, other than its listers, kept in the local book.
- Why polling rather than a gossip beacon: no new topic or signed beacon format to freeze, the digest makes a repeat
  answer a few bytes, and the swarm needs only online members. A gossip beacon can come later without changing the
  RPC.
- What is moving now, for an open file page: `Engine::listing_live` (`tracker::live`, read about once a second) answers from
  this device's own byte counts per blob and device, kept in memory and read without touching the network: the rates up and
  down, the time left and pieces of this device's download, and each peer it exchanges with. Who else holds what stays with
  `Holdings`, at its slower pace.
- Piece availability for the file page: iroh-blobs remote `observe` from up to 16 online holders
  (`Files::piece_map`), counted per 64 KiB ciphertext piece, plus this device's own copy.

## Downloads

- Many sources at once (`Files::fetch_swarm`): the missing ciphertext is cut into 4 MiB slices, and up to 8
  providers each work through slices with ranged iroh-blobs GETs into the same store. Providers are ordered whole
  holders first (freshest answer first), then partial holders, then the listers' devices. A provider that fails a
  slice (bad chunk, stall, or no copy) puts it back for another; after two failures it is dropped. Rarest-first
  ordering of slices is not built: slices go in file order.
- Receipt: on completion the downloader publishes `FileReceipt` with the bytes each provider's root delivered.
  Providers that failed any slice get no credit, nor does the downloader's own root.
- Download rules (ratio after N GB, seed for N h) are kept by the member's own app; `seed_hours` sets the seed limit
  of a finished download.
- **Members only** (decision 5): built, see `08-files.md`, "Who may fetch a blob". The blob server intercepts every
  connection and request and asks `World::may_fetch(device, hash)`, which checks the scopes the blob belongs to.
  Listings add `Scope::Category(c, cat)` through `Engine::add_blob_scope`, checked with `may_see_files` (storage
  members only for a category everyone sees). Linked files are served through the same gate: `FilesProtocol` passes every GET, GET_MANY and observe through the gate's `EventSender`.

## Photo details

Attachments lose identifying details by default (`scrub`). A listing never does: its fingerprint names the file on
disk, and a linked file is served in place, so a scrubbed copy would not match and could not be seeded from the folder.
The Add files dialog says so; a member who wants a photo cleaned sends it as an attachment or cleans it first.

## Browsing

The whole catalog is on the device, so the Files page pages it locally, with no wire change: the page shows 15, 25, 50 or 75
files (25 at first, remembered per device), of what the filters and search match. Changing a filter, the search, the sort or the
size returns to page 1. A listing arriving leaves the page number alone. A page left empty moves to the last valid page.

## Adding files

Adding must not make anyone wait on a file being read (`tracker::adding`). The picker's command returns what can be told
without reading: name, size, type. The dialog opens from that at once, and `start_file_check` fingerprints each file in
the background (four at a time), reporting with `EngineEvent::FileCheck`. Each row shows "Checking…" with its progress, and
Add stays off until every ticked row is known. A type the chosen category refuses is known from the start and is not
waited for. Closing the dialog cancels the job at the next read of each file.

**Whose paths.** A page cannot name a file by itself. Only the OS picker and the app's native drop handler call
`Engine::allow_paths`, and `start_file_check` refuses anything else. A file reaches `offered`, which `list_files` requires, only
when its check says it can be added, so a tricked page cannot publish a file nobody chose, as before.

**Dropping on the Files page** lists with no dialog (`tracker::drops`). Tauri's native drag-drop event gives the paths
(`WindowEvent::DragDrop`, which reports paths on Windows, macOS and Linux; the webview's HTML5 drop has no paths, and Tauri
does not fire it for files while its own handler is on). A drop anywhere else still attaches to the open chat.

- **Category.** `FileType::of_name`, then among the categories this person may add to (`may_add_files`): the page's filter
  if it accepts the file; the narrowest category that names the type; then one allowing any type. Ties go to the first in
  server order.
- **No category.** Someone with `MANAGE_FILES` gets one per type dropped (Images, Documents, ...), open to members. Anyone
  else is told someone has to make one.
- **Left out, with the reason in the toast**: fits no category, over `MAX_FILE_SIZE`, a link, unreadable, and any name that
  looks like a key or password file (`looks_like_credentials`), since a drop asks nothing first and goes to the whole server.
  Add files can still list such a file on purpose.
- **Already listed.** By someone else: this device seeds that listing, as the dialog does. By this device: left alone, so
  its counters stay.
- **Progress** is a tray above the toasts, per file, that blocks nothing. Files are listed one at a time, so a batch can be
  cancelled between files. The bytes of one file being encrypted show no progress yet.
- **Undo** on the toast removes the batch's new listings and stops seeding what it only joined, since an accidental drop
  publishes to the whole server. A single-file drop also offers Add tags.
- Listings stay unscrubbed ("Photo details"). The drop overlay says so once.

**A whole folder**, from Add folder or a drop, always opens a dialog first (`tracker::folders`). It walks as a linked folder
is walked (4 levels, 500 files), and is seeded in place, with no copy, as a linked folder is.

- Category: the page's filter or the one the folder's most common type fits. Tag: the folder's name, editable. Tags
  from the category's defaults apply too.
- Ticked: what can be added. Unticked, and tickable: hidden and system files (dotfiles, desktop.ini, Thumbs.db, and on
  Windows anything with the hidden or system attribute) and names that look like keys or passwords, which also get a warning.
  Not tickable, with the reason: type not allowed, over the size limit, already listed, link not followed.
- Every limit it hits is stated: files past 500 are counted, folders past 4 levels are counted, hidden folders such as `.git`
  are named and not entered, and a walk that gave up (100,000 entries) says more may be in there. Nothing is dropped silently.
- Add stays off until ticked rows are checked. Progress, Cancel and Undo are the drop's. What the dialog checked is not read again
  by the batch, though `list_files` itself still reads the file.

## Seeding

Built in phases 3 to 5 (`tracker::seeding`, `tracker::scan`, `dissent_files::inplace`).

- **From the store**: a finished download is pinned and seeded, for the category's `seed_hours` when it asks for
  them, else for good.
- **In place** (decision 6, `Files::link_file`): linking a file in a category's linked folder reads it once,
  encrypting on the way, to check its fingerprint and write the post-order bao outboard of the ciphertext to
  `outboards/<hash>.obao` (64 bytes per 16 KiB, about 0.4 %). Nothing goes into the store. The node's blobs handler
  is `dissent_files::FilesProtocol`: it serves the iroh-blobs protocol with iroh-blobs' own handlers, and answers a
  GET or an observe for a linked blob itself, on the same ALPN, so a downloader asks the same way whether a copy is
  stored or linked. A GET reads the plaintext chunks it needs, encrypts them, and runs bao-tree's
  `encode_ranges_validated`, which checks each chunk group against the outboard before it writes it: a file that
  changed on disk fails at the first changed group, and nothing of it is sent. A file whose modification time
  changed is refused before reading. After a restart the app relinks its linked seeds (`reconcile_linked`).
- **Scanning**: Files › Scan folder lists the files under the linked folder (4 levels, at most 500) that this device
  does not seed yet, with the same states as a pick, checked as "Adding files" says. Auto-import lists them on its own every 10 minutes, with the
  category's default tags. Live file-system watching is not built.
- **Limits per listing**: Forever, until ratio r, for n hours, until a date. States, worked out from the record, the
  settings and the time: Stopped (by hand, or limit reached), Paused, Scheduled (a metered network, battery, or an
  alternative schedule whose upload cap is 0), Seeding. There is no limit on how many files seed: every Seeding blob is
  announced in holdings and served, subject to the upload slots below. Only Seeding blobs are announced and served: the blob gate asks `serves`, and a linked file is unlinked while not Seeding.
  The unpin sweep keeps every blob a seed book holds; removed seeds release through `release_blobs`.
- **Global** (`SeedingSettings`, `seeding.bin`): upload and download caps; an alternative pair on a weekly window
  in local time (the page reports its UTC offset); uploads at once (`max_active`, see "Upload slots"); only on unmetered networks (on by default);
  pause on battery. The desktop app reads metered (Windows `GetNetworkConnectivityHint`, looked up at run time) and
  battery (Windows `GetSystemPowerStatus`, Linux `/sys/class/power_supply`, macOS `pmset`) every minute. The download
  cap is a token bucket shared by swarm downloads. The upload cap and the per-file upload counters run through the
  blob gate's `throttle` and `on_served` hooks, and, for linked files, through `FilesProtocol::with_served`.
- **Not built**: queueing requests by the requester's level.

### Upload slots

`SeedingSettings::max_active` (default 5, 1 to 100) is the number of GET requests for seeds this device serves at once
(`tracker::slots`). In v0.1.24 and earlier it was the number of seeds served at all, newest first; the field and its format are
unchanged, only its meaning. Only blobs in this device's seed book take a slot: chat files, pictures and avatars are served
whatever the slots say.

- **Counted at the blob gate**, per request, from the GET until its transfer events end. Requests from one device count
  together.
- **Full**: a downloader whose app answers `Request::Features` with `FEATURE_SEED_SLOTS` is refused at once with
  iroh-blobs' `RateLimited` (reset code `ERR_LIMIT`). Its swarm download puts the slice back for other holders, shows the
  provider as busy, and asks it again after 250 ms, doubling to 4 s, back to 250 ms after a slice comes in. A busy answer is
  not one of the two failures that drop a provider.
- **Older downloaders**, and devices that have not answered the probe yet, would count a refusal as a failure, so their
  request is held instead until a slot frees: at most 30 s, 16 requests in all and 4 per device, then refused.
- **Fair**: a freed slot goes to whoever waited longest, held or refused in the last 10 s, before a device that just
  finished a slice and asks again. No device takes more than half the slots, rounded up, while another wants one.
- **Shown**: My seeding has "Uploading n of m"; a file page shows a busy provider as "Busy, asking again", and "Waiting
  for a free upload slot" while every provider is busy.

At 5,000 seeds on one device (release build, `many_seeds`): My seeding 34 ms, the Files view 0.22 s, a holdings answer
of 5,000 blobs 170 KB (the RPC frame limit is 16 MiB), a restart 0.5 s.

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

## Wire shapes

Every type below is postcard, so its field order and variant order are its wire format. They freeze at the merge.
Until then they may change, and this list is reviewed at the merge.

### `Payload` variants, appended after `ChannelLayout` (carried, so a released client shows them as locked)

| Variant | Fields | Why each field is final |
| --- | --- | --- |
| `RoleCreate` | `spec: RoleSpec { name: String, color: u32, perms: Perms(u64), position: u16 }` | The id is the event id, so no id field. `perms` is a 64-bit set: there is room for 53 more permissions, and unknown bits are kept, so new permissions need no wire change. `color` 0 means none, so it needs no `Option`. 65 536 positions is far beyond the 250-role cap. |
| `RoleUpdate` | `role: RoleId, spec: RoleSpec` | It replaces the whole spec, latest by HLC. Partial patches would need a new variant per field. |
| `RoleDelete` | `role: RoleId` | Deletion is permanent, so a role needs no generation. |
| `RoleAssign` | `user: RootId, role: RoleId, on: bool` | The latest by HLC per (user, role) wins. One event both gives and takes a role. |
| `BotRegister` | `root: RootId, name: String, on: bool` | A bot's roles come from `RoleAssign`, and its join comes from an invite, so nothing else is needed. |
| `CustomRolePerms` | `on: bool` | It is a single switch. Narrower switches would be new variants. |
| `FileCategorySet` | `category: Option<CategoryId>, add: AddSpec, visible: Option<ReaderSpec>, inner: FilesSealed` | `None` creates and `Some` updates, in one variant. Who may add and who may see are in the outer layer because replay checks them. Everything else is in `inner`, whose body has its own format (below). |
| `FileCategoryDelete` | `category: CategoryId` | — |
| `FileListing` | `category: CategoryId, blob: Option<([u8; 32], u64)>, inner: FilesSealed` | The blob is clear only for everyone-visible categories, so seeders can mirror those and never the restricted ones. Every other field is sealed. |
| `FileListingUpdate` | `listing: EventId, inner: FilesSealed` | Only tags and the description change. A changed file is a new listing. |
| `FileListingRemove` | `listing: EventId` | — |
| `FileReceipt` | `tag: [u8; 16], credits: Vec<(RootId, u64)>` | Decision 4: no listing. The tag counts a download once without saying which file it was. Bytes per root are all that reputation needs. |
| `ReputationRulesSet` | `rules: ReputationRules { levels: Vec<LevelRule { name, bytes: u64, members: u32 }>, distinct: bool, decay_days: u16, admins_reset: bool }` | These are the levels and the three policies in the design. A new policy would be a new variant. |
| `ReputationReset` | `root: RootId` | — |

`AddSpec { min_rank: u8, roles: Vec<RoleId>, members: Vec<RootId>, min_level: u8 }` and
`ReaderSpec { min_rank: u8, roles: Vec<RoleId>, members: Vec<RootId> }` cover each admission path in the design:
rank (with `NO_RANK` = 4), a role, a named member, and, for adding only, a level. `FilesSealed { generation: u64,
ciphertext: Vec<u8> }` leaves room for any body format inside.

### Sealed bodies (inside `FilesSealed`, opened only by members)

- `CategoryBody { name, icon, types: Vec<FileType>, default_tags, rules: DownloadRules { min_ratio_milli:
  Option<u32>, ratio_after_bytes: u64, seed_hours: Option<u32> } }`
- `ListingBody { name, mime, kind: FileType, size, fingerprint, content_generation, blob, cipher_size, tags,
  description }`
- `ListingUpdateBody { tags, description }`

`FileType` is appended to, never reordered. A body that does not decode is shown as locked, so a later body format
needs a new kind byte in the associated data (`BodyKind`), not a changed struct.

### RPCs, appended after `InboxProbe`

| Request → Response | Fields | Why each field is final |
| --- | --- | --- |
| `Features` → `Features(u64)` | Bits: 0 custom roles. | A probe with 64 bits replaces a new RPC per feature. The Files area will be gated on a bit too. |
| `FilesKeys { community }` → `FilesKeys(Vec<(u64, [u8; 32])>)` | Every generation the responder holds. | Decision 2: every generation goes to every person, so no "since" field is needed. |
| `PushFilesKeys { community, pairs }` → `Ack` | The pairs an admin made after a rotation. | The same pairs as `FilesKeys`. |
| `Holdings { community, digest: Option<[u8; 32]> }` → `Holdings(Option<([u8; 32], Vec<([u8; 32], u64)>)>)` | The digest and (blob, bytes held) pairs. | Bytes held say both whole and partial. Piece detail comes from iroh-blobs `observe`, so this carries none. |
| `CategoryKeys { community, category }` → `CategoryKeys(Vec<(u64, [u8; 32])>)` | A restricted category's generations. | A reader gets them all, as with the files key. |
| `PushCategoryKeys { community, category, pairs }` → `Ack` | A new generation for the readers. | The same pairs as `CategoryKeys`. |

### Local files (this device only, sealed like every store file, see `at_rest`)

`files_keys.bin`: `Vec<(generation, key, Option<CategoryId>)>`. `files_keys.bin` and `tracker.bin` are never replaced after a bad read:
one that does not open or decode moves to `<name>.bad` untouched, any other read error stops saves for that run, and while
a `tracker.bin.bad` is there the unpin sweep keeps everything. `tracker.bin`: `LocalBook { receipt_secret, folders,
auto_import, seeds: BTreeMap<blob, SeedRecord { listing, path, limit, paused, stopped, linked, modified_ms, uploaded,
since_ms, downloaded }>, completed }`. `seeding.bin`: `SeedingSettings`. `outboards/*.obao`: bao post-order
outboards. `tracker_counters.bin` is a marker that a community's upload counters are known good: v0.1.22 counted each slice from the
start of the file, so its saved counters ran too high and a seed under a ratio limit looked finished. A book with no marker has its
counters set to zero, once, and is marked only after that book is saved. It is a separate file so the book keeps the format
v0.1.22 wrote. Neither crosses the wire. After the merge they get an `upgrade_compat`
fixture, and any later change gets a new fixture.

## Build plan

Every phase ends green on `cargo test -p` and `cargo clippy -p -- -D warnings` for the crates it touches, with
networked tests `#[ignore]` and run through `scripts/fwtest.sh`.

State: phases 0 to 5 are built on the `tracker` branch, apart from what the sections above mark as not built or
waiting for the blob gate.

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
| Max active seeds | upload slots (`tracker::slots`) | built |
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
