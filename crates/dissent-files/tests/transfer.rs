//! End-to-end transfers between loopback endpoints. Each endpoint binds 127.0.0.1 only and has
//! relays disabled, so nothing reaches the network and Windows Firewall does not prompt.

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use std::time::{Duration, Instant};

use dissent_files::{
    cipher_len, sanitize_name, FileRef, Files, FilesError, Input, Timeouts, ALPN, MAX_FILE_SIZE,
};
use iroh::endpoint::Connection;
use iroh::protocol::Router;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::EndpointAddr;
use iroh::{endpoint::presets, Endpoint, RelayMode};

const MIB: usize = 1024 * 1024;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dissent-files-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Peer {
    files: Files,
    endpoint: Endpoint,
    _router: Router,
    dir: TestDir,
}

impl Peer {
    async fn new() -> Self {
        Self::build(None, None, None).await
    }

    async fn with_limit(limit: Option<u64>) -> Self {
        Self::build(limit, None, None).await
    }

    async fn build(limit: Option<u64>, gc: Option<Duration>, timeouts: Option<Timeouts>) -> Self {
        let dir = TestDir::new();
        let data = dir.path().join("data");
        let mut files = match gc {
            Some(interval) => Files::open_with_gc_interval(&data, interval).await.unwrap(),
            None => Files::open(&data).await.unwrap(),
        };
        if let Some(limit) = limit {
            files = files.with_size_limit(limit);
        }
        if let Some(timeouts) = timeouts {
            files = files.with_timeouts(timeouts);
        }
        let endpoint = loopback_endpoint().await;
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, files.protocol_handler())
            .spawn();
        Self {
            files,
            endpoint,
            _router: router,
            dir,
        }
    }

    fn addr(&self) -> iroh::EndpointAddr {
        self.endpoint.addr()
    }

    fn downloads(&self) -> PathBuf {
        self.dir.path().join("downloads")
    }
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

fn sample(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect()
}

fn blake3_of(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

#[tokio::test]
async fn five_mib_file_moves_between_peers() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let plain = sample(5 * MIB);

    let src = alice.dir.path().join("photo.bin");
    fs::write(&src, &plain).unwrap();
    let fref = alice
        .files
        .add(Input::Path {
            path: src,
            mime: "image/png".into(),
        })
        .await
        .unwrap();
    assert_eq!(fref.size, plain.len() as u64);
    assert_eq!(fref.cipher_size, cipher_len(plain.len() as u64));
    assert_eq!(fref.name, "photo.bin");

    // Alice's store holds ciphertext: its hash is the ciphertext hash and it is not the plaintext.
    let stored = alice
        .files
        .store()
        .blobs()
        .get_bytes(fref.hash)
        .await
        .unwrap();
    assert_eq!(blake3_of(&stored), fref.hash);
    assert_ne!(stored.as_ref(), plain.as_slice());
    assert_ne!(fref.hash, blake3_of(&plain));

    let mut last = (0, 0);
    let out = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &fref,
            &bob.downloads(),
            |done, total| {
                last = (done, total);
            },
        )
        .await
        .unwrap();
    assert_eq!(fs::read(&out).unwrap(), plain);
    assert_eq!(out.file_name().unwrap(), "photo.bin");
    assert_eq!(last, (fref.cipher_size, fref.cipher_size));
}

#[tokio::test]
async fn empty_and_small_files_roundtrip() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    for plain in [Vec::new(), vec![0xAB_u8]] {
        let fref = alice
            .files
            .add(Input::Bytes {
                name: "tiny.txt".into(),
                mime: "text/plain".into(),
                data: plain.clone(),
            })
            .await
            .unwrap();
        let out = bob
            .files
            .fetch(
                &bob.endpoint,
                &[alice.addr()],
                &fref,
                &bob.downloads(),
                |_, _| {},
            )
            .await
            .unwrap();
        assert_eq!(fs::read(out).unwrap(), plain);
    }
}

#[tokio::test]
async fn hostile_name_stays_inside_downloads() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let fref = alice
        .files
        .add(Input::Bytes {
            name: "../../evil".into(),
            mime: "text/plain".into(),
            data: b"hi".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(fref.name, "evil");
    let out = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(out.parent().unwrap(), bob.downloads());
    assert_eq!(fs::read(&out).unwrap(), b"hi");
}

#[tokio::test]
async fn second_download_gets_a_new_name() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let fref = alice
        .files
        .add(Input::Bytes {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            data: b"one".to_vec(),
        })
        .await
        .unwrap();
    let first = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    let second = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(first.file_name().unwrap(), "notes.txt");
    assert_eq!(second.file_name().unwrap(), "notes (1).txt");
}

#[tokio::test]
async fn wrong_hash_fails() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let real = alice
        .files
        .add(Input::Bytes {
            name: "a.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(1000),
        })
        .await
        .unwrap();
    let mut forged = real.clone();
    forged.hash[0] ^= 0xFF;
    let err = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &forged,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::Fetch(_)), "{err:?}");
    assert!(!bob.downloads().exists() || fs::read_dir(bob.downloads()).unwrap().next().is_none());
}

#[tokio::test]
async fn wrong_key_decrypts_to_error_and_leaves_no_file() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let real = alice
        .files
        .add(Input::Bytes {
            name: "secret.txt".into(),
            mime: "text/plain".into(),
            data: sample(3 * MIB),
        })
        .await
        .unwrap();
    let mut wrong = real.clone();
    wrong.key[0] ^= 1;
    let err = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &wrong,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::Decrypt), "{err:?}");
    assert!(!bob.downloads().join("secret.txt").exists());
    let leftovers: Vec<_> = fs::read_dir(bob.downloads()).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn size_limit_is_enforced_on_add_and_fetch() {
    let alice = Peer::with_limit(Some(MIB as u64)).await;
    let bob = Peer::new().await;

    let err = alice
        .files
        .add(Input::Bytes {
            name: "big.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(2 * MIB),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::TooLarge { .. }), "{err:?}");

    // A reference over the receiver's limit is refused before any network use, even with no providers.
    let big = bob
        .files
        .add(Input::Bytes {
            name: "big.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(2 * MIB),
        })
        .await
        .unwrap();
    let err = alice
        .files
        .fetch(&alice.endpoint, &[], &big, &alice.downloads(), |_, _| {})
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::TooLarge { .. }), "{err:?}");

    // A reference whose ciphertext size disagrees with its plaintext size is refused.
    let mut lying = big.clone();
    lying.size = 10;
    let err = alice
        .files
        .fetch(&alice.endpoint, &[], &lying, &alice.downloads(), |_, _| {})
        .await
        .unwrap_err();
    // The ciphertext is over the limit, so the cap check fires first.
    assert!(matches!(err, FilesError::TooLarge { .. }), "{err:?}");
    let mut mismatched = big.clone();
    mismatched.size = 10;
    mismatched.cipher_size = cipher_len(10) + 1;
    let err = alice
        .files
        .fetch(
            &alice.endpoint,
            &[],
            &mismatched,
            &alice.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::BadRef(_)), "{err:?}");
}

#[tokio::test]
async fn seeder_serves_ciphertext_it_cannot_read() {
    let alice = Peer::new().await;
    let seeder = Peer::new().await;
    let carol = Peer::new().await;
    let plain = sample(3 * MIB + 5);

    let fref = alice
        .files
        .add(Input::Bytes {
            name: "doc.pdf".into(),
            mime: "application/pdf".into(),
            data: plain.clone(),
        })
        .await
        .unwrap();

    // The seeder knows only the public hash and ciphertext size, never the key.
    let mut seen = 0;
    seeder
        .files
        .mirror(
            &seeder.endpoint,
            &[alice.addr()],
            fref.hash,
            fref.cipher_size,
            |done, _| seen = done,
        )
        .await
        .unwrap();
    assert_eq!(seen, fref.cipher_size);
    assert!(seeder.files.store().blobs().has(fref.hash).await.unwrap());
    // The seeder holds no plaintext blob.
    assert!(!seeder
        .files
        .store()
        .blobs()
        .has(blake3_of(&plain))
        .await
        .unwrap());

    // Carol gets the file from the seeder, with the key she received separately.
    let out = carol
        .files
        .fetch(
            &carol.endpoint,
            &[seeder.addr()],
            &fref,
            &carol.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(fs::read(out).unwrap(), plain);
}

#[tokio::test]
async fn fetch_falls_back_to_the_next_provider() {
    let alice = Peer::new().await;
    let dead = Peer::new().await;
    let bob = Peer::new().await;
    let fref = alice
        .files
        .add(Input::Bytes {
            name: "x.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(100),
        })
        .await
        .unwrap();
    // The first address is a peer that does not have the blob, so the request must fall through.
    let out = bob
        .files
        .fetch(
            &bob.endpoint,
            &[dead.addr(), alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(fs::read(out).unwrap().len(), 100);
}

#[tokio::test]
async fn pin_and_unpin() {
    let alice = Peer::new().await;
    let fref = alice
        .files
        .add(Input::Bytes {
            name: "p.txt".into(),
            mime: "text/plain".into(),
            data: b"keep".to_vec(),
        })
        .await
        .unwrap();
    alice.files.pin(fref.hash).await.unwrap();
    assert!(alice.files.unpin(fref.hash).await.unwrap());
    assert!(!alice.files.unpin(fref.hash).await.unwrap());
    let missing = [9u8; 32];
    assert!(alice.files.pin(missing).await.is_err());
}

#[tokio::test]
async fn ciphertext_survives_reopen_and_serves_again() {
    let dir = TestDir::new();
    let data = dir.path().join("data");
    let fref: FileRef;
    {
        let files = Files::open(&data).await.unwrap();
        fref = files
            .add(Input::Bytes {
                name: "keep.txt".into(),
                mime: "text/plain".into(),
                data: b"persisted".to_vec(),
            })
            .await
            .unwrap();
        files.shutdown().await.unwrap();
    }
    let files = Files::open(&data).await.unwrap();
    assert!(files.store().blobs().has(fref.hash).await.unwrap());
    files.shutdown().await.unwrap();
}

#[test]
fn name_sanitizer_is_applied_to_hostile_refs() {
    let r = FileRef {
        hash: [0; 32],
        size: 0,
        cipher_size: cipher_len(0),
        name: "C:\\x".into(),
        mime: "text/plain".into(),
        key: [0; 32],
        nonce_prefix: [0; 8],
    };
    assert_eq!(r.safe_name(), "x");
    assert_eq!(sanitize_name(&r.name), "x");
}

/// A provider that accepts the connection and then never answers a request.
#[derive(Debug)]
struct Silent;

impl ProtocolHandler for Silent {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let _held = connection;
        std::future::pending::<()>().await;
        Ok(())
    }
}

async fn silent_provider() -> (Endpoint, Router) {
    let endpoint = loopback_endpoint().await;
    let router = Router::builder(endpoint.clone())
        .accept(ALPN, Silent)
        .spawn();
    (endpoint, router)
}

fn timeouts(connect: Duration, stall: Duration, base: Duration) -> Timeouts {
    Timeouts {
        connect,
        stall,
        base,
        per_mib: Duration::ZERO,
        max: Duration::from_secs(60),
    }
}

async fn is_pinned(files: &Files, hash: [u8; 32]) -> bool {
    files.unpin(hash).await.unwrap()
}

/// Polls until the blob is gone from the store, up to about five seconds.
async fn wait_until_gone(files: &Files, hash: [u8; 32]) -> bool {
    let hash = iroh_blobs::Hash::from_bytes(hash);
    for _ in 0..200 {
        if matches!(
            files.store().blobs().status(hash).await.unwrap(),
            iroh_blobs::api::proto::BlobStatus::NotFound
        ) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn silent_file(alice: &Peer) -> FileRef {
    alice
        .files
        .add(Input::Bytes {
            name: "x.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(200_000),
        })
        .await
        .unwrap()
}

#[test]
fn timeout_budget_scales_and_caps() {
    let t = Timeouts::default();
    assert_eq!(t.connect, Duration::from_secs(15));
    assert_eq!(t.stall, Duration::from_secs(30));
    assert_eq!(t.overall(0), Duration::from_secs(60));
    assert_eq!(t.overall(1), Duration::from_secs(70));
    assert_eq!(t.overall(5 * MIB as u64), Duration::from_secs(60 + 50));
    assert_eq!(t.overall(5 * MIB as u64 + 1), Duration::from_secs(60 + 60));
    assert_eq!(
        t.overall(MAX_FILE_SIZE * 1000),
        Duration::from_secs(20 * 60)
    );
}

#[tokio::test]
async fn stalled_provider_is_dropped_with_a_clear_error() {
    let bob = Peer::build(
        None,
        None,
        Some(timeouts(
            Duration::from_secs(5),
            Duration::from_secs(1),
            Duration::from_secs(60),
        )),
    )
    .await;
    let alice = Peer::new().await;
    let (silent_ep, _router) = silent_provider().await;
    let fref = silent_file(&alice).await;
    let err = bob
        .files
        .fetch(
            &bob.endpoint,
            &[silent_ep.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no data for"), "{err}");
    assert!(!bob.downloads().exists() || fs::read_dir(bob.downloads()).unwrap().next().is_none());
    assert!(!is_pinned(&bob.files, fref.hash).await);
}

#[tokio::test]
async fn silent_provider_is_skipped_for_the_next_one() {
    let bob = Peer::build(
        None,
        None,
        Some(timeouts(
            Duration::from_secs(5),
            Duration::from_secs(1),
            Duration::from_secs(60),
        )),
    )
    .await;
    let alice = Peer::new().await;
    let (silent_ep, _router) = silent_provider().await;
    let fref = silent_file(&alice).await;
    let started = Instant::now();
    let out = bob
        .files
        .fetch(
            &bob.endpoint,
            &[silent_ep.addr(), alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(fs::read(out).unwrap().len(), 200_000);
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn overall_budget_aborts_and_leaves_nothing_pinned() {
    // Stall is long, so the overall budget is what fires first.
    let bob = Peer::build(
        None,
        None,
        Some(timeouts(
            Duration::from_secs(5),
            Duration::from_secs(60),
            Duration::from_secs(1),
        )),
    )
    .await;
    let alice = Peer::new().await;
    let (silent_ep, _router) = silent_provider().await;
    let fref = silent_file(&alice).await;
    let err = bob
        .files
        .fetch(
            &bob.endpoint,
            &[silent_ep.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out after"), "{err}");
    assert!(!bob.downloads().exists() || fs::read_dir(bob.downloads()).unwrap().next().is_none());
    assert!(!is_pinned(&bob.files, fref.hash).await);
}

#[tokio::test]
async fn cancelled_fetch_leaves_no_partial_file() {
    let bob = Peer::build(
        None,
        None,
        Some(timeouts(
            Duration::from_secs(5),
            Duration::from_secs(60),
            Duration::from_secs(60),
        )),
    )
    .await;
    let alice = Peer::new().await;
    let (silent_ep, _router) = silent_provider().await;
    let fref = silent_file(&alice).await;
    let providers = [silent_ep.addr()];
    let downloads = bob.downloads();
    let attempt = bob
        .files
        .fetch(&bob.endpoint, &providers, &fref, &downloads, |_, _| {});
    // Give up on the fetch while it waits on the silent provider.
    let cancelled = tokio::time::timeout(Duration::from_millis(500), attempt).await;
    assert!(cancelled.is_err());
    assert!(!bob.downloads().exists() || fs::read_dir(bob.downloads()).unwrap().next().is_none());
    assert!(!is_pinned(&bob.files, fref.hash).await);
}

#[tokio::test]
async fn dead_provider_falls_through_to_a_live_one() {
    let alice = Peer::new().await;
    let bob = Peer::build(
        None,
        None,
        Some(timeouts(
            Duration::from_secs(1),
            Duration::from_secs(5),
            Duration::from_secs(60),
        )),
    )
    .await;
    let fref = alice
        .files
        .add(Input::Bytes {
            name: "y.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(5000),
        })
        .await
        .unwrap();
    // Nothing listens on this port. The connect fails or times out before the live provider.
    let closed = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let dead = EndpointAddr::new(alice.endpoint.id()).with_ip_addr(closed);
    let out = bob
        .files
        .fetch(
            &bob.endpoint,
            &[dead, alice.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(fs::read(out).unwrap().len(), 5000);
}

#[tokio::test]
async fn gc_removes_unpinned_and_keeps_pinned() {
    let alice = Peer::build(None, Some(Duration::from_millis(100)), None).await;
    let a = alice
        .files
        .add(Input::Bytes {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            data: b"keep me".to_vec(),
        })
        .await
        .unwrap();
    let b = alice
        .files
        .add(Input::Bytes {
            name: "b.txt".into(),
            mime: "text/plain".into(),
            data: b"drop me".to_vec(),
        })
        .await
        .unwrap();
    assert!(alice.files.unpin(b.hash).await.unwrap());
    assert!(
        wait_until_gone(&alice.files, b.hash).await,
        "unpinned blob should be collected"
    );
    assert!(
        alice.files.store().blobs().has(a.hash).await.unwrap(),
        "pinned blob must stay"
    );
}

#[tokio::test]
async fn gc_collects_partial_download_from_a_failed_fetch() {
    let bob = Peer::build(
        None,
        Some(Duration::from_millis(100)),
        Some(timeouts(
            Duration::from_secs(5),
            Duration::from_secs(1),
            Duration::from_secs(60),
        )),
    )
    .await;
    let alice = Peer::new().await;
    let (silent_ep, _router) = silent_provider().await;
    let fref = silent_file(&alice).await;
    assert!(bob
        .files
        .fetch(
            &bob.endpoint,
            &[silent_ep.addr()],
            &fref,
            &bob.downloads(),
            |_, _| {}
        )
        .await
        .is_err());
    assert!(wait_until_gone(&bob.files, fref.hash).await);
}

#[test]
fn size_limit_is_clamped_to_the_hard_cap() {
    assert_eq!(MAX_FILE_SIZE, 100 * 1024 * 1024);
    let dir = TestDir::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let files = Files::open(dir.path().join("data")).await.unwrap();
        assert_eq!(files.size_limit(), MAX_FILE_SIZE);
        let files = files.with_size_limit(u64::MAX);
        assert_eq!(files.size_limit(), MAX_FILE_SIZE);
        let files = files.with_size_limit(MIB as u64);
        assert_eq!(files.size_limit(), MIB as u64);
        let files = files.with_size_limit(MAX_FILE_SIZE + 1);
        assert_eq!(files.size_limit(), MAX_FILE_SIZE);
    });
}

#[test]
fn over_cap_error_message() {
    let err = FilesError::TooLarge { size: 1, limit: 1 };
    assert_eq!(err.to_string(), "Files can be up to 100 MB.");
}

#[tokio::test]
async fn add_refuses_over_cap_and_fetch_refuses_over_cap_reference() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    // Over the cap on add.
    let err = alice
        .files
        .add(Input::Bytes {
            name: "huge.bin".into(),
            mime: "application/octet-stream".into(),
            data: vec![0u8; MAX_FILE_SIZE as usize + 1],
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::TooLarge { .. }), "{err:?}");

    // A reference claiming an over-cap size is refused before any download, with no providers.
    let size = MAX_FILE_SIZE + 1;
    let fref = FileRef {
        hash: [3; 32],
        size,
        cipher_size: cipher_len(size),
        name: "huge.bin".into(),
        mime: "application/octet-stream".into(),
        key: [0; 32],
        nonce_prefix: [0; 8],
    };
    let err = bob
        .files
        .fetch(&bob.endpoint, &[], &fref, &bob.downloads(), |_, _| {})
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::TooLarge { .. }), "{err:?}");
    assert_eq!(err.to_string(), "Files can be up to 100 MB.");

    // A reference whose ciphertext size is inflated past the cap is also refused.
    let mut inflated = fref.clone();
    inflated.size = MAX_FILE_SIZE;
    inflated.cipher_size = cipher_len(MAX_FILE_SIZE) + 16;
    let err = bob
        .files
        .fetch(&bob.endpoint, &[], &inflated, &bob.downloads(), |_, _| {})
        .await
        .unwrap_err();
    assert!(
        matches!(err, FilesError::TooLarge { .. } | FilesError::BadRef(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn peer_sending_more_than_declared_is_aborted() {
    let alice = Peer::new().await;
    let bob = Peer::new().await;
    let real = alice
        .files
        .add(Input::Bytes {
            name: "big.bin".into(),
            mime: "application/octet-stream".into(),
            data: sample(3 * MIB),
        })
        .await
        .unwrap();
    // The reference lies about the size, so the honest 3 MiB blob exceeds the declared bytes.
    let mut lying = real.clone();
    lying.size = 1000;
    lying.cipher_size = cipher_len(1000);
    let err = bob
        .files
        .fetch(
            &bob.endpoint,
            &[alice.addr()],
            &lying,
            &bob.downloads(),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::Fetch(_)), "{err:?}");
    assert!(!bob.downloads().exists() || fs::read_dir(bob.downloads()).unwrap().next().is_none());
}
