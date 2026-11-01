//! The file tracker's catalog: categories, listing files with dedupe by fingerprint, and downloading a listing.
//! See `docs/design/14-file-tracker.md`. The networked tests are `#[ignore]`d and run through `scripts/fwtest.sh`.

use std::time::Duration;

use crate::common::*;
use dissent_core::tracker::catalog::{AddItem, AddSpecView, CategoryInput};
use dissent_core::tracker::SeedLimit;
use dissent_core::CommunityId;

fn releases() -> CategoryInput {
    CategoryInput {
        name: "Releases".into(),
        icon: "package".into(),
        types: vec!["Installer".into(), "Archive".into()],
        default_tags: vec!["stable".into()],
        rules: Default::default(),
        add: AddSpecView { min_rank: "member".into(), ..AddSpecView::default() },
        visible: None,
    }
}

fn item(path: &std::path::Path, tags: &[&str]) -> AddItem {
    AddItem { path: path.to_string_lossy().into_owned(), tags: tags.iter().map(|t| t.to_string()).collect(), description: "A build".into() }
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn sample(n: usize, salt: u8) -> Vec<u8> {
    (0..n).map(|i| ((i as u32).wrapping_mul(2654435761) >> 13) as u8 ^ salt).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_makes_a_category_lists_files_and_a_second_copy_dedupes() {
    heavy(2, async {
        let alice = peer("tracker-alone").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");

        let view = alice.engine.files_view(id).unwrap();
        assert_eq!(view.categories.len(), 1);
        assert!(view.categories[0].readable, "the owner made the first files key and can read its own category");
        assert_eq!(view.categories[0].name, "Releases");
        assert_eq!(view.categories[0].types, vec!["Installer", "Archive"]);

        let dir = scratch("tracker-files");
        let bytes = sample(150_000, 1);
        let msi = write(&dir, "dissent-0.4.1-windows-x64.msi", &bytes);
        let notes = write(&dir, "RELEASE-NOTES.txt", b"notes");
        let found = alice.engine.prepare_listing(id, cat, vec![msi.clone(), notes.clone()]).await.unwrap();
        assert_eq!(found[0].state, "ready");
        assert_eq!(found[0].fingerprint, data_encoding::HEXLOWER.encode(blake3::hash(&bytes).as_bytes()));
        assert_eq!(found[1].state, "type_not_allowed");
        assert!(alice.engine.list_files(id, cat, vec![item(&notes, &[])], SeedLimit::Forever).await.is_err());

        let listed = alice.engine.list_files(id, cat, vec![item(&msi, &["#Stable", "windows"])], SeedLimit::Forever).await.unwrap();
        let view = alice.engine.files_view(id).unwrap();
        assert_eq!(view.listings.len(), 1);
        let l = &view.listings[0];
        assert_eq!((l.id.as_str(), l.name.as_str(), l.ext.as_str(), l.kind.as_str()), (listed[0].as_str(), "dissent-0.4.1-windows-x64.msi", "MSI", "Installer"));
        assert_eq!(l.tags, vec!["stable", "windows"]);
        assert_eq!(l.size, bytes.len() as u64);
        assert_eq!(l.local, "seeding");
        assert_eq!(l.danger, Some(dissent_core::Danger::Installer), "an installer is flagged");
        assert!(l.mine && l.can_edit);
        assert_eq!(view.categories[0].files, 1);

        // The same bytes under another name are already listed; adding them seeds that listing.
        let copy = write(&dir, "renamed.msi", &bytes);
        let again = alice.engine.prepare_listing(id, cat, vec![copy.clone()]).await.unwrap();
        assert_eq!(again[0].state, "already_listed");
        assert_eq!(again[0].listing.as_deref(), Some(listed[0].as_str()));
        let seeded = alice.engine.list_files(id, cat, vec![item(&copy, &[])], SeedLimit::Forever).await.unwrap();
        assert_eq!(seeded, listed, "no second listing");
        assert_eq!(alice.engine.files_view(id).unwrap().listings.len(), 1);

        alice.engine.update_listing(id, listed[0].parse().unwrap(), vec!["signed".into()], "Signed build".into()).await.unwrap();
        let l = alice.engine.files_view(id).unwrap().listings.remove(0);
        assert_eq!((l.tags, l.description), (vec!["signed".to_string()], "Signed build".to_string()));

        // The keys and the book survive a restart.
        let (identity, data) = stop(alice).await;
        let again = start(identity, data).await;
        let view = again.engine.files_view(id).unwrap();
        assert_eq!(view.listings.len(), 1);
        assert_eq!(view.listings[0].local, "seeding");
        again.engine.remove_listing(id, listed[0].parse().unwrap()).await.unwrap();
        assert!(again.engine.files_view(id).unwrap().listings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_old_app_or_an_offline_member_does_not_stop_a_server_sharing_files() {
    heavy(3, async {
        let alice = peer("tracker-free-alice").await;
        let bob = peer("tracker-free-bob").await;
        let carol = peer("tracker-free-carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;

        // Bob's app is too old for the tracker and Carol is offline.
        dissent_core::test_support::set_features(&bob.engine, 0);
        let (carol_identity, carol_dir) = stop(carol).await;
        let cat = alice.engine.create_category(id, releases()).await.expect("the first category needs nobody's app");

        let dir = scratch("tracker-free-src");
        let bytes = sample(150_000, 9);
        let msi = write(&dir, "app.msi", &bytes);
        alice.engine.prepare_listing(id, cat, vec![msi.clone()]).await.unwrap();
        let listed = alice.engine.list_files(id, cat, vec![item(&msi, &[])], SeedLimit::Forever).await.expect("the first listing");

        // A member who comes back on a current app reads the catalog and downloads.
        let carol = start(carol_identity, carol_dir).await;
        until_event(&carol.engine, Duration::from_secs(60), "carol reads the listing", || {
            carol.engine.files_view(id).ok().filter(|v| v.listings.len() == 1).map(|_| ())
        })
        .await;
        carol.engine.download_listing(id, listed[0].parse().unwrap()).await.expect("download");
        until_event(&carol.engine, Duration::from_secs(120), "carol has the file", || {
            carol.engine.files_view(id).ok().filter(|v| v.listings[0].local == "seeding").map(|_| ())
        })
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

/// Waits for a check job to finish, and returns what it reported.
async fn checked(events: &mut tokio::sync::broadcast::Receiver<dissent_core::EngineEvent>, job: u64) -> Vec<dissent_core::tracker::catalog::CandidateView> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(30), events.recv()).await.expect("the check finishes") {
            Ok(dissent_core::EngineEvent::FileCheck { job: j, result: Some(view), .. }) if j == job => out.push(view),
            Ok(dissent_core::EngineEvent::FileCheckDone { job: j }) if j == job => return out,
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_background_check_reports_each_file_and_only_for_chosen_paths() {
    heavy(1, async {
        let alice = peer("tracker-check").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let dir = scratch("tracker-check-src");
        let bytes = sample(150_000, 3);
        let msi = write(&dir, "a.msi", &bytes);
        let other = write(&dir, "b.msi", &sample(150_000, 4));

        // A path nobody chose goes nowhere: it is not checked, and it does not become listable.
        let refused = alice.engine.start_file_check(id, vec![msi.clone()]);
        assert!(refused.is_err(), "{refused:?}");
        assert!(alice.engine.list_files(id, cat, vec![item(&msi, &[])], SeedLimit::Forever).await.is_err());

        let mut events = alice.engine.subscribe();
        alice.engine.allow_paths([msi.clone(), other.clone()]);
        let job = alice.engine.start_file_check(id, vec![msi.clone(), other.clone()]).expect("chosen paths");
        let mut views = checked(&mut events, job).await;
        views.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].state, "ready");
        assert_eq!(views[0].fingerprint, data_encoding::HEXLOWER.encode(blake3::hash(&bytes).as_bytes()));
        let listed = alice.engine.list_files(id, cat, vec![item(&msi, &[])], SeedLimit::Forever).await.expect("checked, so listable");

        // The same bytes under another name read as already listed.
        let copy = write(&dir, "copy.msi", &bytes);
        alice.engine.allow_paths([copy.clone()]);
        let job = alice.engine.start_file_check(id, vec![copy]).unwrap();
        let views = checked(&mut events, job).await;
        assert_eq!((views[0].state.as_str(), views[0].listing.as_deref()), ("already_listed", Some(listed[0].as_str())));

        // A cancelled check stops, reports nothing more, and makes nothing listable.
        let big = write(&dir, "big.msi", &sample(40_000_000, 5));
        alice.engine.allow_paths([big.clone()]);
        let job = alice.engine.start_file_check(id, vec![big.clone()]).unwrap();
        alice.engine.cancel_file_check(job);
        let views = checked(&mut events, job).await;
        assert!(views.is_empty(), "{views:?}");
        assert!(alice.engine.list_files(id, cat, vec![item(&big, &[])], SeedLimit::Forever).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drop_sends_each_file_to_the_category_its_type_fits() {
    heavy(1, async {
        let alice = peer("tracker-drop").await;
        let server = alice.engine.create_community("Drops".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let dir = scratch("tracker-drop-src");
        let png = write(&dir, "cat.png", b"png");
        let pdf = write(&dir, "book.pdf", b"pdf");
        let key = write(&dir, "server.pem", b"secret");
        let all = vec![png.clone(), pdf.clone(), key.clone()];

        // A page cannot name a path nobody chose.
        assert!(alice.engine.plan_file_drop(id, all.clone(), None).await.is_err());
        alice.engine.allow_paths(all.iter().cloned());

        // A server with no category gets one per type, and never one for a key file, which is not listed.
        let plan = alice.engine.plan_file_drop(id, all.clone(), None).await.expect("plan");
        assert_eq!(plan.created, vec!["Images", "Documents"]);
        let by_name = |plan: &dissent_core::tracker::drops::DropPlan, name: &str| plan.items.iter().find(|i| i.name == name).cloned().unwrap();
        assert_eq!(by_name(&plan, "cat.png").category_name.as_deref(), Some("Images"));
        assert_eq!(by_name(&plan, "book.pdf").category_name.as_deref(), Some("Documents"));
        let secret = by_name(&plan, "server.pem");
        assert_eq!((secret.skip.as_deref(), secret.category), (Some("credentials"), None));

        // With categories, nothing more is made. The narrowest wins, and a filter that accepts the file wins over it.
        let view = alice.engine.files_view(id).unwrap();
        let images: dissent_events::CategoryId = view.categories.iter().find(|c| c.name == "Images").unwrap().id.parse().unwrap();
        let mut any = releases();
        any.name = "Everything".into();
        any.types = vec![];
        let everything = alice.engine.create_category(id, any).await.unwrap();
        let plan = alice.engine.plan_file_drop(id, vec![png.clone()], None).await.unwrap();
        assert!(plan.created.is_empty());
        assert_eq!(plan.items[0].category.as_deref(), Some(images.to_string().as_str()));
        let plan = alice.engine.plan_file_drop(id, vec![png.clone()], Some(everything)).await.unwrap();
        assert_eq!(plan.items[0].category.as_deref(), Some(everything.to_string().as_str()));
        let zip = write(&dir, "a.zip", b"zip");
        alice.engine.allow_paths([zip.clone()]);
        let plan = alice.engine.plan_file_drop(id, vec![zip.clone()], Some(images)).await.unwrap();
        assert_eq!(plan.items[0].category.as_deref(), Some(everything.to_string().as_str()), "the filter refuses a zip");

        // A folder is reported, not listed.
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        alice.engine.allow_paths([sub.clone()]);
        let plan = alice.engine.plan_file_drop(id, vec![sub.clone()], None).await.unwrap();
        assert_eq!((plan.items.len(), plan.folders.len()), (0, 1));
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_drop_respects_who_may_add_and_who_may_make_categories() {
    heavy(2, async {
        let alice = peer("tracker-drop-owner").await;
        let bob = peer("tracker-drop-member").await;
        let id = rustaceans(&alice, &bob).await;
        let dir = scratch("tracker-drop-member-src");
        let png = write(&dir, "cat.png", b"png");
        bob.engine.allow_paths([png.clone()]);

        // A member may not make the first category: nothing is listed, and the plan says why.
        let plan = bob.engine.plan_file_drop(id, vec![png.clone()], None).await.unwrap();
        assert!(plan.no_categories && plan.created.is_empty());
        assert_eq!(plan.items[0].skip.as_deref(), Some("no_category"));

        // A category only admins may add to is not offered to a member.
        let mut staff = releases();
        staff.name = "Staff only".into();
        staff.types = vec![];
        staff.add.min_rank = "admin".into();
        alice.engine.create_category(id, staff).await.unwrap();
        until_event(&bob.engine, Duration::from_secs(60), "bob sees the category", || {
            bob.engine.files_view(id).ok().filter(|v| v.categories.iter().any(|c| c.readable)).map(|_| ())
        })
        .await;
        let plan = bob.engine.plan_file_drop(id, vec![png.clone()], None).await.unwrap();
        assert!(!plan.no_categories);
        assert_eq!(plan.items[0].skip.as_deref(), Some("no_category"), "{:?}", plan.items[0]);
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_folder_is_scanned_and_seeded_in_place_with_no_copy() {
    heavy(2, async {
        let alice = peer("tracker-folder").await;
        let bob = peer("tracker-folder-bob").await;
        let id = rustaceans(&alice, &bob).await;
        let mut images = releases();
        images.name = "Images".into();
        images.types = vec!["Image".into()];
        let cat = alice.engine.create_category(id, images).await.expect("category");
        let mut docs = releases();
        docs.name = "Docs".into();
        docs.types = vec!["Document".into()];
        alice.engine.create_category(id, docs).await.expect("category");

        let dir = scratch("tracker-folder-src").join("Holiday Photos");
        let bytes = sample(200_000, 7);
        let png = write(&dir, "a.png", &bytes);
        write(&dir, "b.png", &sample(1000, 8));
        write(&dir.join("sub"), "c.png", &sample(1000, 9));
        write(&dir, "notes.txt", b"notes");
        write(&dir, ".hidden", b"x");
        write(&dir, "desktop.ini", b"x");
        write(&dir, ".env", b"SECRET=1");
        write(&dir.join(".git"), "config", b"x");
        write(&dir.join("1/2/3/4/5"), "deep.png", b"x");

        // A folder nobody chose is refused.
        assert!(alice.engine.scan_folder(id, dir.clone(), None).await.is_err());
        alice.engine.allow_paths([dir.clone()]);
        let scan = alice.engine.scan_folder(id, dir.clone(), None).await.expect("scan");
        assert_eq!(scan.name, "Holiday Photos");
        assert_eq!(scan.tag, "holiday-photos");
        assert_eq!(scan.category.as_deref(), Some(cat.to_string().as_str()), "the most common type picks the category");
        assert_eq!(scan.hidden_dirs, vec![".git"]);
        assert_eq!(scan.too_deep, 1);
        let flag_of = |name: &str| scan.files.iter().find(|f| f.name == name).map(|f| f.flag.clone());
        assert_eq!(flag_of("a.png").as_deref(), Some(""));
        assert_eq!(flag_of(".hidden").as_deref(), Some("hidden"));
        assert_eq!(flag_of("desktop.ini").as_deref(), Some("system"));
        assert_eq!(flag_of(".env").as_deref(), Some("credentials"));
        assert_eq!(flag_of("deep.png"), None);

        // A check, then listing in place.
        let mut events = alice.engine.subscribe();
        let job = alice.engine.start_file_check(id, vec![png.clone()]).expect("the scan allowed its files");
        checked(&mut events, job).await;
        let listed = alice.engine.list_files_in_place(id, cat, vec![item(&png, &["holiday-photos"])], SeedLimit::Forever).await.expect("listed");

        // No copy went into the store: it has an outboard, and the seed names the file where it is.
        let mut outboards = 0;
        let mut stack = vec![alice.dir.clone()];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "obao") {
                    outboards += 1;
                }
            }
        }
        assert!(outboards >= 1, "the file is linked, not copied");
        let l = alice.engine.files_view(id).unwrap().listings.remove(0);
        assert_eq!((l.local.as_str(), l.local_path.as_deref()), ("seeding", Some(png.to_string_lossy().as_ref())));
        assert_eq!(l.tags, vec!["holiday-photos"]);

        // Another member gets it from there.
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the listing", || {
            bob.engine.files_view(id).ok().filter(|v| v.listings.len() == 1).map(|_| ())
        })
        .await;
        bob.engine.download_listing(id, listed[0].parse().unwrap()).await.expect("download");
        until_event(&bob.engine, Duration::from_secs(120), "bob has the file", || {
            bob.engine.files_view(id).ok().filter(|v| v.listings[0].local == "seeding").map(|_| ())
        })
        .await;
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_joiner_reads_the_catalog_and_downloads_a_listing() {
    heavy(2, async {
        let alice = peer("tracker-alice").await;
        let bob = peer("tracker-bob").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let dir = scratch("tracker-src");
        let bytes = sample(400_000, 2);
        let zip = write(&dir, "brand-kit.zip", &bytes);
        alice.engine.prepare_listing(id, cat, vec![zip.clone()]).await.unwrap();
        let listed = alice.engine.list_files(id, cat, vec![item(&zip, &["brand"])], SeedLimit::Forever).await.unwrap();

        // Bob joins after the listing, under the default history setting, and still reads it.
        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;
        let listing = until_event(&bob.engine, Duration::from_secs(60), "bob reads the listing", || {
            bob.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.id == listed[0]))
        })
        .await;
        assert_eq!(listing.name, "brand-kit.zip");
        assert_eq!(listing.lister_name, alice.engine.me().display_name);

        bob.engine.download_listing(id, listed[0].parse().unwrap()).await.expect("download starts");
        // Once in, the download is kept and seeded.
        let path = until_event(&bob.engine, Duration::from_secs(60), "the download finishes and seeds", || {
            bob.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.local == "seeding")).and_then(|l| l.local_path)
        })
        .await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Bob lists his copy of the same bytes: it dedupes into alice's listing, with the same blob.
        let copy = write(&dir, "bobs-copy.zip", &bytes);
        bob.engine.prepare_listing(id, cat, vec![copy.clone()]).await.unwrap();
        let seeded = bob.engine.list_files(id, cat, vec![item(&copy, &[])], SeedLimit::Forever).await.unwrap();
        assert_eq!(seeded, listed);
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_second_downloader_fetches_from_both_holders_and_receipts_credit_them() {
    heavy(3, async {
        let alice = peer("swarm-alice").await;
        let bob = peer("swarm-bob").await;
        let carol = peer("swarm-carol").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let dir = scratch("swarm-src");
        let bytes = sample(9 * 1024 * 1024 + 3, 4);
        let zip = write(&dir, "nightly.zip", &bytes);
        alice.engine.prepare_listing(id, cat, vec![zip.clone()]).await.unwrap();
        let listed = alice.engine.list_files(id, cat, vec![item(&zip, &[])], SeedLimit::Forever).await.unwrap();
        let listing: dissent_events::EventId = listed[0].parse().unwrap();
        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;
        join_ready(&alice, &carol, &invite).await;

        // Bob downloads from alice and seeds; alice gets credit from bob's receipt.
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the listing", || {
            bob.engine.files_view(id).ok().filter(|v| v.listings.iter().any(|l| l.id == listed[0])).map(|_| ())
        })
        .await;
        bob.engine.download_listing(id, listing).await.expect("bob downloads");
        until_event(&bob.engine, Duration::from_secs(60), "bob seeds", || {
            bob.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.local == "seeding"))
        })
        .await;
        let alice_root = alice.identity.root_id().to_string();
        until_event(&alice.engine, Duration::from_secs(60), "alice is credited", || {
            alice.engine.files_view(id).ok().filter(|v| v.standing.served_members == 1).map(|_| ())
        })
        .await;
        let _ = alice_root;

        // Carol sees two seeders once she has heard holdings, and downloads from both.
        until_event(&carol.engine, Duration::from_secs(60), "carol sees two seeders", || {
            carol.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.seeders == 2))
        })
        .await;
        carol.engine.download_listing(id, listing).await.expect("carol downloads");
        let path = until_event(&carol.engine, Duration::from_secs(60), "carol seeds", || {
            carol.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.local == "seeding")).and_then(|l| l.local_path)
        })
        .await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let swarm = carol.engine.listing_swarm(id, listing).await.unwrap();
        assert!(swarm.peers.len() >= 2, "{:?}", swarm.peers);
        assert!(swarm.pieces.iter().all(|n| *n >= 1), "every piece is held");
        // Bob is credited by carol, since carol's receipt names everyone who delivered.
        until_event(&bob.engine, Duration::from_secs(60), "bob is credited", || {
            bob.engine.files_view(id).ok().filter(|v| v.standing.served_members >= 1).map(|_| ())
        })
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_linked_folder_is_scanned_listed_in_place_and_downloaded_from_there() {
    heavy(2, async {
        let alice = peer("linked-alice").await;
        let bob = peer("linked-bob").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let folder = scratch("linked-folder");
        let bytes = sample(3 * 1024 * 1024 + 9, 6);
        write(&folder, "dissent-0.4.1-linux.AppImage", &bytes);
        write(&folder, "notes.txt", b"not an installer");
        alice.engine.set_category_folder(id, cat, Some(folder.clone())).unwrap();

        let found = alice.engine.scan_category_folder(id, cat).await.unwrap();
        let states: Vec<(&str, &str)> = found.iter().map(|f| (f.name.as_str(), f.state.as_str())).collect();
        assert!(states.contains(&("dissent-0.4.1-linux.AppImage", "ready")), "{states:?}");
        assert!(states.contains(&("notes.txt", "type_not_allowed")), "{states:?}");
        let ready: Vec<AddItem> = found.iter().filter(|f| f.state == "ready").map(|f| AddItem { path: f.path.clone(), tags: vec![], description: String::new() }).collect();
        let listed = alice.engine.list_files(id, cat, ready, SeedLimit::Forever).await.unwrap();
        assert!(alice.engine.scan_category_folder(id, cat).await.unwrap().iter().all(|f| f.state != "ready"), "a seeded file is not found again");
        let store = alice.engine.store_stats().await.unwrap();
        assert_eq!(store.files, 0, "the folder's file was not copied into the store");

        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the listing", || {
            bob.engine.files_view(id).ok().filter(|v| v.listings.iter().any(|l| l.id == listed[0])).map(|_| ())
        })
        .await;
        bob.engine.download_listing(id, listed[0].parse().unwrap()).await.unwrap();
        let path = until_event(&bob.engine, Duration::from_secs(60), "bob has it from the linked copy", || {
            bob.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.local == "seeding")).and_then(|l| l.local_path)
        })
        .await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&folder);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_restricted_category_is_read_and_downloaded_by_its_readers_only() {
    use dissent_core::tracker::catalog::ReaderSpecView;
    heavy(3, async {
        let alice = peer("restricted-alice").await;
        let bob = peer("restricted-bob").await;
        let carol = peer("restricted-carol").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;
        join_ready(&alice, &carol, &invite).await;
        // Open category first, so every member holds the files key; then one only carol may see.
        alice.engine.create_category(id, releases()).await.expect("open category");
        let mut testers = releases();
        testers.name = "Voice samples".into();
        testers.visible = Some(ReaderSpecView { min_rank: "none".into(), roles: vec![], members: vec![carol.identity.root_id().to_string()] });
        let cat = alice.engine.create_category(id, testers).await.expect("restricted category");
        let dir = scratch("restricted-src");
        let bytes = sample(700_000, 7);
        let zip = write(&dir, "samples.zip", &bytes);
        alice.engine.prepare_listing(id, cat, vec![zip.clone()]).await.unwrap();
        let listed = alice.engine.list_files(id, cat, vec![item(&zip, &[])], SeedLimit::Forever).await.unwrap();

        // Carol reads it and downloads it.
        let listing = until_event(&carol.engine, Duration::from_secs(60), "carol reads the restricted listing", || {
            carol.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.id == listed[0]))
        })
        .await;
        assert_eq!(listing.name, "samples.zip");
        carol.engine.download_listing(id, listed[0].parse().unwrap()).await.unwrap();
        let path = until_event(&carol.engine, Duration::from_secs(60), "carol has it", || {
            carol.engine.files_view(id).ok().and_then(|v| v.listings.into_iter().find(|l| l.id == listed[0] && l.local == "seeding")).and_then(|l| l.local_path)
        })
        .await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Bob sees neither the category nor the listing, even once his catalog is complete.
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the open category", || {
            bob.engine.files_view(id).ok().filter(|v| v.categories.len() == 1).map(|_| ())
        })
        .await;
        let view = bob.engine.files_view(id).unwrap();
        assert!(view.categories.iter().all(|c| c.id != cat.to_string()));
        assert!(view.listings.is_empty());
        assert!(bob.engine.download_listing(id, listed[0].parse().unwrap()).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_picked_or_scanned_files_can_be_listed() {
    bounded(async {
        let alice = peer("tracker-offered").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let dir = scratch("tracker-offered");
        let secret = write(&dir, "id_rsa.zip", b"not offered");
        let refused = alice.engine.list_files(id, cat, vec![item(&secret, &[])], SeedLimit::Forever).await;
        assert!(refused.is_err(), "a path the person never picked is refused");
        alice.engine.prepare_listing(id, cat, vec![secret.clone()]).await.unwrap();
        alice.engine.list_files(id, cat, vec![item(&secret, &[])], SeedLimit::Forever).await.expect("once offered, it lists");
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_lying_listing_is_not_saved_cannot_squat_its_fingerprint_and_planted_keys_are_refused() {
    heavy(2, async {
        let alice = peer("liar-alice").await;
        let bob = peer("liar-bob").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;

        // Bob claims the fingerprint of the real release for other bytes.
        let real = sample(500_000, 9);
        let fingerprint = *blake3::hash(&real).as_bytes();
        until_event(&bob.engine, Duration::from_secs(60), "bob reads the category", || {
            bob.engine.files_view(id).ok().filter(|v| v.categories.iter().any(|c| c.readable)).map(|_| ())
        })
        .await;
        let fake = dissent_core::test_support::list_with_false_fingerprint(&bob.engine, id, cat, "release.zip", b"malware".repeat(1000), fingerprint)
            .await
            .unwrap();

        // Alice downloads it: it fails, and nothing is left on disk.
        until_event(&alice.engine, Duration::from_secs(60), "alice reads the fake listing", || {
            alice.engine.files_view(id).ok().filter(|v| v.listings.iter().any(|l| l.id == fake)).map(|_| ())
        })
        .await;
        alice.engine.download_listing(id, fake.parse().unwrap()).await.unwrap();
        until_event(&alice.engine, Duration::from_secs(60), "the download fails its fingerprint", || {
            alice.engine.transfers().into_iter().find(|t| t.state == "failed").map(|_| ())
        })
        .await;
        assert!(alice.engine.files_view(id).unwrap().listings.iter().all(|l| l.local != "seeding"));

        // Alice lists the real file: the fake listing does not block it.
        let dir = scratch("liar-src");
        let path = write(&dir, "release.zip", &real);
        alice.engine.prepare_listing(id, cat, vec![path.clone()]).await.unwrap();
        let listed = alice.engine.list_files(id, cat, vec![item(&path, &[])], SeedLimit::Forever).await.expect("listed anew");
        assert_ne!(listed[0], fake);

        // A key bob plants under a later secret generation is refused, so the newest stays the real one.
        let before = dissent_core::test_support::newest_files_generation(&alice.engine, id);
        assert!(dissent_core::test_support::push_files_keys(&bob.engine, alice.identity.device_id(), id, vec![(u64::MAX, [7; 32])]).await);
        assert_eq!(dissent_core::test_support::newest_files_generation(&alice.engine, id), before);
        let _ = std::fs::remove_dir_all(&dir);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn seeded_listings_survive_the_restart_sweep_and_count_their_uploads() {
    heavy(3, async {
        let alice = peer("sweep-alice").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let src = scratch("sweep-src");
        let folder = scratch("sweep-folder");
        let stored_bytes = sample(600_000, 11);
        let linked_bytes = sample(700_000, 12);
        let stored = write(&src, "stored.zip", &stored_bytes);
        write(&folder, "linked.zip", &linked_bytes);
        alice.engine.set_category_folder(id, cat, Some(folder.clone())).unwrap();
        alice.engine.prepare_listing(id, cat, vec![stored.clone()]).await.unwrap();
        let a = alice.engine.list_files(id, cat, vec![item(&stored, &[])], SeedLimit::Forever).await.unwrap();
        let scanned = alice.engine.scan_category_folder(id, cat).await.unwrap();
        let b = alice
            .engine
            .list_files(id, cat, scanned.iter().map(|f| AddItem { path: f.path.clone(), tags: vec![], description: String::new() }).collect(), SeedLimit::Forever)
            .await
            .unwrap();

        // The restart runs the sweep that unpins what nothing refers to. The seeds must survive it.
        let (identity, data) = stop(alice).await;
        let alice = start(identity, data).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(alice.engine.store_stats().await.unwrap().files, 1, "the stored seed keeps its pin");

        let bob = peer("sweep-bob").await;
        let invite = alice.engine.create_invite(id).await.unwrap();
        join_ready(&alice, &bob, &invite).await;
        for (listing, bytes) in [(&a[0], &stored_bytes), (&b[0], &linked_bytes)] {
            until_event(&bob.engine, Duration::from_secs(60), "bob reads the listing", || {
                bob.engine.files_view(id).ok().filter(|v| v.listings.iter().any(|l| &l.id == listing)).map(|_| ())
            })
            .await;
            bob.engine.download_listing(id, listing.parse().unwrap()).await.unwrap();
            let path = until_event(&bob.engine, Duration::from_secs(60), "bob has it", || {
                bob.engine
                    .files_view(id)
                    .ok()
                    .and_then(|v| v.listings.into_iter().find(|l| &l.id == listing && l.local == "seeding"))
                    .and_then(|l| l.local_path)
            })
            .await;
            assert_eq!(&std::fs::read(&path).unwrap(), bytes);
        }
        // What alice served shows in her seeding counters, from the gate for the stored copy and from the linked
        // server for the other.
        let rows = alice.engine.seeding_view(id).unwrap().rows;
        assert!(rows.iter().all(|r| r.uploaded > 0), "{:?}", rows.iter().map(|r| (r.name.clone(), r.uploaded)).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&folder);
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// Every file the tracker keeps is sealed: the linked folder's path and the settings are not readable on disk, and
/// all of it is there after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn the_trackers_files_are_sealed_and_survive_a_restart() {
    bounded(async {
        let alice = peer("tracker-sealed").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let folder = scratch("tracker-sealed-folder-marker");
        alice.engine.set_category_folder(id, cat, Some(folder.clone())).unwrap();
        let mut settings = alice.engine.seeding_settings();
        settings.max_active = 7;
        alice.engine.set_seeding_settings(settings).unwrap();

        let dir = alice.dir.clone();
        let identity = alice.identity.clone();
        alice.engine.shutdown().await.expect("stops");

        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    walk(&entry.path(), out);
                } else {
                    out.push(entry.path());
                }
            }
        }
        let mut files = Vec::new();
        walk(&dir, &mut files);
        for name in ["tracker.bin", "files_keys.bin", "seeding.bin"] {
            let file = files.iter().find(|f| f.ends_with(name)).unwrap_or_else(|| panic!("{name} is written"));
            let raw = std::fs::read(file).unwrap();
            assert!(raw.starts_with(b"DSF\x01"), "{name} is sealed");
        }
        let marker = folder.file_name().unwrap().to_string_lossy().into_owned();
        for file in &files {
            let raw = std::fs::read(file).unwrap();
            assert!(!raw.windows(marker.len()).any(|w| w == marker.as_bytes()), "{} holds the folder path", file.display());
        }

        let alice = start(identity, dir).await;
        assert_eq!(alice.engine.seeding_settings().max_active, 7);
        let view = alice.engine.files_view(id).unwrap();
        assert!(view.categories[0].readable, "the files key came back");
        assert_eq!(view.categories[0].name, "Releases");
    })
    .await;
}

/// A tracker book that no longer reads is kept beside its place, and the seeds it held are not unpinned by the sweep.
#[tokio::test(flavor = "multi_thread")]
async fn a_book_that_does_not_read_is_kept_and_its_seeds_keep_their_pins() {
    bounded(async {
        let alice = peer("tracker-bad-book").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let cat = alice.engine.create_category(id, releases()).await.expect("category");
        let src = scratch("tracker-bad-book-src");
        let file = write(&src, "kept.zip", &sample(300_000, 21));
        alice.engine.prepare_listing(id, cat, vec![file.clone()]).await.unwrap();
        alice.engine.list_files(id, cat, vec![item(&file, &[])], SeedLimit::Forever).await.unwrap();

        let (identity, data) = stop(alice).await;
        fn find(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
            std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
                if e.file_type().ok()?.is_dir() { find(&e.path(), name) } else { (e.file_name() == name).then(|| e.path()) }
            })
        }
        let book = find(&data, "tracker.bin").expect("the book was written");
        let damaged = [&[0u8][..], &[0x5a; 200]].concat();
        std::fs::write(&book, &damaged).unwrap();

        let alice = start(identity, data).await;
        alice.engine.files_view(id).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(std::fs::read(book.with_file_name("tracker.bin.bad")).unwrap(), damaged, "the bad book is kept whole");
        assert_eq!(alice.engine.store_stats().await.unwrap().files, 1, "the seed keeps its pin");
        let _ = std::fs::remove_dir_all(&src);
        finish(vec![alice]).await;
    })
    .await;
}
