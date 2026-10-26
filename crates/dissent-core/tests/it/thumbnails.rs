//! Thumbnails of images in direct messages: the sender makes one, the recipient fetches it on
//! arrival without asking, and the full file still waits for a click.

use crate::common::*;

/// A 800 by 400 picture, so its thumbnail is 320 by 160.
fn a_picture(path: &std::path::Path) {
    let img = image::RgbImage::from_fn(800, 400, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    img.save(path).expect("write the picture");
}

/// Waits until bob holds the thumbnail of his copy of `message_id`, and returns its path.
async fn thumbnail_of(bob: &Peer, peer: dissent_identity::RootId, message_id: &str) -> std::path::PathBuf {
    eventually("bob has the thumbnail", || {
        bob.engine
            .dm_messages(peer, 10)
            .iter()
            .find(|m| m.id == message_id)
            .and_then(|m| m.attachments[0].thumb.clone())
            .filter(|t| t.state == "downloaded")
            .map(|_| ())
    })
    .await;
    let view = bob
        .engine
        .dm_messages(peer, 10)
        .into_iter()
        .find(|m| m.id == message_id)
        .expect("bob's copy of the message")
        .attachments
        .remove(0);
    std::path::PathBuf::from(view.thumb.expect("the thumbnail is listed").path.expect("a path once downloaded"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_thumbnail_arrives_and_decodes_without_a_click() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("thumbs");
    let path = dir.join("holiday.png");
    a_picture(&path);

    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path], "look".into())
        .await
        .expect("alice sends the picture");
    let message_id = sent.id.clone();
    let attachment_id = sent.attachments[0].id.clone();
    assert_eq!(sent.attachments[0].thumb.as_ref().map(|t| (t.width, t.height)), Some((320, 160)));

    let thumb_path = thumbnail_of(&bob, alice_root, &message_id).await;
    let decoded = image::open(&thumb_path).expect("the thumbnail decodes");
    assert_eq!((decoded.width(), decoded.height()), (320, 160));

    // The full picture was not asked for, so it still waits.
    let view = bob
        .engine
        .dm_messages(alice_root, 10)
        .into_iter()
        .find(|m| m.id == message_id)
        .expect("bob's copy")
        .attachments
        .remove(0);
    assert_eq!(view.state, "available");
    assert_eq!(view.id, attachment_id);

    let stats = bob.engine.store_stats().await.expect("stats");
    assert!(stats.thumbnail_bytes > 0, "the thumbnail counts toward the store");

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_thumbnail_is_removed_at_restart_and_a_live_one_stays() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    let dir = scratch("thumb-sweep");
    let path = dir.join("holiday.png");
    a_picture(&path);
    let sent = alice
        .engine
        .send_dm_files(bob_root, vec![path], "look".into())
        .await
        .expect("alice sends the picture");
    let live = thumbnail_of(&bob, alice_root, &sent.id).await;

    // A file no message refers to, as a deleted message's thumbnail would be.
    let stale = live.with_file_name("0000stale.jpg");
    std::fs::write(&stale, b"left behind").expect("write a stale thumbnail");

    let (identity, data_dir) = stop(bob).await;
    let bob = start(identity, data_dir).await;
    assert!(!stale.exists(), "the start-up sweep removes what no message shows");
    assert!(live.is_file(), "the thumbnail of a live message stays");

    finish(vec![alice, bob]).await;
}
