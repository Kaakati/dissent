//! Times what one device does with many seeds. By hand: `MANY=5000 cargo test -p dissent-core --release --test it --
//! --ignored --nocapture many_seeds::`. One engine on loopback, so it needs no firewall path.

use std::time::Instant;

use crate::common::*;
use dissent_core::tracker::catalog::{AddItem, AddSpecView, CategoryInput};
use dissent_core::tracker::SeedLimit;
use dissent_core::CommunityId;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "by hand: lists thousands of files; see the top of this file"]
async fn many_seeds_stage_timings() {
    let n: usize = std::env::var("MANY").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let alice = peer("many-alice").await;
    let server = alice.engine.create_community("Many".into()).await.expect("create");
    let id: CommunityId = server.id.parse().unwrap();
    let input = CategoryInput {
        name: "All".into(),
        icon: "box".into(),
        types: vec![],
        default_tags: vec![],
        rules: Default::default(),
        add: AddSpecView { min_rank: "member".into(), ..AddSpecView::default() },
        visible: None,
    };
    let cat = alice.engine.create_category(id, input).await.expect("category");
    let dir = scratch("many-src");
    let paths: Vec<_> = (0..n)
        .map(|i| {
            let path = dir.join(format!("file-{i:05}.bin"));
            std::fs::write(&path, format!("seed number {i}").repeat(8)).unwrap();
            path
        })
        .collect();
    println!("| step | time |\n| --- | --- |");
    let started = Instant::now();
    let mut slowest = (0usize, 0f64);
    for (b, batch) in paths.chunks(100).enumerate() {
        let at = Instant::now();
        alice.engine.prepare_listing(id, cat, batch.to_vec()).await.unwrap();
        let items = batch.iter().map(|p| AddItem { path: p.to_string_lossy().into_owned(), tags: vec![], description: String::new() }).collect();
        alice.engine.list_files(id, cat, items, SeedLimit::Forever).await.unwrap();
        let took = at.elapsed().as_secs_f64();
        if took > slowest.1 {
            slowest = (b, took);
        }
        if b == 0 || (b + 1) % 10 == 0 {
            println!("| list batch {} of 100 files | {took:.2} s |", b + 1);
        }
    }
    println!("| list {n} files in all | {:.1} s |", started.elapsed().as_secs_f64());
    println!("| slowest batch | {} at {:.2} s |", slowest.0 + 1, slowest.1);
    let at = Instant::now();
    let view = alice.engine.seeding_view(id).unwrap();
    println!("| seeding_view | {:.3} s ({} rows, {} seeding) |", at.elapsed().as_secs_f64(), view.rows.len(), view.seeding);
    let at = Instant::now();
    let files = alice.engine.files_view(id).unwrap();
    println!("| files_view | {:.3} s ({} listings) |", at.elapsed().as_secs_f64(), files.listings.len());
    let at = Instant::now();
    let (blobs, bytes) = dissent_core::test_support::holdings_answer(&alice.engine, id);
    println!("| holdings answer | {:.3} s, {blobs} blobs, {bytes} bytes |", at.elapsed().as_secs_f64());
    let (identity, data) = stop(alice).await;
    let at = Instant::now();
    let again = start(identity, data).await;
    println!("| restart: engine start | {:.2} s |", at.elapsed().as_secs_f64());
    let at = Instant::now();
    let view = again.engine.seeding_view(id).unwrap();
    println!("| restart: first seeding_view | {:.3} s ({} rows) |", at.elapsed().as_secs_f64(), view.rows.len());
    let _ = std::fs::remove_dir_all(&dir);
    finish(vec![again]).await;
}
