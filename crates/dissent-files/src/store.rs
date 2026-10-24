use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_lite::stream::StreamExt;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::api::blobs::BlobReader;
use iroh_blobs::api::proto::BlobStatus;
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::{BlobsProtocol, Hash, HashAndFormat};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::crypt::{self, cipher_len, plain_len_for_cipher, Sealed};
use crate::error::{FilesError, Result};
use crate::file_ref::FileRef;
use crate::name::{sanitize_mime, sanitize_name};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Hard maximum plaintext size for one file: 10 GB. A configured limit can only be lower.
pub const MAX_FILE_SIZE: u64 = 10 * GIB;

/// Largest payload [`Input::Bytes`] accepts. Anything bigger goes through [`Input::Path`].
pub const MAX_BYTES_INPUT: u64 = 64 * MIB;

/// Free space that must remain on the blobs volume after an operation's own needs.
pub const DISK_MARGIN: u64 = 64 * MIB;

/// How often unpinned blobs are swept from the store.
pub const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// How long the ciphertext of a failed or cancelled fetch is held for resuming.
pub const DEFAULT_PARTIAL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const PIN_PREFIX: &str = "dissent/files/pin/";
const PARTIAL_PREFIX: &str = "dissent/files/partial/";
/// Chunks buffered between the encrypting thread and the blob import.
const STREAM_DEPTH: usize = 16;

/// Time limits for fetching from providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Longest wait to connect to one provider. The next provider is tried after this.
    pub connect: Duration,
    /// Longest gap with no bytes from one provider. That provider is dropped after this. This is
    /// the main guard against hangs, and it applies to every provider attempt.
    pub stall: Duration,
    /// Fixed part of the budget for a whole download.
    pub base: Duration,
    /// Extra budget per MiB of ciphertext.
    pub per_mib: Duration,
    /// Ceiling for the whole download budget. The default is 24 hours.
    pub max: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            stall: Duration::from_secs(30),
            base: Duration::from_secs(60),
            per_mib: Duration::from_secs(10),
            max: Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl Timeouts {
    /// Overall budget for downloading `cipher_size` bytes: `base + per_mib` for each MiB, rounded
    /// up, and never more than `max`. At the default settings a 10 GB file gets the full 24 hours.
    pub fn overall(&self, cipher_size: u64) -> Duration {
        let mibs = u32::try_from(cipher_size.div_ceil(MIB)).unwrap_or(u32::MAX);
        self.base
            .saturating_add(self.per_mib.saturating_mul(mibs))
            .min(self.max)
    }
}

/// Limits a seeder applies before it keeps someone else's ciphertext.
///
/// A seeder holds ciphertext without the key, so it cannot see what a blob is. These limits are
/// the only protection against a community filling its disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorPolicy {
    /// Largest plaintext size accepted for one file. Defaults to 2 GB.
    pub max_file_size: u64,
    /// Most ciphertext this node keeps pinned in total, its own files included. Defaults to 50 GB.
    pub max_total: u64,
}

impl Default for MirrorPolicy {
    fn default() -> Self {
        Self {
            max_file_size: 2 * GIB,
            max_total: 50 * GIB,
        }
    }
}

/// What to share. Name and MIME type are sender-supplied and get sanitized.
#[derive(Debug, Clone)]
pub enum Input {
    /// A file on disk. It is streamed through the encryptor, so memory use does not grow with the
    /// file. The name comes from the last path segment.
    Path { path: PathBuf, mime: String },
    /// Bytes already in memory. For small payloads only: data over [`MAX_BYTES_INPUT`] is refused,
    /// and larger files should use [`Input::Path`].
    Bytes {
        name: String,
        mime: String,
        data: Vec<u8>,
    },
}

type FreeSpaceFn = Arc<dyn Fn(&Path) -> io::Result<u64> + Send + Sync>;

/// Encrypted file storage over an iroh-blobs `FsStore`.
///
/// The store holds ciphertext only. Plaintext appears only in files that `fetch` writes to a
/// directory the caller chooses. Clones share the same store, so `Files` can be shared behind an
/// `Arc`, and `protocol_handler` gives the handler a clone.
///
/// Unpinned blobs are removed by the store's garbage collection, which runs every
/// [`DEFAULT_GC_INTERVAL`] by default. The ciphertext of a failed or cancelled fetch is held under
/// a named tag for [`DEFAULT_PARTIAL_TTL`], so a later fetch can resume from it.
///
/// Memory: `add` streams the file through the encryptor into the store's streaming import, with
/// at most `STREAM_DEPTH` chunks in flight. `fetch` decrypts by reading the blob through a
/// `BlobReader`, one chunk at a time. No path reads a whole file into memory. Only [`Input::Bytes`]
/// holds its payload in memory.
pub struct Files {
    store: FsStore,
    blobs_dir: PathBuf,
    staging: PathBuf,
    size_limit: u64,
    timeouts: Timeouts,
    partial_ttl: Duration,
    free_space: FreeSpaceFn,
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files")
            .field("blobs_dir", &self.blobs_dir)
            .field("staging", &self.staging)
            .field("size_limit", &self.size_limit)
            .field("timeouts", &self.timeouts)
            .field("partial_ttl", &self.partial_ttl)
            .finish_non_exhaustive()
    }
}

impl Files {
    /// Opens or creates the blob store at `<data_dir>/blobs`, with garbage collection every hour.
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_gc_interval(data_dir, DEFAULT_GC_INTERVAL).await
    }

    /// Like [`Files::open`], with a custom garbage-collection interval. Staging files live in
    /// `<data_dir>/tmp`, and any left over from a crash are removed. Partial-download holds that
    /// have expired are released.
    pub async fn open_with_gc_interval(
        data_dir: impl AsRef<Path>,
        gc_interval: Duration,
    ) -> Result<Self> {
        let root = data_dir.as_ref();
        let blobs_dir = root.join("blobs");
        let staging = root.join("tmp");
        fs::create_dir_all(&blobs_dir)?;
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        let mut options = Options::new(&blobs_dir);
        options.gc = Some(GcConfig {
            interval: gc_interval,
            add_protected: None,
        });
        let store = FsStore::load_with_opts(blobs_dir.join("blobs.db"), options)
            .await
            .map_err(store_err)?;
        let files = Self {
            store,
            blobs_dir,
            staging,
            size_limit: MAX_FILE_SIZE,
            timeouts: Timeouts::default(),
            partial_ttl: DEFAULT_PARTIAL_TTL,
            free_space: Arc::new(|dir: &Path| fs4::available_space(dir)),
        };
        files.prune_partials().await?;
        Ok(files)
    }

    /// Lowers the plaintext size limit. Values above [`MAX_FILE_SIZE`] are clamped down to it.
    pub fn with_size_limit(mut self, bytes: u64) -> Self {
        self.size_limit = bytes.min(MAX_FILE_SIZE);
        self
    }

    /// Replaces the fetch time limits.
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Replaces how long a failed or cancelled fetch keeps its partial ciphertext.
    pub fn with_partial_ttl(mut self, ttl: Duration) -> Self {
        self.partial_ttl = ttl;
        self
    }

    /// Replaces the free-space probe used by the disk checks. The probe gets the blobs directory
    /// and returns available bytes. The default asks the operating system.
    pub fn with_free_space(
        mut self,
        probe: impl Fn(&Path) -> io::Result<u64> + Send + Sync + 'static,
    ) -> Self {
        self.free_space = Arc::new(probe);
        self
    }

    pub fn size_limit(&self) -> u64 {
        self.size_limit
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// The iroh-blobs store, for advanced use. It holds ciphertext only.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Protocol handler that serves blobs to peers. Mount it on a `Router` under [`crate::ALPN`].
    pub fn protocol_handler(&self) -> BlobsProtocol {
        BlobsProtocol::new(self.store(), None)
    }

    /// Stops the blob store. Shutting down a `Router` that serves this handler also stops it.
    pub async fn shutdown(&self) -> Result<()> {
        self.store.shutdown().await.map_err(store_err)
    }

    /// Encrypts `input`, stores the ciphertext, and pins it so this node keeps serving it.
    ///
    /// Needs the ciphertext size plus [`DISK_MARGIN`] free on the blobs volume.
    pub async fn add(&self, input: Input) -> Result<FileRef> {
        let limit = self.size_limit;
        let (name, mime, source, plain_len) = match input {
            Input::Path { path, mime } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let len = tokio::fs::metadata(&path).await?.len();
                if len > limit {
                    return Err(FilesError::TooLarge { size: len, limit });
                }
                (name, mime, Source::Path(path), len)
            }
            Input::Bytes { name, mime, data } => {
                let len = data.len() as u64;
                if len > MAX_BYTES_INPUT {
                    return Err(FilesError::BytesTooLarge {
                        size: len,
                        max: MAX_BYTES_INPUT,
                    });
                }
                if len > limit {
                    return Err(FilesError::TooLarge { size: len, limit });
                }
                (name, mime, Source::Bytes(data), len)
            }
        };

        self.check_space(cipher_len(plain_len) + DISK_MARGIN)
            .await?;
        let (tag, sealed) = self.store_encrypted(source, limit).await?;
        let hash = tag.hash();
        self.pin_hash(hash).await?;
        drop(tag);

        Ok(FileRef {
            hash: *hash.as_bytes(),
            size: sealed.plain_len,
            cipher_size: sealed.cipher_len,
            name: sanitize_name(&name),
            mime: sanitize_mime(&mime),
            key: sealed.key.key,
            nonce_prefix: sealed.key.nonce_prefix,
        })
    }

    /// Downloads the ciphertext for `fref` from the first provider that has it, then decrypts it
    /// into `dest_dir` under a sanitized version of the sender's name.
    ///
    /// Bytes are verified against the BLAKE3 hash as they arrive. Each provider gets
    /// [`Timeouts::connect`] to connect and [`Timeouts::stall`] between chunks, and the whole
    /// download gets [`Timeouts::overall`]. A failed or cancelled download keeps the bytes it
    /// verified, and the next call resumes from them. Plaintext is written to a staging file, and
    /// it moves into `dest_dir` only after the whole ciphertext verifies and decrypts. Needs the
    /// ciphertext plus the plaintext plus [`DISK_MARGIN`] free on the blobs volume.
    ///
    /// `progress` receives `(ciphertext bytes held, ciphertext bytes total)`, counting bytes from
    /// earlier attempts. Returns the final path.
    pub async fn fetch(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        fref: &FileRef,
        dest_dir: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        fref.check(self.size_limit)?;
        self.check_space(fref.cipher_size + fref.size + DISK_MARGIN)
            .await?;
        let hash = Hash::from_bytes(fref.hash);
        let keep = self
            .ensure_blob(endpoint, from, hash, fref.cipher_size, &mut progress)
            .await?;
        let result = self.decrypt_to_dest(hash, fref, dest_dir).await;
        drop(keep);
        result
    }

    /// Downloads and pins the ciphertext for `hash` without needing the key, so a seeder can
    /// keep a community's attachments available. The bytes are verified against `hash` as they
    /// arrive, and the store never sees plaintext.
    ///
    /// `policy` decides whether this node keeps the file at all. The check runs before any bytes
    /// move. Needs the ciphertext plus [`DISK_MARGIN`] free on the blobs volume.
    pub async fn mirror(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        hash: [u8; 32],
        cipher_size: u64,
        policy: MirrorPolicy,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        let plain = plain_len_for_cipher(cipher_size).ok_or_else(|| {
            FilesError::BadRef(format!("{cipher_size} is not a valid ciphertext size"))
        })?;
        if plain > self.size_limit {
            return Err(FilesError::TooLarge {
                size: plain,
                limit: self.size_limit,
            });
        }
        if plain > policy.max_file_size {
            return Err(FilesError::PolicyRefused(format!(
                "{plain} bytes is over the {} byte per-file limit",
                policy.max_file_size
            )));
        }
        let pinned = self.pinned_bytes().await?;
        if pinned.saturating_add(cipher_size) > policy.max_total {
            return Err(FilesError::PolicyRefused(format!(
                "keeping {cipher_size} more bytes would pass the {} byte total",
                policy.max_total
            )));
        }
        self.check_space(cipher_size + DISK_MARGIN).await?;

        let hash = Hash::from_bytes(hash);
        let keep = self
            .ensure_blob(endpoint, from, hash, cipher_size, &mut progress)
            .await?;
        let result = self.pin_hash(hash).await;
        drop(keep);
        result
    }

    /// Keeps the blob for `hash` in the store until [`Files::unpin`]. The blob must be present.
    pub async fn pin(&self, hash: [u8; 32]) -> Result<()> {
        let hash = Hash::from_bytes(hash);
        if !self.store.blobs().has(hash).await.map_err(store_err)? {
            return Err(FilesError::Store(format!("no blob {}", hash.to_hex())));
        }
        self.pin_hash(hash).await
    }

    /// Drops a pin. The blob is removed by the next garbage-collection run. Returns whether a
    /// pin was removed.
    pub async fn unpin(&self, hash: [u8; 32]) -> Result<bool> {
        let name = pin_name(&Hash::from_bytes(hash));
        let removed = self.store.tags().delete(name).await.map_err(store_err)?;
        Ok(removed > 0)
    }

    /// Removes partial-download holds whose time is up. Their ciphertext is then removed by the
    /// next garbage-collection run, unless it is pinned. This runs on open and before each download.
    pub async fn prune_partials(&self) -> Result<()> {
        let now = now_ms();
        let mut tags = self
            .store
            .tags()
            .list_prefix(PARTIAL_PREFIX)
            .await
            .map_err(store_err)?;
        let mut expired = Vec::new();
        while let Some(tag) = tags.next().await {
            let tag = tag.map_err(store_err)?;
            let name = String::from_utf8_lossy(tag.name.as_ref()).into_owned();
            if expiry_of(&name).is_some_and(|at| at <= now) {
                expired.push(name);
            }
        }
        for name in expired {
            self.store.tags().delete(name).await.map_err(store_err)?;
        }
        Ok(())
    }

    async fn pin_hash(&self, hash: Hash) -> Result<()> {
        self.store
            .tags()
            .set(pin_name(&hash), HashAndFormat::raw(hash))
            .await
            .map_err(store_err)
    }

    /// Ciphertext bytes and file count of the blobs this node keeps pinned. Blobs still downloading
    /// count as nothing.
    pub async fn pinned_stats(&self) -> Result<(u64, u64)> {
        let mut tags = self
            .store
            .tags()
            .list_prefix(PIN_PREFIX)
            .await
            .map_err(store_err)?;
        let mut hashes = Vec::new();
        while let Some(tag) = tags.next().await {
            hashes.push(tag.map_err(store_err)?.hash);
        }
        let (mut total, mut count) = (0u64, 0u64);
        for hash in hashes {
            if let BlobStatus::Complete { size } =
                self.store.blobs().status(hash).await.map_err(store_err)?
            {
                total = total.saturating_add(size);
                count += 1;
            }
        }
        Ok((total, count))
    }

    /// Ciphertext bytes of every pinned blob that is complete.
    async fn pinned_bytes(&self) -> Result<u64> {
        let mut tags = self
            .store
            .tags()
            .list_prefix(PIN_PREFIX)
            .await
            .map_err(store_err)?;
        let mut hashes = Vec::new();
        while let Some(tag) = tags.next().await {
            hashes.push(tag.map_err(store_err)?.hash);
        }
        let mut total = 0u64;
        for hash in hashes {
            if let BlobStatus::Complete { size } =
                self.store.blobs().status(hash).await.map_err(store_err)?
            {
                total = total.saturating_add(size);
            }
        }
        Ok(total)
    }

    /// Fails with [`FilesError::DiskFull`] when the blobs volume has less than `needed` free.
    async fn check_space(&self, needed: u64) -> Result<()> {
        let probe = Arc::clone(&self.free_space);
        let dir = self.blobs_dir.clone();
        let available = run_blocking(move || Ok(probe(&dir)?)).await?;
        if available < needed {
            return Err(FilesError::DiskFull { needed, available });
        }
        Ok(())
    }

    /// Encrypts `source` and streams the ciphertext into the store. Nothing is staged on disk.
    async fn store_encrypted(&self, source: Source, limit: u64) -> Result<(TempTag, Sealed)> {
        let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(STREAM_DEPTH);
        let encryptor = tokio::task::spawn_blocking(move || encrypt_into(source, limit, tx));
        let chunks = futures_lite::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        let imported = self.store.blobs().add_stream(chunks).await.temp_tag().await;
        let sealed = encryptor
            .await
            .map_err(|e| FilesError::Store(format!("blocking task failed: {e}")))?;
        match (sealed, imported) {
            (Ok(sealed), Ok(tag)) => Ok((tag, sealed)),
            // A broken pipe means the import stopped first, so the import's error is the cause.
            (Err(e), Err(_)) if !is_broken_pipe(&e) => Err(e),
            (_, Err(import)) => Err(store_err(import)),
            (Err(e), Ok(_)) => Err(e),
        }
    }

    /// Makes sure the complete blob for `hash` is in the local store, fetching it if needed.
    /// The returned temp tag keeps the blob safe from garbage collection until it is dropped.
    async fn ensure_blob(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        hash: Hash,
        cipher_size: u64,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<TempTag> {
        let hf = HashAndFormat::raw(hash);
        // Protects the blob from garbage collection while this call runs.
        let keep = self.store.tags().temp_tag(hf).await.map_err(store_err)?;
        self.prune_partials().await?;

        if !self.is_complete(hf).await? {
            // The hold outlives this call, so a failed or cancelled download keeps what it verified.
            self.hold_partial(hash).await?;
            let base = self.local_bytes(hf).await?;
            progress(base, cipher_size);
            let budget = self.timeouts.overall(cipher_size);
            let download = self.download_providers(endpoint, from, hf, cipher_size, base, progress);
            let outcome = match tokio::time::timeout(budget, download).await {
                Ok(result) => result,
                Err(_) => Err(FilesError::Fetch(format!(
                    "timed out after {} s",
                    budget.as_secs()
                ))),
            };
            outcome?;
            self.release_partial(hash).await?;
        }

        let local = self.store.remote().local(hf).await.map_err(store_err)?;
        if local.local_bytes() != cipher_size {
            return Err(FilesError::Fetch(format!(
                "stored {} bytes, declared {cipher_size}",
                local.local_bytes()
            )));
        }
        progress(cipher_size, cipher_size);
        Ok(keep)
    }

    async fn download_providers(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        hf: HashAndFormat,
        cipher_size: u64,
        base: u64,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<()> {
        if from.is_empty() {
            return Err(FilesError::NoProviders);
        }
        let mut last_error = String::from("no provider had the blob");
        for addr in from {
            match self
                .download_from(endpoint, addr.clone(), hf, cipher_size, base, progress)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => last_error = e.to_string(),
            }
            if self.is_complete(hf).await? {
                return Ok(());
            }
        }
        Err(FilesError::Fetch(last_error))
    }

    /// Requests the missing ranges from one provider. iroh-blobs asks only for what the store
    /// lacks, so a partial blob resumes. `base` is the byte count the store already held.
    async fn download_from(
        &self,
        endpoint: &Endpoint,
        addr: EndpointAddr,
        hf: HashAndFormat,
        total: u64,
        base: u64,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<()> {
        let timeouts = self.timeouts;
        let conn = tokio::time::timeout(timeouts.connect, endpoint.connect(addr, iroh_blobs::ALPN))
            .await
            .map_err(|_| FilesError::Fetch("connect timed out".to_owned()))?
            .map_err(|e| FilesError::Fetch(e.to_string()))?;
        let closer = conn.clone();
        let stream = self.store.remote().fetch(conn, hf).stream();
        let mut stream = std::pin::pin!(stream);
        let outcome = loop {
            match tokio::time::timeout(timeouts.stall, stream.next()).await {
                Err(_) => {
                    break Err(FilesError::Fetch(format!(
                        "no data for {} s",
                        timeouts.stall.as_secs()
                    )))
                }
                Ok(None) => break Err(FilesError::Fetch("stream ended early".to_owned())),
                Ok(Some(GetProgressItem::Progress(done))) if base + done > total => {
                    // The peer is sending more than the reference declared. Stop before storing more.
                    break Err(FilesError::Fetch(format!(
                        "peer sent more than the declared {total} bytes"
                    )));
                }
                Ok(Some(GetProgressItem::Progress(done))) => progress(base + done, total),
                Ok(Some(GetProgressItem::Done(_))) => break Ok(()),
                Ok(Some(GetProgressItem::Error(e))) => break Err(FilesError::Fetch(e.to_string())),
            }
        };
        closer.close(0u32.into(), b"done");
        outcome
    }

    /// Decrypts the verified blob into a staging file, then moves it to `dest_dir`.
    async fn decrypt_to_dest(
        &self,
        hash: Hash,
        fref: &FileRef,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        tokio::fs::create_dir_all(dest_dir).await?;
        let target = unique_path(dest_dir, &fref.safe_name()).await;

        // If this future is dropped (cancelled), the blocking step sees the flag and removes the
        // partial output instead of moving it into place.
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(Arc::clone(&cancelled));
        let job = DecryptJob {
            reader: StoreReader {
                handle: Handle::current(),
                reader: self.store.blobs().reader(hash),
                read: 0,
            },
            partial: self.staging_path(),
            target: target.clone(),
            key: fref.file_key(),
            size: fref.size,
            cipher_size: fref.cipher_size,
            cancelled,
        };
        run_blocking(move || job.run()).await?;
        Ok(target)
    }

    fn staging_path(&self) -> PathBuf {
        self.staging
            .join(format!("{:016x}.part", rand::random::<u64>()))
    }

    async fn is_complete(&self, hf: HashAndFormat) -> Result<bool> {
        let local = self.store.remote().local(hf).await.map_err(store_err)?;
        Ok(local.is_complete())
    }

    async fn local_bytes(&self, hf: HashAndFormat) -> Result<u64> {
        let local = self.store.remote().local(hf).await.map_err(store_err)?;
        Ok(local.local_bytes())
    }

    /// Holds the ciphertext for `hash` for [`Files::with_partial_ttl`] time, replacing any earlier hold.
    async fn hold_partial(&self, hash: Hash) -> Result<()> {
        self.release_partial(hash).await?;
        let expires = now_ms().saturating_add(self.partial_ttl.as_millis() as u64);
        self.store
            .tags()
            .set(partial_name(&hash, expires), HashAndFormat::raw(hash))
            .await
            .map_err(store_err)
    }

    async fn release_partial(&self, hash: Hash) -> Result<()> {
        let prefix = format!("{PARTIAL_PREFIX}{}/", hash.to_hex());
        self.store
            .tags()
            .delete_prefix(prefix)
            .await
            .map_err(store_err)?;
        Ok(())
    }
}

/// Where the bytes to encrypt come from.
enum Source {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

/// Encrypts `source` and sends each sealed chunk down `tx`. On failure it sends the error too, so
/// the import fails instead of storing a truncated blob.
fn encrypt_into(source: Source, limit: u64, tx: mpsc::Sender<io::Result<Bytes>>) -> Result<Sealed> {
    let mut out = ChannelWriter(tx.clone());
    let result = match source {
        Source::Path(path) => File::open(&path)
            .map_err(FilesError::from)
            .and_then(|file| {
                let mut input = BufReader::new(file);
                crypt::encrypt_stream(&mut input, &mut out, limit)
            }),
        Source::Bytes(data) => {
            let mut input: &[u8] = &data;
            crypt::encrypt_stream(&mut input, &mut out, limit)
        }
    };
    if let Err(e) = &result {
        let _ = tx.blocking_send(Err(io::Error::other(e.to_string())));
    }
    result
}

/// Sends written bytes to the blob import. Each write becomes one chunk.
struct ChannelWriter(mpsc::Sender<io::Result<Bytes>>);

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "blob import stopped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads a stored blob from a blocking thread. Each read is one request to the store, so the
/// blob never has to be exported to a file first.
struct StoreReader {
    handle: Handle,
    reader: BlobReader,
    read: u64,
}

impl Read for StoreReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self
            .handle
            .block_on(tokio::io::AsyncReadExt::read(&mut self.reader, buf))?;
        self.read += n as u64;
        Ok(n)
    }
}

/// Sets a flag when dropped, so blocking work can see that its caller was cancelled.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The decrypt-and-place step, run on a blocking thread.
struct DecryptJob {
    reader: StoreReader,
    partial: PathBuf,
    target: PathBuf,
    key: crypt::FileKey,
    size: u64,
    cipher_size: u64,
    cancelled: Arc<AtomicBool>,
}

impl DecryptJob {
    fn run(mut self) -> Result<()> {
        let outcome = self.decrypt_to_partial().and_then(|()| {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(FilesError::Fetch("fetch was cancelled".to_owned()));
            }
            place(&self.partial, &self.target)
        });
        if outcome.is_err() {
            let _ = fs::remove_file(&self.partial);
        }
        outcome
    }

    fn decrypt_to_partial(&mut self) -> Result<()> {
        let mut output = BufWriter::new(File::create(&self.partial)?);
        let mut input = BufReader::new(&mut self.reader);
        let plain_len = crypt::decrypt_stream(&mut input, &mut output, &self.key)?;
        output.flush()?;
        drop(output);
        if plain_len != self.size || self.reader.read != self.cipher_size {
            return Err(FilesError::Decrypt);
        }
        Ok(())
    }
}

/// Moves a finished staging file to its final name. Staging and destination can sit on different
/// volumes, where a rename fails, so that case copies the file instead.
fn place(partial: &Path, target: &Path) -> Result<()> {
    if fs::rename(partial, target).is_ok() {
        return Ok(());
    }
    if let Err(e) = fs::copy(partial, target) {
        let _ = fs::remove_file(target);
        return Err(e.into());
    }
    fs::remove_file(partial)?;
    Ok(())
}

/// Picks a free name in `dir`, adding " (n)" before the extension when needed.
async fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let direct = dir.join(name);
    if !tokio::fs::try_exists(&direct).await.unwrap_or(true) {
        return direct;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1u32.. {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !tokio::fs::try_exists(&candidate).await.unwrap_or(true) {
            return candidate;
        }
    }
    unreachable!("u32 candidates exhausted")
}

fn pin_name(hash: &Hash) -> String {
    format!("{PIN_PREFIX}{}", hash.to_hex())
}

fn partial_name(hash: &Hash, expires_ms: u64) -> String {
    format!("{PARTIAL_PREFIX}{}/{expires_ms}", hash.to_hex())
}

/// The expiry time in a partial-hold tag name, in Unix milliseconds.
fn expiry_of(name: &str) -> Option<u64> {
    name.rsplit('/').next()?.parse().ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_broken_pipe(e: &FilesError) -> bool {
    matches!(e, FilesError::Io(io) if io.kind() == io::ErrorKind::BrokenPipe)
}

async fn run_blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| FilesError::Store(format!("blocking task failed: {e}")))?
}

fn store_err(e: impl std::fmt::Display) -> FilesError {
    FilesError::Store(e.to_string())
}
