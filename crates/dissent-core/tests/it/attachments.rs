//! Files in direct messages: sent sealed, fetched from the sender's device, decrypted on the other side.


use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
async fn dm_file_is_sealed_and_downloads_for_the_recipient() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("attach");
    let path = dir.join("notes.txt");
    std::fs::write(&path, b"hello attachment").expect("write source file");

    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path.clone()], "see attached".into())
        .await
        .expect("alice sends a file");
    assert_eq!(sent.text, "see attached");
    assert_eq!(sent.attachments.len(), 1);
    assert_eq!(sent.attachments[0].name, "notes.txt");
    let message_id = sent.id.clone();
    let attachment_id = sent.attachments[0].id.clone();

    eventually("bob sees the file", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == message_id && m.attachments.len() == 1)
            .then_some(())
    })
    .await;

    bob.engine
        .download_attachment(None, message_id.clone(), attachment_id.clone())
        .await
        .expect("bob starts the download");
    eventually("bob has the file", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.attachments.iter().any(|a| a.state == "downloaded"))
            .then_some(())
    })
    .await;

    let local = bob
        .engine
        .dm_messages(alice_root, 10)
        .iter()
        .flat_map(|m| m.attachments.clone())
        .find_map(|a| a.local_path)
        .expect("a local path");
    assert_eq!(std::fs::read(&local).expect("read download"), b"hello attachment");
    assert!(
        bob.engine.transfers().iter().any(|t| t.state == "done" && t.direction == "download"),
        "the transfer is listed as done"
    );
    let sources = bob
        .engine
        .transfers()
        .into_iter()
        .find(|t| t.direction == "download")
        .map(|t| t.sources)
        .unwrap_or_default();
    assert!(!sources.is_empty(), "the providers are listed");
    assert!(sources.iter().all(|s| s.state == "done"));

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_file_reaches_members_and_mirror_policy_is_opt_in() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");

    let dir = scratch("channel-files");
    let path = dir.join("plan.txt");
    std::fs::write(&path, b"the plan").expect("write source file");
    let sent = alice
        .engine
        .send_files(community, general, vec![path], "the plan is attached".into(), true)
        .await
        .expect("alice posts a file");
    assert_eq!(sent.text, "the plan is attached");
    assert_eq!(sent.attachments.len(), 1);
    assert!(!has_message(&bob.engine, community, general, "\u{1e}dissent-attachments/v1\u{1e}"));

    eventually("bob reads the channel message", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .any(|m| m.text == "the plan is attached" && m.attachments.len() == 1)
            .then_some(())
    })
    .await;

    let att = sent.attachments[0].id.clone();
    bob.engine
        .download_attachment(Some(community), sent.id.clone(), att)
        .await
        .expect("bob downloads from the channel");
    eventually("bob has the channel file", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .any(|m| m.attachments.iter().any(|a| a.state == "downloaded"))
            .then_some(())
    })
    .await;

    finish(vec![alice, bob]).await;
}

fn transfer_state(p: &Peer, attachment: &str) -> Option<String> {
    p.engine
        .transfers()
        .into_iter()
        .find(|t| t.id == attachment)
        .map(|t| t.state)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_storage_seeder_mirrors_within_policy_and_refuses_the_rest() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let community = rustaceans(&alice, &bob).await;
    let invite = alice
        .engine
        .create_invite(community)
        .await
        .expect("invite");
    // Carol joins as storage only. She is never in the MLS group, so she cannot read the text.
    carol
        .engine
        .join_as_storage(&invite)
        .await
        .expect("carol joins as storage");
    let general = channel_id(&alice.engine, community, "general").expect("general channel");

    // Carol keeps files up to 1 KB, and nothing larger.
    carol
        .engine
        .set_mirror_policy(Some(dissent_files::MirrorPolicy {
            max_file_size: 1024,
            max_total: 1 << 30,
        }))
        .expect("policy set");

    let dir = scratch("mirror");
    let small_path = dir.join("small.txt");
    let big_path = dir.join("big.bin");
    std::fs::write(&small_path, vec![3u8; 100]).expect("small file");
    std::fs::write(&big_path, vec![5u8; 4096]).expect("big file");
    let small = alice
        .engine
        .send_files(community, general, vec![small_path], "small".into(), true)
        .await
        .expect("alice posts the small file");
    let big = alice
        .engine
        .send_files(community, general, vec![big_path], "big".into(), true)
        .await
        .expect("alice posts the big file");
    let small_id = small.attachments[0].id.clone();
    let big_id = big.attachments[0].id.clone();

    eventually("carol mirrors the small file", || {
        carol.engine.mirrored_attachments().contains(&small_id).then_some(())
    })
    .await;
    // The big file is refused by policy, so it never shows up as mirrored.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let mirrored = carol.engine.mirrored_attachments();
    assert_eq!(mirrored, vec![small_id], "only the file within policy is kept");
    assert!(!mirrored.contains(&big_id));

    finish(vec![alice, bob, carol]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_download_resumes_after_a_restart() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("pause");
    let path = dir.join("large.bin");
    let data: Vec<u8> = (0..32u32 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &data).expect("write source file");
    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path], "large file".into())
        .await
        .expect("alice sends");
    let message_id = sent.id.clone();
    let attachment_id = sent.attachments[0].id.clone();
    eventually("bob sees the file", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == message_id)
            .then_some(())
    })
    .await;

    bob.engine
        .download_attachment(None, message_id.clone(), attachment_id.clone())
        .await
        .expect("bob starts the download");
    assert_eq!(transfer_state(&bob, &attachment_id).as_deref(), Some("active"));
    bob.engine.pause_transfer(&attachment_id).expect("bob pauses");
    assert_eq!(transfer_state(&bob, &attachment_id).as_deref(), Some("paused"));

    // Bob restarts. The paused transfer comes back paused, and resumes from the bytes it has.
    let (identity, bob_dir) = stop(bob).await;
    let bob = start(identity, bob_dir).await;
    assert_eq!(transfer_state(&bob, &attachment_id).as_deref(), Some("paused"));
    bob.engine
        .resume_transfer(&attachment_id)
        .expect("bob resumes");
    crate::common::eventually_within(std::time::Duration::from_secs(90), "bob finishes the download", || {
        (transfer_state(&bob, &attachment_id).as_deref() == Some("done")).then_some(())
    })
    .await;

    let local = bob
        .engine
        .transfers()
        .into_iter()
        .find(|t| t.id == attachment_id)
        .and_then(|t| t.local_path)
        .expect("a finished download has a local path");
    assert_eq!(std::fs::read(&local).expect("read download"), data);

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn files_within_the_auto_accept_limit_download_on_arrival() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    assert_eq!(bob.engine.transfer_settings().auto_accept_bytes, None, "asks by default");
    bob.engine
        .set_transfer_settings(Some(1024))
        .expect("limit set");

    let dir = scratch("auto");
    let small = dir.join("small.txt");
    let large = dir.join("large.bin");
    std::fs::write(&small, vec![1u8; 100]).expect("small");
    std::fs::write(&large, vec![2u8; 4096]).expect("large");
    let small_sent = alice
        .engine
        .send_dm_files(bob_root, vec![small], "small".into())
        .await
        .expect("alice sends small");
    let large_sent = alice
        .engine
        .send_dm_files(bob_root, vec![large], "large".into())
        .await
        .expect("alice sends large");

    eventually("the small file arrives downloaded", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == small_sent.id && m.attachments.iter().any(|a| a.state == "downloaded"))
            .then_some(())
    })
    .await;
    // Nobody asked for the large file, and it is over the limit, so it waits.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let large_state = bob
        .engine
        .dm_messages(alice_root, 10)
        .iter()
        .find(|m| m.id == large_sent.id)
        .and_then(|m| m.attachments.first().map(|a| a.state.clone()));
    assert_eq!(large_state.as_deref(), Some("available"));

    let stats = alice.engine.store_stats().await.expect("stats");
    assert!(stats.files >= 2, "alice keeps both files pinned");
    assert!(stats.used_bytes > 0);
    assert_eq!(stats.budget_bytes, 0, "no mirror budget when not mirroring");
    assert!(stats.shared_bytes > 0, "the files alice sent are counted as shared");
    assert_eq!(stats.downloaded_bytes, 0, "alice downloaded nothing");

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_go_to_the_chosen_folder_and_bad_folders_are_refused() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("dl-dir");
    let missing = dir.join("does-not-exist");
    assert!(bob.engine.set_download_dir(Some(missing)).is_err(), "a missing folder is refused");
    let chosen = dir.join("chosen");
    std::fs::create_dir_all(&chosen).expect("chosen folder");
    let settings = bob
        .engine
        .set_download_dir(Some(chosen.clone()))
        .expect("a writable folder is accepted");
    assert_eq!(std::path::PathBuf::from(&settings.download_dir), chosen);

    let src = dir.join("hello.txt");
    std::fs::write(&src, b"to the chosen folder").expect("source");
    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![src], "file".into())
        .await
        .expect("alice sends");
    let (message, attachment) = (sent.id.clone(), sent.attachments[0].id.clone());
    eventually("bob sees the file", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == message)
            .then_some(())
    })
    .await;
    bob.engine
        .download_attachment(None, message, attachment)
        .await
        .expect("download starts");
    eventually("bob has the file", || {
        bob.engine
            .transfers()
            .iter()
            .any(|t| t.state == "done" && t.direction == "download")
            .then_some(())
    })
    .await;
    let local = bob
        .engine
        .transfers()
        .into_iter()
        .find_map(|t| t.local_path)
        .expect("a local path");
    assert!(std::path::Path::new(&local).starts_with(&chosen), "saved in the chosen folder");

    finish(vec![alice, bob]).await;
}

/// A 600 by 300 picture, large enough to get a thumbnail.
fn a_photo(path: &std::path::Path) {
    let img = image::RgbImage::from_fn(600, 300, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 64]));
    img.save(path).expect("write the photo");
}

/// Alice's own view of the one attachment of her message `id` in her conversation with `peer`.
fn own_view(p: &Peer, peer: dissent_identity::RootId, id: &str) -> dissent_core::AttachmentView {
    p.engine
        .dm_messages(peer, 10)
        .into_iter()
        .find(|m| m.id == id)
        .expect("the message")
        .attachments
        .remove(0)
}

// Regression: the sender of a photo saw "Download failed" and an empty preview, because its upload record read as a
// failed download. The sender has the file, so its message shows it at once and nothing is downloaded.
#[tokio::test(flavor = "multi_thread")]
async fn the_sender_shows_its_own_photo_at_once_and_the_recipient_downloads_it_on_request() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("own-photo");
    let path = dir.join("ridge.png");
    a_photo(&path);
    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path.clone()], String::new())
        .await
        .expect("alice sends the photo");
    let mine = &sent.attachments[0];
    assert_eq!(mine.state, "downloaded", "the sender's own photo is here");
    assert_eq!(mine.local_path.as_deref(), Some(path.to_string_lossy().as_ref()), "shown from where it was shared");
    assert!(!mine.needed && mine.error.is_none() && mine.transfer_id.is_none(), "{mine:?}");
    assert_eq!(mine.thumb.as_ref().map(|t| t.state.as_str()), Some("downloaded"), "the sender keeps its own preview");
    assert!(alice.engine.saved_files().contains(&path), "the app may show and open the shared file");

    // Asking again downloads nothing.
    alice
        .engine
        .download_attachment(None, sent.id.clone(), mine.id.clone())
        .await
        .expect("a no-op on the sender");
    assert!(alice.engine.transfers().iter().all(|t| t.direction == "upload"), "the sender never downloads its own file");

    // The recipient sees it as available until it asks for it.
    eventually("bob sees the photo", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == sent.id && m.attachments.len() == 1)
            .then_some(())
    })
    .await;
    let theirs = own_view(&bob, alice_root, &sent.id);
    assert_eq!(theirs.state, "available");
    assert!(theirs.needed && theirs.local_path.is_none());
    bob.engine
        .download_attachment(None, sent.id.clone(), theirs.id.clone())
        .await
        .expect("bob starts the download");
    let local = eventually("bob has the photo", || own_view(&bob, alice_root, &sent.id).local_path).await;
    assert_eq!(std::fs::read(&local).expect("read download"), std::fs::read(&path).expect("read source"));
    assert!(bob.engine.saved_files().iter().any(|p| p.to_string_lossy() == local));

    // Once the shared file is gone, the sender's copy is available again and decrypts from its own store.
    std::fs::remove_file(&path).expect("remove the source");
    let gone = own_view(&alice, bob_root, &sent.id);
    assert_eq!(gone.state, "available", "{gone:?}");
    alice
        .engine
        .download_attachment(None, sent.id.clone(), gone.id.clone())
        .await
        .expect("the sender saves its own copy");
    let saved = eventually("alice saves her copy", || own_view(&alice, bob_root, &sent.id).local_path).await;
    assert_eq!(std::fs::read(&saved).expect("read the saved copy"), std::fs::read(&local).expect("read bob's copy"));

    finish(vec![alice, bob]).await;
}

// A download with no one to fetch from fails with a reason the card shows beside Retry.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_that_fails_says_why() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("failed-photo");
    let path = dir.join("summit.png");
    a_photo(&path);
    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path], String::new())
        .await
        .expect("alice sends the photo");
    eventually("bob sees the photo", || {
        bob.engine
            .dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.id == sent.id && m.attachments.len() == 1)
            .then_some(())
    })
    .await;
    // Alice goes away before bob asks.
    finish(vec![alice]).await;
    bob.engine
        .download_attachment(None, sent.id.clone(), sent.attachments[0].id.clone())
        .await
        .expect("bob starts the download");
    let failed = eventually_within(std::time::Duration::from_secs(120), "the download fails", || {
        Some(own_view(&bob, alice_root, &sent.id)).filter(|a| a.state == "failed")
    })
    .await;
    let reason = failed.error.expect("a reason");
    assert!(reason.contains("Try again when the sender is online"), "{reason}");
    assert!(failed.local_path.is_none());

    finish(vec![bob]).await;
}

/// The hash of an attachment, from its id.
fn hash_of(attachment_id: &str) -> [u8; 32] {
    data_encoding::BASE32_NOPAD
        .decode(attachment_id.to_uppercase().as_bytes())
        .expect("base32 id")
        .try_into()
        .expect("32 bytes")
}

// Audit P-13: a deleted message's file was pinned, and served, for as long as the device ran. Now the sender and a
// seeder that mirrored it both unpin it, and a file another live message names stays pinned.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_message_unpins_its_file_and_a_live_one_keeps_its_own() {
    use dissent_core::test_support::pinned_blobs;
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(community).await.expect("invite");
    let carol = peer("carol").await;
    carol.engine.join_as_storage(&invite).await.expect("carol joins as storage");
    carol
        .engine
        .set_mirror_policy(Some(dissent_files::MirrorPolicy { max_file_size: 1 << 20, max_total: 1 << 30 }))
        .expect("carol mirrors");
    let general = channel_id(&alice.engine, community, "general").expect("general channel");

    let dir = scratch("unpin");
    std::fs::write(dir.join("keep.txt"), b"still here").expect("keep");
    std::fs::write(dir.join("gone.txt"), b"deleted soon").expect("gone");
    let keep = alice
        .engine
        .send_files(community, general, vec![dir.join("keep.txt")], "keep".into(), true)
        .await
        .expect("alice posts keep");
    let gone = alice
        .engine
        .send_files(community, general, vec![dir.join("gone.txt")], "gone".into(), true)
        .await
        .expect("alice posts gone");
    let (keep_hash, gone_hash) = (hash_of(&keep.attachments[0].id), hash_of(&gone.attachments[0].id));
    let gone_id = gone.attachments[0].id.clone();
    let keep_id = keep.attachments[0].id.clone();
    eventually("carol mirrors both files", || {
        let m = carol.engine.mirrored_attachments();
        (m.contains(&gone_id) && m.contains(&keep_id)).then_some(())
    })
    .await;
    let pinned = pinned_blobs(&alice.engine).await;
    assert!(pinned.contains(&keep_hash) && pinned.contains(&gone_hash), "the sender pins what it shares");

    let message: dissent_core::EventId = gone.id.parse().expect("event id");
    alice.engine.delete_message(community, general, message).await.expect("alice deletes gone");

    for (who, engine) in [("alice", &alice.engine), ("carol", &carol.engine)] {
        let mut unpinned = false;
        for _ in 0..100 {
            if !pinned_blobs(engine).await.contains(&gone_hash) {
                unpinned = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(unpinned, "{who} unpins the deleted message's file");
        assert!(pinned_blobs(engine).await.contains(&keep_hash), "{who} keeps the live message's file");
    }
    assert!(!carol.engine.mirrored_attachments().contains(&gone_id), "the seeder forgets the mirror");

    finish(vec![alice, bob, carol]).await;
}

// The same for a direct message: deleting it unpins its file on the sender.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_direct_message_unpins_its_file() {
    use dissent_core::test_support::pinned_blobs;
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let dir = scratch("unpin-dm");
    std::fs::write(dir.join("note.txt"), b"for bob only").expect("note");
    let sent = alice
        .engine
        .send_dm_files(bob.identity.root_id(), vec![dir.join("note.txt")], String::new())
        .await
        .expect("alice sends bob a file");
    let hash = hash_of(&sent.attachments[0].id);
    assert!(pinned_blobs(&alice.engine).await.contains(&hash));
    alice.engine.dm_delete(bob.identity.root_id(), &sent.id).await.expect("alice deletes it");
    let mut unpinned = false;
    for _ in 0..100 {
        if !pinned_blobs(&alice.engine).await.contains(&hash) {
            unpinned = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(unpinned, "the deleted direct message's file is unpinned");
    finish(vec![alice, bob]).await;
}

/// SOI, a JFIF header, an Exif block with a location and a camera in it, then the scan.
fn tagged_photo(path: &std::path::Path) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    j.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    let exif = b"Exif\0\0MM\0\x2A\0\0\0\x08\0\0GPS 37.3349 N 122.0090 W Make=PhoneCo";
    j.extend_from_slice(&[0xFF, 0xE1]);
    j.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    j.extend_from_slice(exif);
    j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9]);
    std::fs::write(path, &j).expect("write the photo");
    j
}

// A photo goes out without its location and camera, unless the sender keeps them. The file on disk never changes.
#[tokio::test(flavor = "multi_thread")]
async fn a_photo_goes_out_without_its_location_unless_the_sender_keeps_it() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();
    let dir = scratch("photo-details");

    let received = |sent: &str| {
        let bob = &bob;
        let sent = sent.to_string();
        async move {
            eventually("bob sees the photo", || {
                bob.engine.dm_messages(alice_root, 10).iter().any(|m| m.id == sent && m.attachments.len() == 1).then_some(())
            })
            .await;
            let theirs = own_view(bob, alice_root, &sent);
            bob.engine.download_attachment(None, sent.clone(), theirs.id.clone()).await.expect("bob starts the download");
            let local = eventually("bob has the photo", || own_view(bob, alice_root, &sent).local_path).await;
            std::fs::read(&local).expect("read the download")
        }
    };
    let has = |data: &[u8], needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);

    let path = dir.join("beach.jpg");
    let original = tagged_photo(&path);
    let sent = alice.engine.send_dm_files(bob_root, vec![path.clone()], String::new()).await.expect("send the photo");
    let got = received(&sent.id).await;
    assert!(!has(&got, b"37.3349") && !has(&got, b"PhoneCo"), "bob's copy has no location or camera");
    assert!(got.starts_with(&[0xFF, 0xD8]) && got.ends_with(&[0xFF, 0xD9]), "it is still the picture");
    assert_eq!(std::fs::read(&path).expect("read the original"), original, "alice's file is untouched");
    assert_eq!(own_view(&alice, bob_root, &sent.id).local_path.as_deref(), Some(path.to_string_lossy().as_ref()), "alice's message shows her original");

    // With the switch off, the photo goes as it is.
    alice.engine.set_keep_photo_metadata(true);
    let kept = dir.join("kept.jpg");
    tagged_photo(&kept);
    let sent = alice.engine.send_dm_files(bob_root, vec![kept.clone()], String::new()).await.expect("send the photo");
    assert_eq!(received(&sent.id).await, original, "the photo arrives exactly as it was");

    finish(vec![alice, bob]).await;
}
