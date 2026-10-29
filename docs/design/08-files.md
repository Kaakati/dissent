# Design 08: encrypted file sharing

Status: `crates/dissent-files` implements storage, encryption and transfer, with tests. Core mounts
it and sends, fetches and mirrors attachments (`dissent-core/src/attachments.rs`, `mirror_hint.rs`).

## Why iroh-blobs and not IPFS

- **One networking stack.** IPFS would bring libp2p next to iroh. That means a second set of
  connections, NAT traversal and relays. iroh-blobs runs on the iroh endpoints Dissent already
  uses, so it gets the same device-key authentication, NAT traversal and relays.
- **No public announcements.** The public IPFS DHT announces which node holds each content ID.
  That tells strangers what a community shares and who fetches it. Files are private by default,
  so announcing them would leak exactly what we want to hide.
- **No gateways.** Public IPFS gateways bring back central servers that see every request.
- **Verified streams.** iroh-blobs checks each BLAKE3 chunk as it arrives, so a bad peer cannot
  feed us bytes that do not match the hash.

## How a file moves

1. The sender encrypts the file with a new random key.
2. The ciphertext goes into the local blob store under its BLAKE3 hash.
3. The sender builds a `FileRef`: hash, sizes, a sanitized name and MIME type, the key and the
   nonce prefix.
4. The `FileRef` travels inside an MLS-sealed message, so only group members read it.
5. A recipient fetches the ciphertext from any provider, checks it against the hash, and decrypts
   it into a downloads folder.

## Keys and encryption

- Keys only travel inside MLS-sealed messages. A non-member never sees a key.
- Every file has its own random key. Exposing one key exposes one file.
- Content is AES-256-GCM in 64 KiB chunks. Each chunk's nonce holds an 8-byte random prefix, a
  3-byte chunk index and a last-chunk flag.
- The nonce is 12 bytes, so the index is 3 bytes rather than 4. That caps a file at 1 TiB, far
  above the 10 GB limit.
- The index stops chunks from being reordered or spliced. The last-chunk flag stops truncation at a
  chunk boundary. Any change anywhere makes decryption fail, and nothing is written to the final path.

## What each party can see

| Party | Sees | Does not see |
| --- | --- | --- |
| Relays | Connection metadata, ciphertext | Content, name, key |
| Seeders | BLAKE3 hash, ciphertext size and bytes, which devices ask for it | Content, name, key |
| Sender's node | Its own plaintext, the same ciphertext view as a seeder | Recipients' keys |
| Recipients | Plaintext, after the MLS message opens | Nothing extra |

Ciphertext length reveals the plaintext length to within one 64 KiB chunk. We do not pad yet.

## Limits

- **Size limit.** Files are capped at 10 GB (`MAX_FILE_SIZE`, 10 × 1024³ bytes) of plaintext. A
  node can lower its own limit but never raise it. Adding a larger file fails. Fetching rejects a
  reference whose size or ciphertext size is over the cap before any bytes move. A peer that sends
  more bytes than the reference declares is cut off mid-download.
- **Bytes input.** `Input::Bytes` holds its payload in memory, so it is for small payloads only. It
  refuses data over 64 MiB (`MAX_BYTES_INPUT`) and points the caller to `Input::Path`.
- **Memory.** No path reads a whole file into memory. `add` streams the file through the encryptor
  into the store's streaming import, with a bounded number of chunks in flight. `fetch` decrypts
  by reading the verified blob through a blob reader, one chunk at a time. A test in
  `tests/memory.rs` adds and fetches a 300 MB file and checks the heap peak. It takes minutes in
  debug builds, so it is `#[ignore]` and runs in release.
- **Disk use.** Every operation checks the blobs volume first, and refuses with `DiskFull` when
  free space is short. `add` needs the ciphertext plus 64 MiB (`DISK_MARGIN`). `fetch` needs the
  ciphertext plus the plaintext plus the margin, because the plaintext is staged on that volume
  before it moves to the destination. `mirror` needs the ciphertext plus the margin. Free space comes
  from `fs4::available_space`, and `Files::with_free_space` replaces it in tests.
- **Garbage collection.** The store sweeps unpinned blobs once an hour. Unpinning takes effect at
  the next sweep, so the disk space can stay in use for up to an hour. There is no on-demand sweep,
  because iroh-blobs 0.103 does not expose one.
- **Resumable downloads.** A failed or cancelled fetch keeps the ciphertext it verified. That
  ciphertext is held under a named tag for 24 hours (`DEFAULT_PARTIAL_TTL`), so garbage collection
  leaves it alone. The next fetch asks iroh-blobs only for the missing ranges. Plaintext is written
  only after the whole ciphertext verifies. The expired holds are released on open, on each fetch,
  and by `Files::prune_partials`. Garbage collection then removes the bytes on its next run. A
  process that stops midway keeps the bytes it fetched too: `a_download_resumes_after_the_process_restarts`
  stops a fetch at half, reopens the data folder, and checks that only the rest crosses the wire.
- **Timeouts.** A provider gets 15 seconds to connect and 30 seconds of silence before it is
  dropped. The 30-second stall timeout is the main guard against hangs. The whole download gets
  60 seconds plus 10 seconds per MiB, capped at 24 hours. A 10 GB file reaches the cap.
- **Seeder policy.** `mirror` takes a `MirrorPolicy`. Its defaults are 2 GB per file and 50 GB of
  pinned ciphertext in total, counting the node's own files. A refused file moves no bytes.
- **No revocation.** A removed member who already holds a `FileRef` keeps the key and can decrypt
  any ciphertext they have. Revoking old files would mean re-encrypting them.
- **No streaming playback.** A file is decrypted only after the whole blob arrives.
- **Name collisions** get " (n)" added before the extension.
- **Sanitizing.** Received names keep only the last path segment, remove control and bidi
  characters, replace characters Windows forbids and avoid reserved device names. The sanitized
  name is then joined onto the download folder: the one the user picked, else the OS Downloads
  folder. A portable exe uses its profile's downloads folder instead, so it does not write to the
  host's Downloads. The app opens and reveals only files the engine recorded as saved, in any
  download folder it has used, never the rest of a folder.

## Thumbnails

For a PNG, JPEG, WebP or GIF (first frame), the sender makes a JPEG of at most 320 pixels on its long
side, at most 64 KB, and stores it as its own encrypted blob. The EXIF orientation is applied first,
and the JPEG is re-encoded from pixels, so no metadata survives. Transparent pixels are drawn over
white. A picture over 25 MB, or over 40 megapixels, gets none.

The message carries the thumbnail's `FileRef` in a trailer after the existing payload. Payloads
without thumbnails are byte-for-byte what earlier versions wrote. An older reader stops after the
payload it knows and ignores the trailer, so it shows the message and its files, without previews.
A recipient fetches a thumbnail of 64 KB or less when the message arrives, and stores it under the
attachment id in the profile's `thumbnails` folder. The full file still waits for a click, unless
the automatic-download setting covers it. See `06-threat-model.md` for what a thumbnail reveals.

### Video thumbnails

A video gets a thumbnail of its first frame, made and sent the same way as an image's: a JPEG of at most 320 px on
the long side and 64 KB, in the same trailer, fetched on arrival. The UI shows it unblurred, with a play mark. A
click downloads the video, and once it is here, the same click opens it.

- **Reading.** `dissent_video::still::first_frame` reads MP4 and MOV with `re_mp4`, and WebM and MKV with
  `matroska-demuxer`. Both are pure Rust. Only the track headers and the first frames are read, so a large file costs
  little. An MP4's track matrix is applied, so a phone video filmed upright stays upright.
- **Decoding.** H.264 goes through OpenH264 and AV1 through rav1d, the decoders calls use. Frames from the first
  keyframe on are fed until one gives a picture, at most 120.
- **Limits and failures.** VP9, HEVC and other codecs have no decoder here. Neither do 10-bit AV1, encoded Matroska
  tracks or pictures over 8192 by 4608. A video like that, or one that does not parse, is sent with
  no thumbnail, as a plain file card. Nothing fails the send.
- **Code.** `crates/dissent-video/src/still.rs` (with fixtures in `tests/fixtures`),
  `crates/dissent-core/src/video_thumbs.rs`, and `make_thumbnail_rgb` in `dissent-files`.

## Next steps

- Padding to fixed size buckets, if length leaks prove a real concern.

## Integration steps for core and net

Done. These were the steps, kept for reference.

1. Mount the protocol handler. Call `Files::protocol_handler()` and accept it under
   `dissent_files::ALPN` on the router that owns the endpoint. Net builds its own router today, so
   it needs a way to accept more protocols before spawn, as it does for the RPC and media handlers.
2. Shutdown order. The handler's shutdown closes the blob store, so the router and the store must
   share a lifetime.
3. Send path. Call `Files::add`, then send the returned `FileRef` in a new MLS-sealed message variant.
4. Receive path. Call `Files::fetch` with the sender's device address and any other members
   known to hold the blob. Store the returned path in the message view.
5. Seeders. Call `Files::mirror` with the public hash, the ciphertext size and a `MirrorPolicy`
   (the defaults are 2 GB per file and 50 GB in total) to keep an attachment available without its key.

## Cleartext manifest (metadata)

A sealed channel message that carries files is preceded, inside its signed event, by a cleartext
manifest. For each file it lists the ciphertext hash, the ciphertext size, and whether members may
mirror it. Nothing else is in clear: no key, name or MIME type. Storage-only members read the
manifest and mirror within their `MirrorPolicy`, without any key.

What this reveals: that a member shared a file, the file's ciphertext hash, and its size. Because
each file has a random key, the hash reveals nothing about the content. The size reveals roughly how
large the file was. Recipients check that the manifest matches the files in the sealed text, and
refuse the message when it does not.

Direct messages carry no manifest. Their files are fetched from the sender's devices after the
recipient reads the sealed text.

## Per-source progress

A download reports what each provider delivered. `TransferView.sources` lists the providers in the
order the file store tried them. Each source has its device, display name, a `state` (`waiting`
while the transfer runs, `done` once it finishes), `bytes` (the ciphertext it delivered that is now
in the store) and `verified_pieces` (the pieces its bytes completed, each checked against the file's
hashes as it arrived). `TransferView.pieces` holds the total and verified piece counts.

The attachment card reads the same counts. `AttachmentView.sources` lists only the providers that
delivered bytes, with an average rate, and `AttachmentView.pieces` gives the piece counts. The card
says "From N sources" when more than one provider served the file, and the details panel shows the
piece grid. The UI shows these only when the engine advertises `multi_source` in `CAPABILITIES`.

How the counts are made: `dissent-files` tries the providers one after another (failover), and each
attempt's bytes are the growth of the store during that attempt. A provider that delivers part of
the file and then fails keeps its share, and the next provider fetches the rest. The counts add up
to the file size, but the providers are not downloaded in parallel. The file store does not report
whether a provider was reached directly or by relay, so `route` is always `direct`.

The counts are held in memory for the run. They are not stored or sent on the wire, so after a
restart an attachment has no per-source counts until it is downloaded again.

Tests: `two_providers_each_deliver_part_of_the_file` in `crates/dissent-files/tests/transfer.rs`
(networked, both providers deliver and the sum covers the file), unit tests of the view mapping in
`crates/dissent-core/src/attachments.rs`, and the driver's download checks in `apps/desktop/ui-next`.

## What a seeder serves (v0.1.8)

A seeder is a storage member. It holds ciphertext and no read keys, and it serves what it can hold without one.

- **Attachments.** Ciphertext of files the sender marked `seed`, mirrored within the seeder's `MirrorPolicy`
  and found by the cleartext manifest. The seeder app keeps them by default, within `--keep-files-up-to` and
  `--files-quota`; `--no-files` turns it off.
- **Pictures.** The server icon, the server banner (still and animated, with its poster), each member's avatar,
  and each member's profile banner. These come from community events, which a storage member can open with the
  community secret, so a seeder fetches them like any member does and keeps the blobs it holds. Their fetch
  sources are now every member of the server, not only the picture's owner, so a seeder that holds a picture
  can serve it when the owner is offline. Covered by `seeder_serves`.
- **Thumbnails and link-preview images.** Their references sit inside the sealed message, so a seeder cannot
  read their hashes, and the cleartext manifest is left alone (older readers refuse a message whose manifest does
  not match its files). Instead, the sender tells the community's seeders the hash and size of each thumbnail and
  preview picture when it posts, and a member does the same after it fetches one. A seeder keeps the ciphertext
  within its `MirrorPolicy`, from any member who holds it, and accepts hints only from members. See `mirror_hint`
  and `06-threat-model.md`, "Seeder hints". Older seeders reject the unknown request, which costs nothing.
- **History keys and epoch keys. Never.** Storage members are outside the group, and `EpochHistory` and
  `HistoryKeys` answer only active members (`05`). A seeder never serves a read key, so joiners' history comes from
  active members.

## Who may fetch a blob (members only)

The blob server used to answer anyone who named a hash. It now serves a device only when the device's root
may see something the blob belongs to (`dissent-core/src/blob_gate.rs`). The node mounts
`Files::protocol_handler_with` with an iroh-blobs `EventSender` that intercepts every connection and every `get`,
`get_many` and `observe`. Push stays disabled. A refusal is iroh-blobs' `AbortReason::Permission`.

| Scope | Blobs | Served to |
| --- | --- | --- |
| Community | Files, thumbnails and preview pictures of a public channel's messages; files named by a manifest; hinted blobs | Active persons and storage members of the server, not banned, not left |
| Channel | The same, for a private channel | Its readers (`can_read`); storage members are left out |
| Conv | The same, for a direct or group conversation | Its members, while the conversation is not left |
| Shared | People's avatars and profile banners | Contacts, and anyone who shares a server or a conversation with us |
| Public | A server's icon and banner (still, animated and poster) | Anyone: an invite card shows them before the reader joins |

- Our own devices are always served. A revoked device is never served. A hash with no scope is refused.
- Membership is checked on every request, so a ban takes effect on a connection that is already open.
- Storage members pass as replay admits them. Once storage grants are required (phase 2 of `storage_grants`), an
  ungranted seeder is no longer a member and is refused with no change here.
- The index of scopes is built at start from the content, manifests and pictures this device keeps, and grows as
  messages are stored. Hints a seeder accepted are kept in `blob_hints.bin`, since a seeder cannot read the message
  they came from. A refusal rebuilds the index once, at most every 2 s, so a blob that arrived by a path that
  records no scope is still served.
- **A device we have not met.** A fresh joiner may ask before its certificate reaches us. The gate refuses, and at
  once asks the device for its signed address record (`Request::AddrLookup` for itself), which carries its
  certificate, at most once every 10 s per device. On the fetching side `Files::fetch_tallied` asks a provider that
  refused once more after `Timeouts::refused_retry` (1 s), and that ask is served. Membership still comes only from
  our own state; the record only proves which root the device belongs to.
- A fetch that every provider refused ends in `FilesError::Refused`, which the attachment card shows as its own
  reason.
- **Unpinning.** A blob nothing live refers to any more is unpinned, and the store's hourly garbage collection
  removes it: a deleted message's files, thumbnails and preview pictures (ours, or arriving from their author, on
  members and seeders alike), a left server's or group's blobs, and a removed contact's pictures. A sweep at start
  catches what changed while the device was off, and keeps files shared in the last day. See `blob_release.rs`.
- Covered by `crates/dissent-core/tests/it/blob_gate.rs` (members, strangers, bans, conversations, private
  channels, a fresh joiner) and `crates/dissent-files/tests/transfer.rs` (a refusal, and a refusal followed by a
  served retry).
