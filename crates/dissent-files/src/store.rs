use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_lite::stream::StreamExt;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::api::blobs::{AddPathOptions, ImportMode};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::{BlobFormat, BlobsProtocol, Hash, HashAndFormat};

use crate::crypt::{self, cipher_len, Sealed};
use crate::error::{FilesError, Result};
use crate::file_ref::FileRef;
use crate::name::{sanitize_mime, sanitize_name};

/// Hard maximum plaintext size for one file: 100 MB. Configured limits can only be lower.
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// How often unpinned blobs are swept from the store.
pub const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);

const PIN_PREFIX: &str = "dissent/files/pin/";
const MIB: u64 = 1024 * 1024;

/// Time limits for fetching from providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Longest wait to connect to one provider. The next provider is tried after this.
    pub connect: Duration,
    /// Longest gap with no bytes from one provider. That provider is dropped after this.
    pub stall: Duration,
    /// Fixed part of the budget for a whole download.
    pub base: Duration,
    /// Extra budget per MiB of ciphertext.
    pub per_mib: Duration,
    /// Ceiling for the whole download budget.
    pub max: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            stall: Duration::from_secs(30),
            base: Duration::from_secs(60),
            per_mib: Duration::from_secs(10),
            max: Duration::from_secs(20 * 60),
        }
    }
}

impl Timeouts {
    /// Budget for downloading `cipher_size` bytes: `base + per_mib` per MiB, at most `max`.
    pub fn overall(&self, cipher_size: u64) -> Duration {
        let mibs = u32::try_from(cipher_size.div_ceil(MIB)).unwrap_or(u32::MAX);
        self.base
            .saturating_add(self.per_mib.saturating_mul(mibs))
            .min(self.max)
    }
}

/// What to share. Name and MIME type are sender-supplied and get sanitized.
#[derive(Debug, Clone)]
pub enum Input {
    /// A file on disk. The name comes from the last path segment.
    Path { path: PathBuf, mime: String },
    /// Bytes already in memory.
    Bytes {
        name: String,
        mime: String,
        data: Vec<u8>,
    },
}

/// Encrypted file storage over an iroh-blobs `FsStore`.
///
/// The store holds ciphertext only. Plaintext appears only in files that `fetch` writes to a
/// directory the caller chooses. Clones share the same store, so `Files` can be shared behind an
/// `Arc`, and `protocol_handler` gives the handler a clone.
///
/// Unpinned blobs, including partial downloads left by failed or cancelled fetches, are removed
/// by the store's garbage collection, which runs every [`DEFAULT_GC_INTERVAL`] by default.
pub struct Files {
    store: FsStore,
    staging: PathBuf,
    size_limit: u64,
    timeouts: Timeouts,
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files")
            .field("staging", &self.staging)
            .field("size_limit", &self.size_limit)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl Files {
    /// Opens or creates the blob store at `<data_dir>/blobs`, with garbage collection every hour.
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_gc_interval(data_dir, DEFAULT_GC_INTERVAL).await
    }

    /// Like [`Files::open`], with a custom garbage-collection interval. Staging files live in
    /// `<data_dir>/tmp`, and any left over from a crash are removed.
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
        Ok(Self {
            store,
            staging,
            size_limit: MAX_FILE_SIZE,
            timeouts: Timeouts::default(),
        })
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
    pub async fn add(&self, input: Input) -> Result<FileRef> {
        let limit = self.size_limit;
        let (name, mime, source) = match input {
            Input::Path { path, mime } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let len = tokio::fs::metadata(&path).await?.len();
                if len > limit {
                    return Err(FilesError::TooLarge { size: len, limit });
                }
                (name, mime, Source::Path(path))
            }
            Input::Bytes { name, mime, data } => {
                if data.len() as u64 > limit {
                    return Err(FilesError::TooLarge {
                        size: data.len() as u64,
                        limit,
                    });
                }
                (name, mime, Source::Bytes(data))
            }
        };

        let staged = StagedFile::new(self.staging_path());
        let target = staged.path().to_path_buf();
        let sealed = run_blocking(move || encrypt_to_file(source, &target, limit)).await?;

        let tag = self
            .store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: staged.path().to_path_buf(),
                format: BlobFormat::Raw,
                mode: ImportMode::Copy,
            })
            .temp_tag()
            .await
            .map_err(store_err)?;
        drop(staged);
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
    /// download gets [`Timeouts::overall`]. The plaintext goes to a temp file that is renamed on
    /// success, so a failed or cancelled fetch leaves no partial file. `progress` receives
    /// `(ciphertext bytes received, ciphertext bytes total)`. Returns the final path.
    pub async fn fetch(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        fref: &FileRef,
        dest_dir: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        fref.check(self.size_limit)?;
        let hash = Hash::from_bytes(fref.hash);
        let keep = self
            .ensure_blob(endpoint, from, hash, fref.cipher_size, &mut progress)
            .await?;
        let result = self.export_and_decrypt(hash, fref, dest_dir).await;
        drop(keep);
        result
    }

    /// Downloads and pins the ciphertext for `hash` without needing the key, so a seeder can
    /// keep a community's attachments available. The bytes are verified against `hash` as they
    /// arrive, and the store never sees plaintext.
    pub async fn mirror(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        hash: [u8; 32],
        cipher_size: u64,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        let max_cipher = cipher_len(self.size_limit);
        if cipher_size > max_cipher {
            return Err(FilesError::TooLarge {
                size: cipher_size,
                limit: max_cipher,
            });
        }
        let hash = Hash::from_bytes(hash);
        let _keep = self
            .ensure_blob(endpoint, from, hash, cipher_size, &mut progress)
            .await?;
        self.pin_hash(hash).await
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

    async fn pin_hash(&self, hash: Hash) -> Result<()> {
        self.store
            .tags()
            .set(pin_name(&hash), HashAndFormat::raw(hash))
            .await
            .map_err(store_err)
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
        let keep = self.store.tags().temp_tag(hf).await.map_err(store_err)?;

        if !self.is_complete(hf).await? {
            let budget = self.timeouts.overall(cipher_size);
            let download = self.download_providers(endpoint, from, hf, cipher_size, progress);
            match tokio::time::timeout(budget, download).await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(FilesError::Fetch(format!(
                        "timed out after {} s",
                        budget.as_secs()
                    )))
                }
            }
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
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<()> {
        if from.is_empty() {
            return Err(FilesError::NoProviders);
        }
        let mut last_error = String::from("no provider had the blob");
        for addr in from {
            match self
                .download_from(endpoint, addr.clone(), hf, cipher_size, progress)
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

    async fn download_from(
        &self,
        endpoint: &Endpoint,
        addr: EndpointAddr,
        hf: HashAndFormat,
        total: u64,
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
                Ok(Some(GetProgressItem::Progress(done))) if done > total => {
                    // The peer is sending more than the reference declared. Stop before storing more.
                    break Err(FilesError::Fetch(format!(
                        "peer sent more than the declared {total} bytes"
                    )));
                }
                Ok(Some(GetProgressItem::Progress(done))) => progress(done, total),
                Ok(Some(GetProgressItem::Done(_))) => break Ok(()),
                Ok(Some(GetProgressItem::Error(e))) => break Err(FilesError::Fetch(e.to_string())),
            }
        };
        closer.close(0u32.into(), b"done");
        outcome
    }

    async fn export_and_decrypt(
        &self,
        hash: Hash,
        fref: &FileRef,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        let staged = StagedFile::new(self.staging_path());
        self.store
            .blobs()
            .export(hash, staged.path())
            .await
            .map_err(store_err)?;

        tokio::fs::create_dir_all(dest_dir).await?;
        let target = unique_path(dest_dir, &fref.safe_name()).await;
        let partial = self.staging_path();

        // If this future is dropped (cancelled), the blocking step sees the flag and removes the
        // partial output instead of renaming it into place.
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(Arc::clone(&cancelled));
        let job = DecryptJob {
            ciphertext: staged.path().to_path_buf(),
            partial: partial.clone(),
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
}

/// Removes a staging file when dropped, including when the owning future is cancelled.
struct StagedFile(PathBuf);

impl StagedFile {
    fn new(path: PathBuf) -> Self {
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
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
    ciphertext: PathBuf,
    partial: PathBuf,
    target: PathBuf,
    key: crate::crypt::FileKey,
    size: u64,
    cipher_size: u64,
    cancelled: Arc<AtomicBool>,
}

impl DecryptJob {
    fn run(self) -> Result<()> {
        let outcome = self.decrypt_to_partial().and_then(|()| {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(FilesError::Fetch("fetch was cancelled".to_owned()));
            }
            fs::rename(&self.partial, &self.target)?;
            Ok(())
        });
        if outcome.is_err() {
            let _ = fs::remove_file(&self.partial);
        }
        outcome
    }

    fn decrypt_to_partial(&self) -> Result<()> {
        let on_disk = fs::metadata(&self.ciphertext)?.len();
        if on_disk != self.cipher_size {
            return Err(FilesError::Decrypt);
        }
        let mut input = BufReader::new(File::open(&self.ciphertext)?);
        let mut output = BufWriter::new(File::create(&self.partial)?);
        let plain_len = crypt::decrypt_stream(&mut input, &mut output, &self.key)?;
        output.flush()?;
        drop(output);
        if plain_len != self.size {
            return Err(FilesError::Decrypt);
        }
        Ok(())
    }
}

enum Source {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

fn encrypt_to_file(source: Source, target: &Path, limit: u64) -> Result<Sealed> {
    let mut output = BufWriter::new(File::create(target)?);
    let sealed = match source {
        Source::Path(path) => {
            let mut input = BufReader::new(File::open(path)?);
            crypt::encrypt_stream(&mut input, &mut output, limit)?
        }
        Source::Bytes(data) => {
            let mut input: &[u8] = &data;
            crypt::encrypt_stream(&mut input, &mut output, limit)?
        }
    };
    output.flush()?;
    Ok(sealed)
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
