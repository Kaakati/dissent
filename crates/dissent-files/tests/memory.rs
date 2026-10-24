//! Peak-memory check for a 300 MB transfer.
//!
//! This file is its own test binary, so the counting allocator below sees only this test. The
//! allocator counts live heap bytes and records the peak. The test asserts that adding and
//! fetching the file grows the peak by far less than the file size.
//!
//! The test is `#[ignore]` because the encryption is slow in debug builds. Run it with:
//! `cargo test -p dissent-files --release --test memory -- --ignored --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use dissent_files::{Files, Input, ALPN};
use iroh::protocol::Router;
use iroh::{endpoint::presets, Endpoint, RelayMode};

const MIB: usize = 1024 * 1024;
const FILE_SIZE: usize = 300 * MIB;
/// Most the peak may grow over the starting heap. Far below the file size, so a whole-file buffer
/// would fail the check.
const PEAK_CEILING: usize = 64 * MIB;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    let live = LIVE.fetch_add(bytes, SeqCst) + bytes;
    PEAK.fetch_max(live, SeqCst);
}

fn shrank(bytes: usize) {
    LIVE.fetch_sub(bytes, SeqCst);
}

// SAFETY: every call forwards to the system allocator unchanged. The only addition is the atomic
// bookkeeping, which never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        shrank(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            if new_size > layout.size() {
                grew(new_size - layout.size());
            } else {
                shrank(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dissent-mem-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes `size` bytes of a repeating pattern, one 1 MiB buffer at a time.
fn write_generated(path: &Path, size: usize) {
    let pattern: Vec<u8> = (0..MIB)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8)
        .collect();
    let mut out = BufWriter::new(File::create(path).unwrap());
    let mut written = 0;
    while written < size {
        let n = (size - written).min(MIB);
        out.write_all(&pattern[..n]).unwrap();
        written += n;
    }
    out.flush().unwrap();
}

fn blake3_file(path: &Path) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(File::open(path).unwrap()).unwrap();
    hasher.finalize()
}

async fn loopback_endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .unwrap()
        .bind()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow in debug builds; run in release, see the module docs"]
async fn three_hundred_mb_transfer_keeps_peak_memory_low() {
    let root = Dir::new();
    let src = root.0.join("generated.bin");
    write_generated(&src, FILE_SIZE);
    let source_hash = blake3_file(&src);

    let alice = Files::open(root.0.join("alice")).await.unwrap();
    let bob = Files::open(root.0.join("bob")).await.unwrap();
    let alice_endpoint = loopback_endpoint().await;
    let bob_endpoint = loopback_endpoint().await;
    let _router: Router = Router::builder(alice_endpoint.clone())
        .accept(ALPN, alice.protocol_handler())
        .spawn();

    let baseline = LIVE.load(SeqCst);
    PEAK.store(baseline, SeqCst);

    let fref = alice
        .add(Input::Path {
            path: src.clone(),
            mime: "application/octet-stream".into(),
        })
        .await
        .unwrap();
    let downloads = root.0.join("downloads");
    let out = bob
        .fetch(
            &bob_endpoint,
            &[alice_endpoint.addr()],
            &fref,
            &downloads,
            |_, _| {},
        )
        .await
        .unwrap();

    let peak_growth = PEAK.load(SeqCst).saturating_sub(baseline);
    println!("peak heap growth: {} MiB", peak_growth / MIB);
    assert!(
        peak_growth < PEAK_CEILING,
        "peak heap grew by {} MiB, ceiling {} MiB",
        peak_growth / MIB,
        PEAK_CEILING / MIB
    );

    assert_eq!(fs::metadata(&out).unwrap().len(), FILE_SIZE as u64);
    assert_eq!(blake3_file(&out), source_hash, "content must match");
}
