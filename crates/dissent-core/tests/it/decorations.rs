//! Avatar decorations. A built-in and an upload reach a member of a shared server and a contact with no shared server,
//! a removal travels too, and art that does not pass the check is never given a file. The networked tests are
//! `#[ignore]`d and run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_core::test_support::publish_unchecked_decoration;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, Rgba, RgbaImage};

/// A generated looping GIF of `frames` frames, `side` pixels square, with a clear middle.
fn art(frames: u32, side: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 30);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..frames {
            let img = RgbaImage::from_fn(side, side, |x, y| {
                let middle = x > side / 4 && x < 3 * side / 4 && y > side / 4 && y < 3 * side / 4;
                if middle { Rgba([0, 0, 0, 0]) } else { Rgba([(i * 40) as u8, x as u8, y as u8, 255]) }
            });
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test(flavor = "multi_thread")]
async fn my_decoration_is_on_my_profile_sealed_and_survives_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        let me = alice.identity.root_id();
        assert!(alice.engine.me().decoration.is_none());

        // What the check refuses, whatever the file is called.
        let mut oversize = art(1, 16);
        oversize.resize(2 * 1024 * 1024 + 1, 0);
        for (what, bytes) in [
            ("over 2 MB", oversize),
            ("an SVG", b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec()),
            ("a GIF in name only", b"GIF89a-not-really".to_vec()),
            ("over 512 pixels", art(1, 513)),
            ("over 300 frames", art(301, 16)),
        ] {
            assert!(alice.engine.set_avatar_decoration(bytes).await.is_err(), "{what} is refused");
        }
        assert!(alice.engine.set_avatar_decoration_builtin("Not An Id".into()).await.is_err());
        assert!(alice.engine.me().decoration.is_none(), "a refused decoration changes nothing");

        // A built-in names art the UI ships, and the name is sealed on disk.
        alice.engine.set_avatar_decoration_builtin("koi_pond".into()).await.expect("built-in");
        assert_eq!(alice.engine.me().decoration.and_then(|d| d.builtin).as_deref(), Some("koi_pond"));
        let book = std::fs::read(alice.dir.join("decorations.bin")).expect("the book is written");
        assert!(!contains(&book, b"koi_pond"), "the book is sealed");

        // An upload has a file and a still, both sealed, and both opened through the picture scheme.
        let bytes = art(4, 200);
        alice.engine.set_avatar_decoration(bytes.clone()).await.expect("upload");
        let view = alice.engine.user_profile(me).decoration.expect("a decoration");
        assert_eq!(view.builtin, None);
        let (image, poster) = (view.decoration_image.expect("art"), view.decoration_poster.expect("a still"));
        assert_eq!(std::path::Path::new(&image).parent(), Some(alice.dir.join("avatars").as_path()));
        assert_eq!(picture(&alice.engine, &image), bytes);
        assert!(picture(&alice.engine, &poster).starts_with(b"\x89PNG"));
        for path in [&image, &poster] {
            let on_disk = std::fs::read(path).unwrap();
            assert!(!on_disk.starts_with(b"GIF8") && !on_disk.starts_with(b"\x89PNG"), "{path} is sealed");
        }

        let (identity, dir) = stop(alice).await;
        let again = start(identity, dir).await;
        let view = again.engine.me().decoration.expect("kept across a restart");
        assert_eq!(view.decoration_image.as_deref(), Some(image.as_str()));
        again.engine.clear_avatar_decoration().await.expect("clear");
        assert!(again.engine.me().decoration.is_none());

        // Cleared, the upload is still offered on this device, and picking it again needs no file.
        let recent = again.engine.recent_decorations();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].decoration_image, image);
        assert!(again.engine.set_avatar_decoration_recent("nothing-of-mine".into()).await.is_err());
        again.engine.set_avatar_decoration_recent(recent[0].id.clone()).await.expect("picked again");
        assert_eq!(again.engine.me().decoration.and_then(|d| d.decoration_image).as_deref(), Some(image.as_str()));
        // Only the latest few are kept, newest first.
        for tag in 0..7 {
            again.engine.set_avatar_decoration(art(tag + 2, 32)).await.expect("upload");
        }
        let recent = again.engine.recent_decorations();
        assert_eq!(recent.len(), 6);
        assert_eq!(picture(&again.engine, &recent[0].decoration_image), art(8, 32));
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_server_member_and_a_contact_see_a_decoration_and_its_removal() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        // Bob shares a server with Alice. Carol is only her contact.
        rustaceans(&alice, &bob).await;
        make_contacts_ready(&alice, &carol).await;
        let root = alice.identity.root_id();
        let viewers = [("bob, in the server", &bob.engine), ("carol, a contact", &carol.engine)];

        alice.engine.set_avatar_decoration_builtin("koi_pond".into()).await.expect("built-in");
        for (who, engine) in viewers {
            eventually(&format!("{who} sees the built-in"), || {
                engine.user_profile(root).decoration.and_then(|d| d.builtin).filter(|id| id == "koi_pond")
            })
            .await;
        }

        // An id this app does not ship still arrives; the UI simply draws nothing for it.
        alice.engine.set_avatar_decoration_builtin("from-a-newer-app".into()).await.expect("unknown id");
        for (who, engine) in viewers {
            eventually(&format!("{who} keeps the unknown id"), || {
                engine.user_profile(root).decoration.and_then(|d| d.builtin).filter(|id| id == "from-a-newer-app")
            })
            .await;
        }

        let bytes = art(3, 288);
        alice.engine.set_avatar_decoration(bytes.clone()).await.expect("upload");
        for (who, engine) in viewers {
            let path = eventually(&format!("{who} fetches the upload"), || {
                engine.user_profile(root).decoration.and_then(|d| d.decoration_image)
            })
            .await;
            assert_eq!(picture(engine, &path), bytes, "{who}");
            let poster = engine.user_profile(root).decoration.and_then(|d| d.decoration_poster).expect("a still of its own");
            assert!(picture(engine, &poster).starts_with(b"\x89PNG"), "{who}");
        }

        alice.engine.clear_avatar_decoration().await.expect("clear");
        for (who, engine) in viewers {
            eventually(&format!("{who} sees it removed"), || engine.user_profile(root).decoration.is_none().then_some(())).await;
        }
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_decoration_set_before_two_people_are_contacts_arrives_both_ways() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        alice.engine.set_avatar_decoration_builtin("koi_pond".into()).await.expect("alice's");
        bob.engine.set_avatar_decoration_builtin("green_smoke".into()).await.expect("bob's");
        make_contacts_ready(&alice, &bob).await;
        let (a, b) = (alice.identity.root_id(), bob.identity.root_id());
        eventually("alice sees bob's", || alice.engine.user_profile(b).decoration.and_then(|d| d.builtin).filter(|id| id == "green_smoke")).await;
        eventually("bob sees alice's", || bob.engine.user_profile(a).decoration.and_then(|d| d.builtin).filter(|id| id == "koi_pond")).await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn art_that_fails_the_check_never_gets_a_file_on_the_receiving_side() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        rustaceans(&alice, &bob).await;
        let root = alice.identity.root_id();

        // A modified client skips the sender's check. The size is under the cap, so only decoding can catch these.
        publish_unchecked_decoration(&alice.engine, art(301, 16)).await.expect("too many frames");
        // Bob must get as far as refusing it, so a good one follows and is what he ends up showing.
        let good = art(2, 64);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(bob.engine.user_profile(root).decoration.is_none(), "refused art shows nothing");
        alice.engine.set_avatar_decoration(good.clone()).await.expect("good art");
        let path = eventually("bob fetches the good art", || bob.engine.user_profile(root).decoration.and_then(|d| d.decoration_image)).await;
        assert_eq!(picture(&bob.engine, &path), good);
        finish(vec![alice, bob]).await;
    })
    .await;
}
