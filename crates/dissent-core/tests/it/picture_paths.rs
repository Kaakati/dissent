//! Every file path a view hands the window must load there. Pictures the engine keeps are sealed at rest, so the
//! desktop app serves them through its `dimg` scheme, which opens only what `Engine::open_picture` opens. A plain file
//! goes through the asset protocol instead: a download, or a file the user picked. This walks every view the window
//! reads after a scene with every kind of picture, and checks each path it names is one of the two.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use dissent_core::test_support::map_preview_host;
use dissent_core::Engine;
use serde::Serialize;

use crate::common::*;

/// Stand-in bytes for a cropped WebP. The engine stores what it is given.
fn webp(tag: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, len));
    bytes
}

/// Every string in `value` that names a file on disk.
fn files_in(value: &serde_json::Value, out: &mut BTreeSet<PathBuf>) {
    match value {
        serde_json::Value::String(s) if Path::new(s).is_absolute() && Path::new(s).is_file() => {
            out.insert(PathBuf::from(s));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| files_in(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| files_in(v, out)),
        _ => {}
    }
}

/// What the window does with the files its views name.
#[derive(Default, Debug)]
struct Served {
    /// Sealed pictures, through `dimg`.
    pictures: BTreeSet<PathBuf>,
    /// Plain files, through the asset protocol: downloads and the files the user picked.
    plain: BTreeSet<PathBuf>,
}

/// Sorts each file `views` name. A file in the profile is a picture `open_picture` opens, or a download in
/// `<profile>/downloads` (where a test engine saves them). Anything else in the profile would reach the window as
/// ciphertext or not at all, so it fails the test. A file outside the profile is the user's own.
fn served(engine: &Engine, profile: &Path, views: &[(&str, serde_json::Value)]) -> Served {
    let profile = profile.canonicalize().unwrap();
    let downloads = profile.join("downloads");
    let mut out = Served::default();
    for (what, view) in views {
        let mut files = BTreeSet::new();
        files_in(view, &mut files);
        for file in files {
            let real = file.canonicalize().unwrap();
            if engine.open_picture(&file).is_ok() {
                assert_ne!(std::fs::read(&file).unwrap(), engine.open_picture(&file).unwrap().0, "{what}: {} is sealed", file.display());
                out.pictures.insert(real);
            } else if real.starts_with(&downloads) || !real.starts_with(&profile) {
                out.plain.insert(real);
            } else {
                panic!("{what} names {}, a store file the window can't load", file.display());
            }
        }
    }
    out
}

fn json(view: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(view).expect("the view serialises")
}

/// Every view the window reads that can carry a picture, as `engine` sees them.
async fn views(engine: &Engine, community: dissent_identity::CommunityId, other: dissent_identity::RootId, invite: &str) -> Vec<(&'static str, serde_json::Value)> {
    let general = channel_id(engine, community, "general").expect("general channel");
    vec![
        ("me", json(&engine.me())),
        ("communities", json(&engine.communities())),
        ("community_view", json(&engine.community_view(community).expect("the server opens"))),
        ("channel_messages", json(&engine.channel_messages(community, general, 50))),
        ("user_profile", json(&engine.user_profile(other))),
        ("conversations", json(&engine.conversations())),
        ("transfers", json(&engine.transfers())),
        ("invite_preview", json(&engine.invite_preview(invite).await.expect("the invite previews"))),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn every_picture_a_view_names_is_one_the_window_can_load() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");

    // Alice's picture and banner, the server's icon and banner, a link preview with a picture, and a photo.
    alice.engine.set_avatar(webp(1, 200)).await.expect("avatar");
    alice.engine.set_profile_banner(webp(2, 2000)).await.expect("profile banner");
    alice.engine.set_community_icon(community, webp(3, 200)).await.expect("icon");
    alice.engine.set_community_banner(community, webp(4, 2000)).await.expect("banner");
    let addr = crate::previews::a_site(Arc::new(AtomicUsize::new(0))).await;
    map_preview_host("preview.test", addr);
    map_preview_host("picture.test", addr);
    alice.engine.set_link_previews(true).expect("previews on");
    let sent = alice
        .engine
        .send_message(community, general, format!("see http://preview.test:{}/page", addr.port()))
        .await
        .expect("alice posts the link");
    assert!(sent.embeds[0].image.is_some(), "the preview has its picture");
    let photo = scratch("picture-paths").join("photo.png");
    image::RgbImage::from_pixel(800, 400, image::Rgb([200, 40, 90])).save(&photo).unwrap();
    let posted = alice
        .engine
        .send_files(community, general, vec![photo.clone()], "a photo".into(), false)
        .await
        .expect("alice posts the photo");
    let invite = alice.engine.create_invite(community).await.expect("invite");

    // Bob has every picture once he has fetched them: alice's two, the server's two, the thumbnail and the preview's.
    let bob_root = bob.identity.root_id();
    let alice_root = alice.identity.root_id();
    eventually("bob has every picture", || {
        let view = bob.engine.community_view(community).ok()?;
        let messages = bob.engine.channel_messages(community, general, 50);
        let photo = messages.iter().find(|m| m.id == posted.id)?;
        let thumb = photo.attachments[0].thumb.as_ref().filter(|t| t.state == "downloaded")?;
        let preview = messages.iter().find_map(|m| m.embeds.first()?.image.clone())?;
        let profile = bob.engine.user_profile(alice_root);
        (view.icon.is_some() && view.banner.is_some() && thumb.path.is_some() && profile.banner.is_some() && !preview.is_empty())
            .then_some(())
    })
    .await;
    // And the photo itself, downloaded as a plain file.
    let attachment = posted.attachments[0].id.clone();
    bob.engine.download_attachment(Some(community), posted.id.clone(), attachment.clone()).await.expect("download");
    eventually("bob has the photo", || {
        let messages = bob.engine.channel_messages(community, general, 50);
        let a = &messages.iter().find(|m| m.id == posted.id)?.attachments[0];
        (a.state == "downloaded").then_some(())
    })
    .await;

    let bobs = served(&bob.engine, &bob.dir, &views(&bob.engine, community, alice_root, &invite).await);
    assert!(bobs.pictures.len() >= 6, "bob's views name the six pictures: {bobs:?}");
    assert_eq!(bobs.plain.len(), 1, "and one plain file, the download: {bobs:?}");
    let download = bobs.plain.iter().next().unwrap();
    assert_eq!(std::fs::read(download).unwrap(), std::fs::read(&photo).unwrap(), "the download is the plain photo");

    // Alice's side: her own pictures, her thumbnail, and the photo she picked, which stays where it was.
    let alices = served(&alice.engine, &alice.dir, &views(&alice.engine, community, bob_root, &invite).await);
    assert!(alices.pictures.len() >= 6, "alice's views name her pictures too: {alices:?}");
    assert!(alices.plain.contains(&photo.canonicalize().unwrap()), "the picked photo is a plain file: {alices:?}");

    finish(vec![alice, bob]).await;
}
