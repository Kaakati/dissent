//! Downloads from many providers at once. The missing ciphertext is cut into slices; each provider works through the
//! slices one at a time, and a slice a provider fails on goes back for another. Every chunk is verified against the
//! blob's hash as it arrives, as in a single-provider fetch, so a bad provider costs only its own slice. See
//! `docs/design/14-file-tracker.md`, "Downloads".

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use futures_lite::stream::StreamExt;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest, ObserveRequest};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{Hash, HashAndFormat};
use tokio::sync::mpsc;

use crate::error::{FilesError, Result};
use crate::file_ref::FileRef;
use crate::store::{store_err, Files, Timeouts, DISK_MARGIN};
use crate::tally::{SourceCount, PIECE_BYTES};

/// A slice: 4 MiB of ciphertext, in 1 KiB chunks. A multiple of the store's 16 KiB chunk groups.
const SLICE_CHUNKS: u64 = 4096;
/// Most providers asked at once.
pub const MAX_PARALLEL: usize = 8;
/// A provider that fails this many slices is dropped from the download. A busy answer is not a failure.
const MAX_FAILURES: u32 = 2;
/// The pause after a busy answer, doubling up to the longest, and back to the first after a slice comes in.
const BUSY_FIRST: std::time::Duration = std::time::Duration::from_millis(250);
const BUSY_LONGEST: std::time::Duration = std::time::Duration::from_secs(4);
/// The shortest time between two reports of a download. Bytes arrive per 16 KiB, thousands of times a second on a fast
/// link, and each report takes a lock and may reach the UI.
const REPORT_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// A shared download cap: bytes per second for every swarm download together, or none.
#[derive(Default)]
pub(crate) struct DownloadCap {
    bps: std::sync::atomic::AtomicU64,
    /// When the bucket was last filled, and what it holds.
    bucket: Mutex<Option<(std::time::Instant, f64)>>,
}

impl DownloadCap {
    /// How long to wait after receiving `bytes`, under the cap. Zero without one.
    fn wait_after(&self, bytes: u64) -> std::time::Duration {
        let bps = self.bps.load(std::sync::atomic::Ordering::Relaxed);
        if bps == 0 {
            return std::time::Duration::ZERO;
        }
        let mut bucket = self.bucket.lock().unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        let (at, level) = bucket.unwrap_or((now, bps as f64));
        // The bucket holds at most one second of bytes.
        let level = (level + now.duration_since(at).as_secs_f64() * bps as f64).min(bps as f64) - bytes as f64;
        *bucket = Some((now, level));
        if level >= 0.0 {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_secs_f64(-level / bps as f64)
        }
    }
}

enum Event {
    /// A provider delivered `bytes` more verified bytes.
    Bytes { provider: usize, bytes: u64 },
    /// A provider's upload slots are full. It is asked again after a pause.
    Busy { provider: usize },
    /// A provider failed a slice.
    Failed { provider: usize, error: String },
}

/// What a swarm download produced: the saved file, and the providers (by position) that failed at least one slice, a
/// bad chunk included. A receipt gives those no credit.
#[derive(Clone, Debug)]
pub struct SwarmFetch {
    pub path: PathBuf,
    pub failed: Vec<usize>,
}

impl Files {
    /// Caps every swarm download together at `bps` bytes per second, or lifts the cap with `None`.
    /// Makes busy answers count as failures, as a release before upload slots did. For tests.
    #[doc(hidden)]
    pub fn act_as_before_upload_slots(&self) {
        self.busy_retry.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_download_cap(&self, bps: Option<u64>) {
        self.download_cap.bps.store(bps.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    }

    /// Like [`Files::fetch_tallied`], asking up to [`MAX_PARALLEL`] providers at once, each for its own slices of the
    /// missing ciphertext. A provider that sends a bad chunk fails its slice; the slice goes back for another provider,
    /// and the bytes that provider delivered before the bad one stay credited to it. `sources` gets the counts in the
    /// order the providers were given.
    pub async fn fetch_swarm(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        fref: &FileRef,
        dest_dir: &Path,
        mut progress: impl FnMut(u64, u64),
        mut sources: impl FnMut(&[SourceCount]) + Send,
    ) -> Result<SwarmFetch> {
        fref.check(self.size_limit)?;
        self.check_space(fref.cipher_size + fref.size + DISK_MARGIN).await?;
        let hash = Hash::from_bytes(fref.hash);
        let hf = HashAndFormat::raw(hash);
        let keep = self.store.tags().temp_tag(hf).await.map_err(store_err)?;
        self.prune_partials().await?;
        let mut failed = Vec::new();
        if !self.is_complete(hf).await? {
            self.hold_partial(hash).await?;
            let budget = self.timeouts.overall(fref.cipher_size);
            let download = self.download_parallel(endpoint, from, hash, fref.cipher_size, &mut progress, &mut sources);
            match tokio::time::timeout(budget, download).await {
                Ok(result) => failed = result?,
                Err(_) => return Err(FilesError::Fetch(format!("timed out after {} s", budget.as_secs()))),
            }
            self.release_partial(hash).await?;
        }
        let local = self.store.remote().local(hf).await.map_err(store_err)?;
        if local.local_bytes() != fref.cipher_size {
            return Err(FilesError::Fetch(format!("stored {} bytes, declared {}", local.local_bytes(), fref.cipher_size)));
        }
        progress(fref.cipher_size, fref.cipher_size);
        let result = self.decrypt_to_dest(hash, fref, dest_dir).await;
        drop(keep);
        result.map(|path| SwarmFetch { path, failed })
    }

    async fn download_parallel(
        &self,
        endpoint: &Endpoint,
        from: &[EndpointAddr],
        hash: Hash,
        cipher_size: u64,
        progress: &mut impl FnMut(u64, u64),
        sources: &mut impl FnMut(&[SourceCount]),
    ) -> Result<Vec<usize>> {
        if from.is_empty() {
            return Err(FilesError::NoProviders);
        }
        let base = self.local_bytes(HashAndFormat::raw(hash)).await?;
        progress(base, cipher_size);
        let missing = missing(&self.store, hash).await?;
        let chunks = cipher_size.div_ceil(1024);
        let mut slices = VecDeque::new();
        let mut start = 0;
        while start < chunks {
            let end = (start + SLICE_CHUNKS).min(chunks);
            let slice = ChunkRanges::chunks(start..end) & missing.clone();
            if !slice.is_empty() {
                slices.push_back(slice);
            }
            start = end;
        }
        let queue = Arc::new(Mutex::new(slices));
        let (tx, mut rx) = mpsc::channel::<Event>(256);
        let mut workers = tokio::task::JoinSet::new();
        for (provider, addr) in from.iter().take(MAX_PARALLEL).enumerate() {
            let worker = Worker {
                cap: self.download_cap.clone(),
                busy_retry: self.busy_retry.load(std::sync::atomic::Ordering::Relaxed),
                store: self.store.clone(),
                timeouts: self.timeouts,
                endpoint: endpoint.clone(),
                addr: addr.clone(),
                hash,
                provider,
                queue: queue.clone(),
                tx: tx.clone(),
            };
            workers.spawn(worker.run());
        }
        drop(tx);
        let mut counts: Vec<SourceCount> = (0..from.len()).map(|index| SourceCount { index, bytes: 0, pieces: 0, busy: false }).collect();
        let mut held = base;
        let mut last_error = String::from("no provider had the blob");
        let mut failed: Vec<usize> = Vec::new();
        let mut reported = std::time::Instant::now();
        while let Some(event) = rx.recv().await {
            match event {
                Event::Bytes { provider, bytes } => {
                    held = (held + bytes).min(cipher_size);
                    let c = &mut counts[provider];
                    c.busy = false;
                    c.bytes += bytes;
                    c.pieces = c.bytes / PIECE_BYTES;
                    if reported.elapsed() >= REPORT_EVERY {
                        reported = std::time::Instant::now();
                        progress(held, cipher_size);
                        sources(&counts);
                    }
                }
                Event::Busy { provider } => {
                    if !counts[provider].busy {
                        counts[provider].busy = true;
                        sources(&counts);
                    }
                }
                Event::Failed { provider, error } => {
                    if !failed.contains(&provider) {
                        failed.push(provider);
                    }
                    last_error = error;
                }
            }
        }
        while workers.join_next().await.is_some() {}
        // The last report is always made: receipts credit these counts.
        progress(held, cipher_size);
        sources(&counts);
        if self.is_complete(HashAndFormat::raw(hash)).await? {
            Ok(failed)
        } else {
            Err(FilesError::Fetch(last_error))
        }
    }
}

impl Files {
    /// The whole 64 KiB pieces of `hash` this store holds, as (start, end) runs.
    pub async fn held_pieces(&self, hash: [u8; 32], cipher_size: u64) -> Result<Vec<(u64, u64)>> {
        let missing = missing(&self.store, Hash::from_bytes(hash)).await?;
        Ok(held_runs(&missing, cipher_size))
    }

    /// Which 64 KiB pieces of `hash` the provider at `addr` holds, as (start, end) runs, from its first answer to an
    /// observe request. A piece counts as held only when all of it is.
    pub async fn piece_map(&self, endpoint: &Endpoint, addr: EndpointAddr, hash: [u8; 32], cipher_size: u64) -> Result<Vec<(u64, u64)>> {
        let timeouts = self.timeouts;
        let conn = tokio::time::timeout(timeouts.connect, endpoint.connect(addr, iroh_blobs::ALPN))
            .await
            .map_err(|_| FilesError::Fetch("connect timed out".to_owned()))?
            .map_err(|e| FilesError::Fetch(e.to_string()))?;
        let closer = conn.clone();
        let stream = self.store.remote().observe(conn, ObserveRequest::new(Hash::from_bytes(hash)));
        let mut stream = std::pin::pin!(stream);
        let first = tokio::time::timeout(timeouts.stall, stream.next()).await;
        closer.close(0u32.into(), b"done");
        let bitfield = match first {
            Ok(Some(Ok(bitfield))) => bitfield,
            Ok(Some(Err(e))) => return Err(FilesError::Fetch(e.to_string())),
            Ok(None) => return Err(FilesError::Fetch("no answer".to_owned())),
            Err(_) => return Err(FilesError::Fetch("no answer in time".to_owned())),
        };
        let chunks = cipher_size.div_ceil(1024);
        Ok(held_runs(&(ChunkRanges::chunks(0..chunks) - bitfield.ranges), cipher_size))
    }
}

/// The runs of whole 64 KiB pieces, as (start, end), of a blob of `cipher_size` bytes whose `missing` 1 KiB chunks are
/// not held. Worked out from the missing ranges' edges, so a 500 GB file's 7.6 million pieces cost nothing to count.
pub(crate) fn held_runs(missing: &ChunkRanges, cipher_size: u64) -> Vec<(u64, u64)> {
    let chunks = cipher_size.div_ceil(1024);
    let per_piece = PIECE_BYTES / 1024;
    let total = crate::tally::pieces_of(cipher_size);
    let edges = missing.boundaries();
    // The pieces any missing chunk falls in, merged.
    let mut touched: Vec<(u64, u64)> = Vec::new();
    for pair in edges.chunks(2) {
        let start = pair[0].0.min(chunks);
        let end = pair.get(1).map_or(chunks, |e| e.0.min(chunks));
        let (first, last) = (start / per_piece, end.div_ceil(per_piece).min(total));
        if first >= last {
            continue;
        }
        match touched.last_mut() {
            Some((_, e)) if *e >= first => *e = (*e).max(last),
            _ => touched.push((first, last)),
        }
    }
    let mut runs = Vec::new();
    let mut at = 0;
    for (first, last) in touched {
        if first > at {
            runs.push((at, first));
        }
        at = at.max(last);
    }
    if at < total {
        runs.push((at, total));
    }
    runs
}


/// The missing ranges of `hash`, as `store` sees them now.
async fn missing(store: &FsStore, hash: Hash) -> Result<ChunkRanges> {
    let local = store.remote().local(HashAndFormat::raw(hash)).await.map_err(store_err)?;
    let missing = local.missing();
    Ok(missing.ranges.as_single().map(|(_, r)| r.clone()).unwrap_or_else(ChunkRanges::empty))
}

/// Why one slice did not come from a provider.
enum SliceError {
    /// Its upload slots are full.
    Busy,
    Failed(FilesError),
}

struct Worker {
    cap: Arc<DownloadCap>,
    busy_retry: bool,
    store: FsStore,
    timeouts: Timeouts,
    endpoint: Endpoint,
    addr: EndpointAddr,
    hash: Hash,
    provider: usize,
    queue: Arc<Mutex<VecDeque<ChunkRanges>>>,
    tx: mpsc::Sender<Event>,
}

impl Worker {
    async fn run(self) {
        let timeouts = self.timeouts;
        let conn = match tokio::time::timeout(timeouts.connect, self.endpoint.connect(self.addr.clone(), iroh_blobs::ALPN)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return self.fail(e.to_string()).await,
            Err(_) => return self.fail("connect timed out".into()).await,
        };
        let mut failures = 0;
        let mut pause = BUSY_FIRST;
        loop {
            let Some(slice) = self.queue.lock().unwrap_or_else(PoisonError::into_inner).pop_front() else {
                break;
            };
            // Another provider may have filled part of it since, after a failure.
            let wanted = match missing(&self.store, self.hash).await {
                Ok(missing) => slice.clone() & missing,
                Err(e) => return self.fail(e.to_string()).await,
            };
            if wanted.is_empty() {
                continue;
            }
            match self.get(&conn, wanted).await {
                Ok(()) => pause = BUSY_FIRST,
                // Others may take the slice meanwhile; this provider is asked again after the pause, for whatever is left.
                Err(SliceError::Busy) if self.busy_retry => {
                    self.queue.lock().unwrap_or_else(PoisonError::into_inner).push_back(slice);
                    let _ = self.tx.send(Event::Busy { provider: self.provider }).await;
                    tokio::time::sleep(pause).await;
                    pause = (pause * 2).min(BUSY_LONGEST);
                }
                Err(e) => {
                    let e = match e {
                        SliceError::Busy => FilesError::Fetch("the provider's upload slots are full".to_owned()),
                        SliceError::Failed(e) => e,
                    };
                    self.queue.lock().unwrap_or_else(PoisonError::into_inner).push_back(slice);
                    failures += 1;
                    let _ = self.tx.send(Event::Failed { provider: self.provider, error: e.to_string() }).await;
                    if failures >= MAX_FAILURES {
                        break;
                    }
                }
            }
        }
        conn.close(0u32.into(), b"done");
    }

    /// Fetches one slice. A provider that sends more than a slice holds is cut off.
    async fn get(&self, conn: &iroh::endpoint::Connection, ranges: ChunkRanges) -> std::result::Result<(), SliceError> {
        let limit = SLICE_CHUNKS * 1024;
        let timeouts = self.timeouts;
        let request = GetRequest::blob_ranges(self.hash, ranges);
        let stream = self.store.remote().execute_get(conn.clone(), request).stream();
        let mut stream = std::pin::pin!(stream);
        let mut last = 0u64;
        loop {
            match tokio::time::timeout(timeouts.stall, stream.next()).await {
                Err(_) => return Err(SliceError::Failed(FilesError::Fetch(format!("no data for {} s", timeouts.stall.as_secs())))),
                Ok(None) => return Err(SliceError::Failed(FilesError::Fetch("stream ended early".to_owned()))),
                Ok(Some(GetProgressItem::Progress(done))) if done > limit => {
                    return Err(SliceError::Failed(FilesError::Fetch("peer sent more than the slice".to_owned())));
                }
                Ok(Some(GetProgressItem::Progress(done))) => {
                    if done > last {
                        let _ = self.tx.send(Event::Bytes { provider: self.provider, bytes: done - last }).await;
                        let wait = self.cap.wait_after(done - last);
                        last = done;
                        if !wait.is_zero() {
                            tokio::time::sleep(wait).await;
                        }
                    }
                }
                Ok(Some(GetProgressItem::Done(_))) => return Ok(()),
                Ok(Some(GetProgressItem::Error(e))) if e.iroh_error_code() == Some(iroh_blobs::protocol::ERR_LIMIT) => {
                    return Err(SliceError::Busy);
                }
                Ok(Some(GetProgressItem::Error(e))) => return Err(SliceError::Failed(FilesError::Fetch(e.to_string()))),
            }
        }
    }

    async fn fail(&self, error: String) {
        let _ = self.tx.send(Event::Failed { provider: self.provider, error }).await;
    }
}

#[cfg(test)]
mod held_runs_tests {
    use super::*;

    const PIECE_CHUNKS: u64 = PIECE_BYTES / 1024;

    #[test]
    fn whole_pieces_are_held_only_where_no_chunk_is_missing() {
        let cipher = 10 * PIECE_BYTES;
        assert_eq!(held_runs(&ChunkRanges::empty(), cipher), vec![(0, 10)]);
        assert_eq!(held_runs(&ChunkRanges::all(), cipher), vec![]);
        // One chunk missing in piece 3, and everything from piece 7 on.
        let missing = ChunkRanges::chunks(3 * PIECE_CHUNKS + 5..3 * PIECE_CHUNKS + 6) | ChunkRanges::chunks(7 * PIECE_CHUNKS..);
        assert_eq!(held_runs(&missing, cipher), vec![(0, 3), (4, 7)]);
    }

    #[test]
    fn a_short_last_piece_and_a_huge_blob_count_right() {
        let cipher = 2 * PIECE_BYTES + 100;
        assert_eq!(held_runs(&ChunkRanges::empty(), cipher), vec![(0, 3)]);
        let huge = 500 * 1024 * 1024 * 1024;
        let missing = ChunkRanges::chunks(PIECE_CHUNKS..2 * PIECE_CHUNKS);
        assert_eq!(held_runs(&missing, huge), vec![(0, 1), (2, crate::tally::pieces_of(huge))]);
    }
}
