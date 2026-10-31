//! A seeder keeps the thumbnails of a server's pictures for members who join later. The sender hints the seeders when
//! it posts, so the seeder holds the thumbnail before the sender goes offline. A new member joins through the seeder
//! and gets the thumbnail from the seeder.

use crate::common::*;
use dissent_core::{CommunityId, Engine, HistorySetting, MessageItem};
use std::time::Duration;

/// An 800 by 400 picture, so its thumbnail is 320 by 160.
fn a_picture(path: &std::path::Path) {
    let img = image::RgbImage::from_fn(800, 400, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    img.save(path).expect("write the picture");
}

/// The message in `channel` whose text is `text`, once `engine` can read it.
fn read_message(engine: &Engine, id: CommunityId, channel: dissent_core::ChannelId, text: &str) -> Option<MessageItem> {
    engine
        .channel_messages(id, channel, 100)
        .into_iter()
        .find(|m| m.text == text && !m.locked)
}

/// Whether the message's first picture has its thumbnail on this device.
fn has_thumbnail(item: &MessageItem) -> bool {
    item.attachments
        .first()
        .and_then(|a| a.thumb.as_ref())
        .is_some_and(|t| t.state == "downloaded")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_new_member_gets_the_thumbnail_from_the_seeder_while_the_sender_is_offline() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Thumbs".into())
            .await
            .expect("create community")
            .id
            .parse()
            .unwrap();
        // Joiners read past messages under `All`. Bob is in the community when the picture is posted, so he holds
        // the keys that the joiner needs.
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("owner shares past messages");
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = peer("bob").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        let seeder = peer("seeder").await;
        seeder.engine.join_as_storage(&invite).await.expect("seeder joins as storage");
        seeder
            .engine
            .set_mirror_policy(Some(dissent_files::MirrorPolicy::default()))
            .expect("the seeder mirrors");
        eventually("alice sees three members", || {
            alice.engine.community_view(id).ok().filter(|v| v.members.len() == 3).map(|_| ())
        })
        .await;

        let channel = channel_id(&alice.engine, id, "general").expect("general channel");
        // The first send after members join starts the readiness check, and goes out in the older form until it
        // answers. A warm-up message takes that send, so the picture goes out in the new form.
        alice
            .engine
            .send_message(id, channel, "warm up".into())
            .await
            .expect("alice warms up");
        tokio::time::sleep(Duration::from_secs(3)).await;
        let dir = scratch("thumb-hints");
        // A picture its sender keeps off the seeders: the seeder is told nothing about it, and Bob's fetch of its
        // thumbnail tells the seeder nothing either.
        let kept_off = dir.join("private.png");
        a_picture(&kept_off);
        alice
            .engine
            .send_files(id, channel, vec![kept_off], "kept off".into(), false)
            .await
            .expect("alice sends a picture kept off the seeders");
        eventually("bob has the thumbnail of the picture kept off the seeders", || {
            read_message(&bob.engine, id, channel, "kept off").filter(has_thumbnail).map(|_| ())
        })
        .await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            seeder.engine.store_stats().await.expect("the seeder's stats").files,
            0,
            "the seeder keeps nothing of a file its sender kept off the seeders"
        );

        let path = dir.join("holiday.png");
        a_picture(&path);
        alice
            .engine
            .send_files(id, channel, vec![path], "look".into(), true)
            .await
            .expect("alice sends the picture");

        // The seeder keeps the thumbnail blob, which the sender's hint named, and the picture, which it may seed,
        // before alice leaves.
        let started = std::time::Instant::now();
        while seeder.engine.store_stats().await.expect("the seeder's stats").files < 2 {
            assert!(started.elapsed() < Duration::from_secs(60), "the seeder never kept the thumbnail");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Alice goes offline. Carol joins through the seeder, reads the message from Bob's keys, and gets the
        // thumbnail from the seeder.
        stop(alice).await;
        let seeder_invite = seeder.engine.create_invite(id).await.expect("seeder invite");
        let carol = peer("carol").await;
        carol
            .engine
            .join_community(&seeder_invite)
            .await
            .expect("carol joins through the seeder");
        eventually("carol reads the message", || read_message(&carol.engine, id, channel, "look").map(|_| ())).await;
        eventually("carol has the thumbnail", || {
            read_message(&carol.engine, id, channel, "look").filter(has_thumbnail).map(|_| ())
        })
        .await;
    })
    .await;
}
