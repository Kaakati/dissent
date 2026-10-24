# Design 08: encrypted file sharing

Status: `crates/dissent-files` implements storage, encryption and transfer, with tests. Core and
net do not mount it yet. The integration steps are at the end.

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
  and by `Files::prune_partials`. Garbage collection then removes the bytes on its next run.
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
  name is then joined onto the downloads folder the caller chose.

## Next steps

- Thumbnails, generated by the sender and sent as their own small encrypted blob.
- A test that resumes a download across a process restart. The hold is a persistent named tag, so
  the design expects it to work, but no test covers it yet.
- Padding to fixed size buckets, if length leaks prove a real concern.
- Core and UI integration, listed below.

## Integration steps for core and net

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

## Per-source progress (gap)

A download lists its providers in `TransferView.sources`, each with its device, display name and a
`state` (`waiting` while the transfer runs, `done` once it finishes). The `bytes` field of each
source is always 0, and there is no `verified_pieces` field.

The reason: `dissent-files` tries the providers one after another and reports only one progress
callback, with the total held bytes. It does not say which provider served the bytes, or how many
each one sent. A true per-source byte count needs `dissent-files` to expose its provider attempts,
or the engine to drive one attempt per provider and count the progress deltas. Neither is built.
