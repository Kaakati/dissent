//! Seeding a file from where it is, without a copy of its ciphertext. Linking a file reads it once, encrypting it on
//! the way, to build the bao outboard of the ciphertext (about 0.4 % of the size) and check its fingerprint. Serving it
//! reads the plaintext, encrypts the chunks asked for, and checks every chunk group against the outboard **before**
//! sending it, so if the file changed on disk, ciphertext of other bytes under its key never leaves this device. See
//! `docs/design/14-file-tracker.md`, "Seeding" and "Nonce reuse".
//!
//! The protocol handler serves the iroh-blobs protocol for the store, and answers a GET or an observe for a linked blob
//! itself, so a downloader asks this device the same way whether a blob is stored or linked.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::SystemTime;

use aes_gcm::Aes256Gcm;
use bao_tree::io::outboard::PostOrderOutboard;
use bao_tree::io::sync::{encode_ranges_validated, outboard_post_order, ReadAt};
use bao_tree::{BaoTree, BlockSize};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::api::Store;
use iroh_blobs::protocol::{ChunkRanges, Request};
use iroh_blobs::provider::events::{ClientConnected, ConnectionClosed, EventSender, HasErrorCode};
use iroh_blobs::provider::StreamPair;
use iroh_blobs::Hash;
use tokio::sync::mpsc;

use crate::crypt::{self, cipher_len, FileKey, CHUNK_SIZE, TAG_LEN};
use crate::error::{FilesError, Result};
use crate::file_ref::FileRef;
use crate::store::Files;

/// The chunk group size iroh-blobs verifies in: 16 chunks of 1 KiB.
const BLOCK_SIZE: BlockSize = BlockSize::from_chunk_log(4);
const FRAME: u64 = (CHUNK_SIZE + TAG_LEN) as u64;

/// A file seeded from where it is.
#[derive(Clone, Debug)]
pub struct Linked {
    pub path: PathBuf,
    pub key: FileKey,
    pub size: u64,
    pub cipher_size: u64,
    /// The file's modification time when it was linked. A file whose time or size changed is not served.
    pub modified: Option<SystemTime>,
}

/// The linked files of a store, by blob hash.
#[derive(Clone, Default, Debug)]
pub(crate) struct LinkedSet(Arc<RwLock<HashMap<Hash, Linked>>>);

impl LinkedSet {
    fn get(&self, hash: &Hash) -> Option<Linked> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).get(hash).cloned()
    }
}

/// Reads a file as its ciphertext, in order, and hashes its plaintext on the way.
struct CipherReader {
    file: BufReader<File>,
    cipher: Aes256Gcm,
    key: FileKey,
    index: u64,
    chunks: u64,
    out: Vec<u8>,
    at: usize,
    hasher: blake3::Hasher,
}

impl Read for CipherReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at == self.out.len() {
            if self.index == self.chunks {
                return Ok(0);
            }
            let mut plain = vec![0u8; CHUNK_SIZE];
            let n = crypt::read_full(&mut self.file, &mut plain)?;
            plain.truncate(n);
            self.hasher.update(&plain);
            let last = self.index + 1 == self.chunks;
            self.out = crypt::seal_chunk(&self.cipher, &self.key, self.index, last, &plain)
                .map_err(|e| io::Error::other(e.to_string()))?;
            self.at = 0;
            self.index += 1;
        }
        let n = buf.len().min(self.out.len() - self.at);
        buf[..n].copy_from_slice(&self.out[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Reads a linked file's ciphertext at any offset, encrypting the chunk it falls in. The last chunk made is kept, so
/// the encoder's in-order reads encrypt each chunk once.
struct CipherAt {
    file: File,
    cipher: Aes256Gcm,
    key: FileKey,
    chunks: u64,
    cache: RefCell<Option<(u64, Vec<u8>)>>,
}

impl CipherAt {
    fn chunk(&self, index: u64) -> io::Result<Vec<u8>> {
        if let Some((i, bytes)) = self.cache.borrow().as_ref() {
            if *i == index {
                return Ok(bytes.clone());
            }
        }
        let mut plain = vec![0u8; CHUNK_SIZE];
        let n = read_at_full(&self.file, index * CHUNK_SIZE as u64, &mut plain)?;
        plain.truncate(n);
        let sealed = crypt::seal_chunk(&self.cipher, &self.key, index, index + 1 == self.chunks, &plain)
            .map_err(|e| io::Error::other(e.to_string()))?;
        *self.cache.borrow_mut() = Some((index, sealed.clone()));
        Ok(sealed)
    }
}

impl ReadAt for CipherAt {
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        let index = pos / FRAME;
        if index >= self.chunks {
            return Ok(0);
        }
        let chunk = self.chunk(index)?;
        let within = (pos % FRAME) as usize;
        if within >= chunk.len() {
            return Ok(0);
        }
        let n = buf.len().min(chunk.len() - within);
        buf[..n].copy_from_slice(&chunk[within..within + n]);
        Ok(n)
    }
}

fn read_at_full(file: &File, mut pos: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        let n = ReadAt::read_at(file, pos, &mut buf[done..])?;
        if n == 0 {
            break;
        }
        done += n;
        pos += n as u64;
    }
    Ok(done)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

impl Files {
    fn outboard_path(&self, hash: &Hash) -> PathBuf {
        self.blobs_dir.join("outboards").join(format!("{}.obao", hash.to_hex()))
    }

    /// Links the file at `path`, encrypted under `key`, which was derived from `fingerprint`, and serves it from there
    /// from now on. Reads the file once. Fails with [`FilesError::Changed`] when it does not hash to `fingerprint`.
    /// Returns the reference a listing names; nothing is copied into the store.
    pub async fn link_file(&self, path: &Path, mime: &str, key: FileKey, fingerprint: [u8; 32]) -> Result<FileRef> {
        let meta = tokio::fs::symlink_metadata(path).await?;
        if !meta.is_file() {
            return Err(FilesError::BadRef("only a regular file can be linked, not a link or a folder".into()));
        }
        let size = meta.len();
        if size > self.size_limit {
            return Err(FilesError::TooLarge { size, limit: self.size_limit });
        }
        let cipher_size = cipher_len(size);
        let before = modified(path);
        let (owned, k) = (path.to_path_buf(), key.clone());
        let dir = self.blobs_dir.join("outboards");
        let (hash, outboard) = tokio::task::spawn_blocking(move || -> Result<(Hash, Vec<u8>)> {
            std::fs::create_dir_all(&dir)?;
            let mut reader = CipherReader {
                file: BufReader::new(File::open(&owned)?),
                cipher: crypt::cipher_for(&k)?,
                key: k,
                index: 0,
                chunks: crypt::chunk_count(size),
                out: Vec::new(),
                at: 0,
                hasher: blake3::Hasher::new(),
            };
            let mut outboard = Vec::new();
            let root = outboard_post_order(&mut reader, BaoTree::new(cipher_size, BLOCK_SIZE), &mut outboard)?;
            if *reader.hasher.finalize().as_bytes() != fingerprint {
                return Err(FilesError::Changed);
            }
            Ok((Hash::from(root), outboard))
        })
        .await
        .map_err(|e| FilesError::Store(format!("blocking task failed: {e}")))??;
        if modified(path) != before {
            return Err(FilesError::Changed);
        }
        let file_path = self.outboard_path(&hash);
        tokio::fs::write(&file_path, outboard).await?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let linked = Linked { path: path.to_path_buf(), key: key.clone(), size, cipher_size, modified: before };
        self.linked.0.write().unwrap_or_else(PoisonError::into_inner).insert(hash, linked);
        Ok(FileRef {
            hash: *hash.as_bytes(),
            size,
            cipher_size,
            name: crate::name::sanitize_name(&name),
            mime: crate::name::sanitize_mime(mime),
            key: key.key,
            nonce_prefix: key.nonce_prefix,
        })
    }

    /// Serves a file linked before, after a restart. Its outboard must still be on disk, and the file unchanged since
    /// `modified`; otherwise it is not served and this says so.
    pub fn relink(&self, hash: [u8; 32], linked: Linked) -> Result<()> {
        let hash = Hash::from_bytes(hash);
        if !self.outboard_path(&hash).is_file() {
            return Err(FilesError::Store("the file's outboard is gone".into()));
        }
        if modified(&linked.path) != linked.modified || std::fs::metadata(&linked.path).map(|m| m.len()).ok() != Some(linked.size) {
            return Err(FilesError::Changed);
        }
        self.linked.0.write().unwrap_or_else(PoisonError::into_inner).insert(hash, linked);
        Ok(())
    }

    /// Stops serving a linked file. Its outboard stays, so [`Files::relink`] can serve it again.
    pub fn unlink(&self, hash: [u8; 32]) {
        self.linked.0.write().unwrap_or_else(PoisonError::into_inner).remove(&Hash::from_bytes(hash));
    }

    /// Stops serving a linked file for good and drops its outboard.
    pub fn forget_linked(&self, hash: [u8; 32]) {
        self.unlink(hash);
        let _ = std::fs::remove_file(self.outboard_path(&Hash::from_bytes(hash)));
    }

    /// Whether `hash` is served from a linked file.
    pub fn is_linked(&self, hash: [u8; 32]) -> bool {
        self.linked.get(&Hash::from_bytes(hash)).is_some()
    }

    /// The protocol handler for this store and its linked files.
    pub fn protocol_handler(&self) -> FilesProtocol {
        self.protocol_handler_with(EventSender::DEFAULT)
    }

    /// Like [`Files::protocol_handler`], reporting requests to `events`, which may also refuse them.
    pub fn protocol_handler_with(&self, events: EventSender) -> FilesProtocol {
        FilesProtocol {
            store: (*self.store).clone(),
            events,
            linked: self.linked.clone(),
            outboards: self.blobs_dir.join("outboards"),
        }
    }
}

/// Serves the iroh-blobs protocol for a store, with linked files answered from disk.
#[derive(Clone, Debug)]
pub struct FilesProtocol {
    store: Store,
    events: EventSender,
    linked: LinkedSet,
    outboards: PathBuf,
}

impl ProtocolHandler for FilesProtocol {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let connection_id = conn.stable_id() as u64;
        if let Err(cause) = self
            .events
            .client_connected(|| ClientConnected { connection_id, endpoint_id: Some(conn.remote_id()) })
            .await
        {
            conn.close(cause.code(), cause.reason());
            return Ok(());
        }
        while let Ok(pair) = StreamPair::accept(&conn, self.events.clone()).await {
            let this = self.clone();
            tokio::spawn(async move {
                if let Err(e) = this.dispatch(pair).await {
                    tracing::debug!(error = %e, "blob stream failed");
                }
            });
        }
        self.events.connection_closed(|| ConnectionClosed { connection_id }).await.ok();
        Ok(())
    }

    async fn shutdown(&self) {
        if let Err(cause) = self.store.shutdown().await {
            tracing::error!(?cause, "error shutting down store");
        }
    }
}

impl FilesProtocol {
    async fn dispatch(&self, mut pair: StreamPair) -> std::result::Result<(), String> {
        let request = pair.read_request().await.map_err(|e| e.to_string())?;
        let store = self.store.clone();
        match request {
            Request::Get(request) => match self.linked.get(&request.hash) {
                Some(linked) => self.serve_linked(pair, request, linked).await,
                None => iroh_blobs::provider::handle_get(pair, store, request).await.map_err(|e| e.to_string()),
            },
            Request::Observe(request) => match self.linked.get(&request.hash) {
                Some(linked) => self.observe_linked(pair, request, linked).await,
                None => iroh_blobs::provider::handle_observe(pair, store, request).await.map_err(|e| e.to_string()),
            },
            Request::GetMany(request) => iroh_blobs::provider::handle_get_many(pair, store, request).await.map_err(|e| e.to_string()),
            Request::Push(request) => iroh_blobs::provider::handle_push(pair, store, request).await.map_err(|e| e.to_string()),
            _ => Ok(()),
        }
    }

    /// Answers a GET for a linked blob: its size, then the bao encoding of the ranges asked for, each chunk group
    /// checked against the outboard before it is written.
    async fn serve_linked(&self, pair: StreamPair, request: iroh_blobs::protocol::GetRequest, linked: Linked) -> std::result::Result<(), String> {
        let tracker = pair.get_request(|| request.clone()).await.map_err(|e| e.to_string())?;
        let mut writer = pair.into_writer(tracker).await.map_err(|e| e.to_string())?;
        let Some((0, ranges)) = request.ranges.as_single().map(|(o, r)| (o, r.clone())) else {
            writer.inner.reset(iroh_blobs::protocol::ERR_INTERNAL).ok();
            return Err("only single blob requests are served from linked files".into());
        };
        // A path that became a link since it was linked is not followed, and a changed file is not read.
        let is_file = std::fs::symlink_metadata(&linked.path).is_ok_and(|m| m.is_file());
        if !is_file || modified(&linked.path) != linked.modified {
            writer.inner.reset(iroh_blobs::protocol::ERR_INTERNAL).ok();
            return Err("the linked file changed on disk".into());
        }
        let hash = request.hash;
        let outboard_path = self.outboards.join(format!("{}.obao", hash.to_hex()));
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
        let encoder = tokio::task::spawn_blocking(move || encode_linked(&linked, hash, &outboard_path, &ranges, tx));
        writer.inner.write_all(&linked_size_prefix(&request, &self.linked)).await.map_err(|e| e.to_string())?;
        while let Some(bytes) = rx.recv().await {
            writer.inner.write_all(&bytes).await.map_err(|e| e.to_string())?;
        }
        let outcome = encoder.await.map_err(|e| e.to_string()).and_then(|r| r);
        match outcome {
            Ok(()) => writer.inner.finish().map_err(|e| e.to_string()),
            Err(e) => {
                // A mismatch or a read error: nothing past the last good chunk group was sent.
                writer.inner.reset(iroh_blobs::protocol::ERR_INTERNAL).ok();
                Err(e)
            }
        }
    }

    /// Answers an observe for a linked blob: all of it is here.
    async fn observe_linked(&self, pair: StreamPair, request: iroh_blobs::protocol::ObserveRequest, linked: Linked) -> std::result::Result<(), String> {
        let tracker = pair.observe_request(|| request.clone()).await.map_err(|e| e.to_string())?;
        let mut writer = pair.into_writer(tracker).await.map_err(|e| e.to_string())?;
        let chunks = linked.cipher_size.div_ceil(1024);
        let item = iroh_blobs::protocol::ObserveItem { size: linked.cipher_size, ranges: ChunkRanges::from(bao_tree::ChunkNum(0)..bao_tree::ChunkNum(chunks)) };
        let body = postcard::to_allocvec(&item).map_err(|e| e.to_string())?;
        let mut framed = Vec::with_capacity(body.len() + 9);
        let mut len = body.len() as u64;
        loop {
            let byte = (len & 0x7f) as u8;
            len >>= 7;
            framed.push(if len == 0 { byte } else { byte | 0x80 });
            if len == 0 {
                break;
            }
        }
        framed.extend_from_slice(&body);
        writer.inner.write_all(&framed).await.map_err(|e| e.to_string())?;
        writer.inner.finish().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// The 8-byte little-endian size iroh-blobs sends before a blob's encoding.
fn linked_size_prefix(request: &iroh_blobs::protocol::GetRequest, linked: &LinkedSet) -> [u8; 8] {
    linked.get(&request.hash).map_or(0, |l| l.cipher_size).to_le_bytes()
}

fn encode_linked(linked: &Linked, hash: Hash, outboard_path: &Path, ranges: &ChunkRanges, tx: mpsc::Sender<Vec<u8>>) -> std::result::Result<(), String> {
    let data = CipherAt {
        file: File::open(&linked.path).map_err(|e| e.to_string())?,
        cipher: crypt::cipher_for(&linked.key).map_err(|e| e.to_string())?,
        key: linked.key.clone(),
        chunks: crypt::chunk_count(linked.size),
        cache: RefCell::new(None),
    };
    let outboard = PostOrderOutboard {
        root: blake3::Hash::from_bytes(*hash.as_bytes()),
        tree: BaoTree::new(linked.cipher_size, BLOCK_SIZE),
        data: File::open(outboard_path).map_err(|e| e.to_string())?,
    };
    let sink = ChannelSink { tx, buf: Vec::with_capacity(256 * 1024) };
    encode_ranges_validated(data, outboard, ranges, sink).map_err(|e| e.to_string())
}

/// Sends what the encoder writes to the stream task, in pieces of at most 256 KiB.
struct ChannelSink {
    tx: mpsc::Sender<Vec<u8>>,
    buf: Vec<u8>,
}

impl io::Write for ChannelSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= 256 * 1024 {
            self.flush()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let out = std::mem::take(&mut self.buf);
            self.tx.blocking_send(out).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the stream closed"))?;
        }
        Ok(())
    }
}

impl Drop for ChannelSink {
    fn drop(&mut self) {
        let _ = io::Write::flush(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| ((i as u32).wrapping_mul(2654435761) >> 13) as u8).collect()
    }

    /// Builds the outboard of `plain` under `key` as `link_file` does, and returns the root and the outboard.
    fn outboard_of(path: &Path, key: &FileKey, size: u64) -> (Hash, Vec<u8>) {
        let mut reader = CipherReader {
            file: BufReader::new(File::open(path).unwrap()),
            cipher: crypt::cipher_for(key).unwrap(),
            key: key.clone(),
            index: 0,
            chunks: crypt::chunk_count(size),
            out: Vec::new(),
            at: 0,
            hasher: blake3::Hasher::new(),
        };
        let mut ob = Vec::new();
        let root = outboard_post_order(&mut reader, BaoTree::new(cipher_len(size), BLOCK_SIZE), &mut ob).unwrap();
        (Hash::from(root), ob)
    }

    #[test]
    fn the_outboard_root_is_the_ciphertext_hash_and_a_changed_file_sends_nothing_past_the_change() {
        let dir = std::env::temp_dir().join(format!("dissent-inplace-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = sample(5 * CHUNK_SIZE + 123);
        let path = dir.join("file.bin");
        std::fs::write(&path, &plain).unwrap();
        let key = FileKey::generate();
        let size = plain.len() as u64;
        let (hash, ob) = outboard_of(&path, &key, size);
        // The root equals the hash of the whole ciphertext, as the streaming encryptor makes it.
        let (cipher, _) = {
            let mut out = Vec::new();
            let sealed = crypt::encrypt_stream_with(&mut plain.as_slice(), &mut out, u64::MAX, key.clone(), None).unwrap();
            (out, sealed)
        };
        assert_eq!(hash, Hash::new(&cipher));
        let ob_path = dir.join("file.obao");
        std::fs::write(&ob_path, &ob).unwrap();
        let linked = Linked { path: path.clone(), key: key.clone(), size, cipher_size: cipher_len(size), modified: None };
        let all = ChunkRanges::all();

        let (tx, mut rx) = mpsc::channel(64);
        encode_linked(&linked, hash, &ob_path, &all, tx).unwrap();
        let mut encoded = Vec::new();
        while let Ok(b) = rx.try_recv() {
            encoded.extend(b);
        }
        assert!(encoded.len() as u64 > cipher_len(size), "the whole encoding, hash pairs included");

        // The third chunk changes on disk: encoding fails, and what was sent ends before it.
        let mut changed = plain.clone();
        changed[2 * CHUNK_SIZE + 10] ^= 1;
        std::fs::write(&path, &changed).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        assert!(encode_linked(&linked, hash, &ob_path, &all, tx).is_err());
        let mut sent = Vec::new();
        while let Ok(b) = rx.try_recv() {
            sent.extend(b);
        }
        assert!((sent.len() as u64) < 2 * FRAME + 64 * 64, "stopped before the changed chunk: {} bytes", sent.len());
        // Nothing sent holds the changed chunk's new ciphertext.
        let cipher_changed = crypt::seal_chunk(&crypt::cipher_for(&key).unwrap(), &key, 2, false, &changed[2 * CHUNK_SIZE..3 * CHUNK_SIZE]).unwrap();
        assert!(!sent.windows(64).any(|w| w == &cipher_changed[..64]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
