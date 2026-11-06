//! File size limits: a server's limit reaches its members and their apps keep to it, and a file over a device's own
//! download limit is not fetched on arrival but can be downloaded anyway. See `dissent_core::limits`.

use std::time::Duration;

use crate::common::*;
use dissent_core::tracker::catalog::{AddItem, AddSpecView, CategoryInput};
use dissent_core::tracker::SeedLimit;
use dissent_core::{SizeLimits, OVER_DOWNLOAD_LIMIT};

const MIB: u64 = 1 << 20;

fn file(dir: &std::path::Path, name: &str, len: u64) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let bytes: Vec<u8> = (0..len).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    std::fs::write(&path, bytes).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_keeps_its_own_limits_and_refuses_ones_outside_the_range() {
    let alice = peer("limits-alice").await;
    assert_eq!(alice.engine.size_limits(), SizeLimits::default());
    let set = SizeLimits { max_upload: 500 << 30, max_download: 2 * MIB };
    alice.engine.set_size_limits(set).expect("limits within 1 MiB to 1 TiB");
    assert!(alice.engine.set_size_limits(SizeLimits { max_upload: MIB - 1, max_download: MIB }).is_err());
    assert!(alice.engine.set_size_limits(SizeLimits { max_upload: MIB, max_download: (1 << 40) + 1 }).is_err());
    let (identity, dir) = stop(alice).await;
    let again = start(identity, dir).await;
    assert_eq!(again.engine.size_limits(), set, "kept across a restart");
    finish(vec![again]).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_servers_limit_is_kept_by_members_and_a_file_over_my_limit_waits_for_a_click() {
    heavy(2, async {
        let alice = peer("limits-owner").await;
        let bob = peer("limits-member").await;
        let id = rustaceans(&alice, &bob).await;
        let general = channel_id(&alice.engine, id, "general").expect("general");
        let cat = alice
            .engine
            .create_category(
                id,
                CategoryInput {
                    name: "Any".into(),
                    icon: "box".into(),
                    types: vec![],
                    default_tags: vec![],
                    rules: Default::default(),
                    add: AddSpecView { min_rank: "member".into(), ..AddSpecView::default() },
                    visible: None,
                },
            )
            .await
            .expect("category");

        // Only someone who manages the server sets its limit, and it replays to every member.
        assert!(bob.engine.set_server_file_size_limit(id, 2 * MIB).await.is_err(), "a member may not");
        alice.engine.set_server_file_size_limit(id, 2 * MIB).await.expect("the owner may");
        until_event(&bob.engine, Duration::from_secs(60), "bob has the server's limit", || {
            bob.engine.community_view(id).ok().filter(|c| c.max_file_size == 2 * MIB).map(|_| ())
        })
        .await;
        assert!(!bob.engine.community_view(id).unwrap().can_set_max_file_size);

        // Bob's app keeps to it, for a message attachment and for a listing alike.
        let dir = scratch("limits-src");
        let big = file(&dir, "big.bin", 3 * MIB);
        let err = bob.engine.send_files(id, general, vec![big.clone()], String::new(), false).await.expect_err("over the server's limit");
        assert!(err.to_string().contains("this server's limit of 2 MB"), "{err}");
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the category", || {
            bob.engine.files_view(id).ok().filter(|v| v.categories.iter().any(|c| c.readable)).map(|_| ())
        })
        .await;
        let candidate = bob.engine.prepare_listing(id, cat, vec![big.clone()]).await.unwrap();
        assert_eq!(candidate[0].state, "too_large");
        let item = AddItem { path: big.to_string_lossy().into_owned(), tags: vec![], description: String::new() };
        assert!(bob.engine.list_files(id, cat, vec![item], SeedLimit::Forever).await.is_err(), "a listing over it is refused");
        let small = file(&dir, "small.bin", MIB);
        bob.engine.send_files(id, general, vec![small], String::new(), false).await.expect("within the limit");

        // A file over bob's own download limit is not fetched on arrival, though he takes everything automatically.
        bob.engine.set_transfer_settings(Some(u64::MAX)).unwrap();
        bob.engine.set_size_limits(SizeLimits { max_upload: 10 << 30, max_download: MIB }).unwrap();
        let sent = alice.engine.send_files(id, general, vec![file(&dir, "two.bin", MIB + MIB / 2)], "two".into(), false).await.expect("the owner sends");
        let attachment = sent.attachments[0].id.clone();
        let card = until_event(&bob.engine, Duration::from_secs(60), "bob has the message", || {
            bob.engine
                .channel_messages(id, general, 50)
                .into_iter()
                .find(|m| m.id == sent.id)
                .map(|m| m.attachments[0].clone())
        })
        .await;
        assert_eq!(card.over_limit.as_deref(), Some("Larger than your download limit (1 MB)"));
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(bob.engine.transfers().iter().all(|t| t.id != attachment), "not fetched on arrival");
        let err = bob.engine.download_attachment(Some(id), sent.id.clone(), attachment.clone()).await.expect_err("over my limit");
        assert!(err.to_string().contains(OVER_DOWNLOAD_LIMIT), "{err}");
        bob.engine.download_attachment_with(Some(id), sent.id.clone(), attachment.clone(), true).await.expect("anyway");
        until_event(&bob.engine, Duration::from_secs(60), "the download finishes", || {
            bob.engine.transfers().into_iter().find(|t| t.id == attachment && t.state == "done").map(|_| ())
        })
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob]).await;
    })
    .await;
}
