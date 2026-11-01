//! Times a large listing from Add to the first and last byte at a downloader, and what the downloader reads meanwhile.
//! Run by hand: `BIG_MB=1536 [BIG_LINKED=1] target/fw/dissent-test.exe --ignored --nocapture big_timing::`.
//! Everything runs on loopback, so its transfer rate is this machine's disk and CPU, not any real network.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::common::*;
use dissent_core::tracker::catalog::{AddItem, AddSpecView, CategoryInput};
use dissent_core::tracker::SeedLimit;
use dissent_core::{CommunityId, EngineEvent};

fn category() -> CategoryInput {
    CategoryInput {
        name: "Big".into(),
        icon: "package".into(),
        types: vec![],
        default_tags: vec![],
        rules: Default::default(),
        add: AddSpecView { min_rank: "member".into(), ..AddSpecView::default() },
        visible: None,
    }
}

/// Writes `mb` MiB of random bytes, without keeping them in memory.
fn random_file(path: &std::path::Path, mb: usize) {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut block = vec![0u8; 1024 * 1024];
    for _ in 0..mb {
        rand_block(&mut block);
        file.write_all(&block).unwrap();
    }
    file.flush().unwrap();
}

fn lap(what: &str, from: Instant) -> Duration {
    let took = from.elapsed();
    println!("| {what} | {:.2} s |", took.as_secs_f64());
    took
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "by hand: writes a large file; see the top of this file"]
async fn big_listing_stage_timings() {
    let mb: usize = std::env::var("BIG_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let linked = std::env::var("BIG_LINKED").is_ok();
    println!("## {mb} MiB, {}", if linked { "linked folder" } else { "copied to the store" });
    let alice = peer("big-alice").await;
    let bob = peer("big-bob").await;
    let server = alice.engine.create_community("Big".into()).await.expect("create");
    let id: CommunityId = server.id.parse().unwrap();
    let cat = alice.engine.create_category(id, category()).await.expect("category");
    let invite = alice.engine.create_invite(id).await.unwrap();
    join_ready(&alice, &bob, &invite).await;

    let dir = scratch("big-src");
    let path = dir.join("big.bin");
    random_file(&path, mb);
    if linked {
        alice.engine.set_category_folder(id, cat, Some(dir.clone())).unwrap();
    }

    // A tick that overruns means the runtime stalled; a slow read of `me` means the world's lock is held.
    let worst_tick = Arc::new(AtomicU64::new(0));
    let worst_lock = Arc::new(AtomicU64::new(0));
    let probe = {
        let (worst_tick, worst_lock, engine) = (worst_tick.clone(), worst_lock.clone(), alice.engine.clone());
        tokio::spawn(async move {
            loop {
                let at = Instant::now();
                tokio::time::sleep(Duration::from_millis(20)).await;
                worst_tick.fetch_max(at.elapsed().as_millis() as u64, Ordering::Relaxed);
                let at = Instant::now();
                let _ = engine.me();
                worst_lock.fetch_max(at.elapsed().as_micros() as u64, Ordering::Relaxed);
            }
        })
    };

    println!("| stage | time |\n| --- | --- |");
    let add_start = Instant::now();
    let at = Instant::now();
    alice.engine.prepare_listing(id, cat, vec![path.clone()]).await.unwrap();
    lap("prepare_listing (dialog fingerprint)", at);
    let item = AddItem { path: path.to_string_lossy().into_owned(), tags: vec![], description: String::new() };
    let at = Instant::now();
    let adding = {
        let engine = alice.engine.clone();
        tokio::spawn(async move { engine.list_files(id, cat, vec![item], SeedLimit::Forever).await })
    };
    let (mut seen_at, mut listed) = (None, None);
    let mut adding = Some(adding);
    while seen_at.is_none() || adding.is_some() {
        if seen_at.is_none() && bob.engine.files_view(id).is_ok_and(|v| !v.listings.is_empty()) {
            seen_at = Some(at.elapsed());
        }
        if adding.as_ref().is_some_and(|t| t.is_finished()) {
            listed = Some(adding.take().unwrap().await.unwrap().unwrap());
            lap("list_files (fingerprint again, encrypt + hash + import, publish)", at);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let seen_at = seen_at.unwrap();
    println!("| listing visible to the peer, after list_files began | {:.2} s |", seen_at.as_secs_f64());
    let listing = listed.unwrap().remove(0);
    lap("Add to listed, total", add_start);
    println!("| seeder: worst 20 ms tick while adding | {} ms |", worst_tick.swap(0, Ordering::Relaxed));
    println!("| seeder: worst read of the world while adding | {} us |", worst_lock.swap(0, Ordering::Relaxed));

    // Counts the events the desktop's forwarder would receive.
    let (progress_events, lagged) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let counter = {
        let (progress_events, lagged) = (progress_events.clone(), lagged.clone());
        let mut rx = bob.engine.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(EngineEvent::AttachmentProgress { .. }) => {
                        progress_events.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        lagged.fetch_add(n, Ordering::Relaxed);
                    }
                    Err(_) => return,
                }
            }
        })
    };

    let click = Instant::now();
    bob.engine.download_listing(id, listing.parse().unwrap()).await.expect("download starts");
    println!("| download_listing returned | {:.3} s |", click.elapsed().as_secs_f64());
    let mut first_byte = None;
    let mut all_bytes = None;
    let (mut last_progress, mut last_change, mut worst_gap) = (0.0f64, Instant::now(), Duration::ZERO);
    let mut next_report = 0.0;
    println!("\n| t (s) | progress | source bytes | average |\n| --- | --- | --- | --- |");
    let done = loop {
        let row = bob.engine.transfers().into_iter().find(|t| t.message_id.contains(&listing)).expect("transfer row");
        if row.progress > last_progress {
            worst_gap = worst_gap.max(last_change.elapsed());
            last_change = Instant::now();
            last_progress = row.progress;
            if first_byte.is_none() {
                first_byte = Some(click.elapsed());
            }
        }
        let bytes: u64 = row.sources.iter().map(|s| s.bytes).sum();
        if row.progress >= next_report {
            let t = click.elapsed().as_secs_f64();
            println!("| {t:.2} | {:.1}% | {bytes} | {:.0} MB/s |", row.progress * 100.0, bytes as f64 / t.max(0.001) / 1e6);
            next_report = row.progress + 0.1;
        }
        if all_bytes.is_none() && row.progress >= 1.0 {
            all_bytes = Some(click.elapsed());
        }
        match row.state.as_str() {
            "done" => break row,
            "failed" => panic!("failed: {:?}", row.error),
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let finished = click.elapsed();
    println!("\n| stage | time |\n| --- | --- |");
    println!("| click to first progress | {:.3} s |", first_byte.unwrap_or_default().as_secs_f64());
    println!("| click to 100% | {:.2} s |", all_bytes.unwrap_or_default().as_secs_f64());
    println!("| click to done (decrypt + verify after 100%) | {:.2} s |", finished.as_secs_f64());
    println!("| longest pause in progress, sampled | {:.2} s |", worst_gap.as_secs_f64());
    println!("| progress events | {} (lagged {}) |", progress_events.load(Ordering::Relaxed), lagged.load(Ordering::Relaxed));
    println!("| seeder: worst 20 ms tick while serving | {} ms |", worst_tick.load(Ordering::Relaxed));
    println!("| seeder: worst read of the world while serving | {} us |", worst_lock.load(Ordering::Relaxed));
    assert!(done.local_path.is_some());
    probe.abort();
    counter.abort();
    let _ = std::fs::remove_dir_all(&dir);
    finish(vec![alice, bob]).await;
}

/// Incompressible bytes, fast: a BLAKE3 stream keyed by a counter. Nothing here relies on their quality.
fn rand_block(block: &mut [u8]) {
    static N: AtomicU64 = AtomicU64::new(0);
    let seed = blake3::hash(&N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    blake3::Hasher::new_keyed(seed.as_bytes()).finalize_xof().fill(block);
}

/// A chat attachment goes through the same blob gate as a listing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "by hand: writes a large file; see the top of this file"]
async fn big_attachment_timing() {
    let mb: usize = std::env::var("BIG_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let alice = peer("big-att-alice").await;
    let bob = peer("big-att-bob").await;
    let server = alice.engine.create_community("Big".into()).await.expect("create");
    let id: CommunityId = server.id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.unwrap();
    join_ready(&alice, &bob, &invite).await;
    let channel = channel_id(&alice.engine, id, "general").expect("general");
    let dir = scratch("big-att-src");
    let path = dir.join("big.bin");
    random_file(&path, mb);
    let at = Instant::now();
    let sent = alice.engine.send_files(id, channel, vec![path.clone()], String::new(), false).await.expect("send");
    println!("| attachment {mb} MiB: send_files | {:.2} s |", at.elapsed().as_secs_f64());
    let attachment = sent.attachments[0].id.clone();
    let until = Instant::now() + Duration::from_secs(60);
    let row = loop {
        match bob.engine.download_attachment(Some(id), sent.id.clone(), attachment.clone()).await {
            Ok(row) => break row,
            Err(e) => assert!(Instant::now() < until, "{e}"),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let click = Instant::now();
    let mut all_bytes = None;
    loop {
        let now = bob.engine.transfers().into_iter().find(|t| t.id == row.id).expect("row");
        if all_bytes.is_none() && now.progress >= 1.0 {
            all_bytes = Some(click.elapsed().as_secs_f64());
        }
        match now.state.as_str() {
            "done" => break,
            "failed" => panic!("{:?}", now.error),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    println!("| attachment {mb} MiB: click to 100% | {:.2} s |", all_bytes.unwrap_or_default());
    println!("| attachment {mb} MiB: click to done | {:.2} s |", click.elapsed().as_secs_f64());
    let _ = std::fs::remove_dir_all(&dir);
    finish(vec![alice, bob]).await;
}
