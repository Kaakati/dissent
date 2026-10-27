//! Server Settings: the owner renames a server and sets its banner. The name is trimmed and checked,
//! both survive a restart, a member cannot change them, and a member and an invite card see the new
//! name and banner. The networked tests are `#[ignore]`d and run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_core::{CommunityId, Engine};
use dissent_events::Trait;

fn traits(items: &[(&str, &str)]) -> Vec<Trait> {
    items
        .iter()
        .map(|(emoji, label)| Trait {
            emoji: (*emoji).into(),
            label: (*label).into(),
        })
        .collect()
}

/// Stand-in bytes for a cropped WebP banner. Larger than an icon may be, under the banner cap.
fn banner(tag: u8) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 300 * 1024));
    bytes
}

fn name_of(engine: &Engine, id: CommunityId) -> Option<String> {
    engine
        .communities()
        .into_iter()
        .find(|c| c.id == id.to_string())
        .map(|c| c.name)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_and_a_banner_are_checked_and_survive_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice.engine.create_community("Makers".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();

        // The creation rules: trimmed, not empty, at most 64 characters.
        assert!(alice.engine.rename_community(id, "   ".into()).await.is_err());
        assert!(alice.engine.rename_community(id, "x".repeat(65)).await.is_err());
        alice.engine.rename_community(id, "  Makers Guild  ".into()).await.expect("rename");
        assert_eq!(name_of(&alice.engine, id).as_deref(), Some("Makers Guild"));
        assert_eq!(alice.engine.community_view(id).unwrap().name, "Makers Guild");

        // A banner has its own cap, above the icon's 256 KB.
        assert!(alice.engine.set_community_banner(id, vec![1; 512 * 1024 + 1]).await.is_err());
        assert!(alice.engine.set_community_icon(id, banner(2)).await.is_err(), "an icon stays under 256 KB");
        alice.engine.set_community_banner(id, banner(3)).await.expect("set banner");
        let path = alice.engine.community_view(id).unwrap().banner.expect("the banner has a file");
        assert_eq!(std::fs::read(&path).unwrap(), banner(3));
        // The file is in `<profile>/avatars`, the one folder the desktop app opens to the webview
        // (`avatar_cmds::allow_avatars`), and its extension gives the asset protocol its MIME type.
        let file = std::path::Path::new(&path);
        assert_eq!(file.parent(), Some(alice.dir.join("avatars").as_path()), "{path}");
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("webp"));

        let (identity, dir) = stop(alice).await;
        let again = start(identity, dir).await;
        assert_eq!(name_of(&again.engine, id).as_deref(), Some("Makers Guild"));
        let view = again.engine.community_view(id).unwrap();
        assert_eq!(view.banner.as_deref(), Some(path.as_str()));

        again.engine.clear_community_banner(id).await.expect("clear banner");
        assert!(again.engine.community_view(id).unwrap().banner.is_none());

        // A preset is a small id, not a picture. An unknown one is refused.
        assert!(again.engine.set_community_banner_preset(id, 10).await.is_err());
        again.engine.set_community_banner_preset(id, 3).await.expect("preset");
        let view = again.engine.community_view(id).unwrap();
        assert_eq!((view.banner_preset, view.banner), (Some(3), None));

        // The profile: trimmed, and refused whole when over a cap.
        let six = traits(&[("🎮", "a"), ("🎨", "b"), ("🎵", "c"), ("📚", "d"), ("🌱", "e"), ("🍕", "f")]);
        assert!(again.engine.set_community_profile(id, String::new(), six).await.is_err());
        assert!(again.engine.set_community_profile(id, "d".repeat(301), Vec::new()).await.is_err());
        assert!(again.engine.set_community_profile(id, String::new(), traits(&[("🎮", &"x".repeat(25))])).await.is_err());
        again
            .engine
            .set_community_profile(id, "  We build things.  ".into(), traits(&[("🛠️", " Tools ")]))
            .await
            .expect("profile");
        let view = again.engine.community_view(id).unwrap();
        assert_eq!(view.description, "We build things.");
        assert_eq!(view.traits, traits(&[("🛠️", "Tools")]));
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_member_sees_the_new_name_and_banner_and_cannot_change_them() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;

        // A member is refused before anything is sent.
        assert!(bob.engine.rename_community(id, "Bob's".into()).await.is_err());
        assert!(bob.engine.set_community_banner(id, banner(4)).await.is_err());

        alice.engine.rename_community(id, "Crustaceans".into()).await.expect("the owner renames");
        let bytes = banner(5);
        alice.engine.set_community_banner(id, bytes.clone()).await.expect("the owner sets a banner");
        eventually("bob sees the new name", || {
            (name_of(&bob.engine, id).as_deref() == Some("Crustaceans")).then_some(())
        })
        .await;
        let path = eventually("bob fetches the banner", || bob.engine.community_view(id).ok()?.banner).await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Someone holding an invite, not yet a member, sees the new name and the banner on the card.
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let card = carol.engine.invite_preview(&invite).await.expect("carol previews the invite");
        assert!(!card.already_member);
        assert_eq!(card.name, "Crustaceans", "the owner's rename proves the name");
        let card_banner = card.banner.expect("the card carries the banner");
        assert_eq!(std::fs::read(card_banner).unwrap(), bytes);

        // The profile and a preset banner reach the member and a fresh card.
        alice
            .engine
            .set_community_profile(id, "Crabs welcome.".into(), traits(&[("🦀", "Rust")]))
            .await
            .expect("profile");
        alice.engine.set_community_banner_preset(id, 6).await.expect("preset");
        eventually("bob sees the profile and the preset", || {
            let v = bob.engine.community_view(id).ok()?;
            (v.description == "Crabs welcome." && v.banner_preset == Some(6)).then_some(())
        })
        .await;
        let dave = peer("dave").await;
        let card = dave.engine.invite_preview(&invite).await.expect("dave previews the invite");
        assert_eq!(card.description, "Crabs welcome.");
        assert_eq!(card.traits, traits(&[("🦀", "Rust")]));
        assert_eq!((card.banner_preset, card.banner), (Some(6), None));
        finish(vec![dave]).await;

        // The owner removes the banner, and the member sees it go.
        alice.engine.clear_community_banner(id).await.expect("clear banner");
        eventually("bob sees the banner removed", || {
            let v = bob.engine.community_view(id).ok()?;
            (v.banner.is_none() && v.banner_preset.is_none()).then_some(())
        })
        .await;
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

/// The set-banner call writes the event locally and returns. It does not wait for a member who is
/// offline: publishing to peers happens in the background.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn setting_a_banner_returns_quickly_with_every_member_offline() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        // Bob goes offline. Alice's gossip neighbour is now a dead peer.
        let (_identity, _dir) = stop(bob).await;
        let t = std::time::Instant::now();
        alice.engine.set_community_banner(id, banner(6)).await.expect("set banner");
        let took = t.elapsed();
        assert!(took < std::time::Duration::from_secs(1), "set-banner took {took:?}");
        let t = std::time::Instant::now();
        alice.engine.set_community_banner_preset(id, 2).await.expect("preset");
        alice.engine.rename_community(id, "Offline test".into()).await.expect("rename");
        alice
            .engine
            .set_community_profile(id, "Quiet here.".into(), Vec::new())
            .await
            .expect("profile");
        let took = t.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "the other settings took {took:?}");
        let path = alice.engine.community_view(id).unwrap();
        assert_eq!(path.banner_preset, Some(2));
        finish(vec![alice]).await;
    })
    .await;
}

/// Over the real network (relays and the DHT): the same as above, for a member who went offline
/// after joining. Run with `scripts/fwtest.sh dissent-core it --ignored internet_banner --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the internet: run with scripts/fwtest.sh"]
async fn internet_banner_with_an_offline_member_returns_quickly() {
    bounded(async {
        let start_real = |name: &'static str| async move {
            let identity = dissent_core::Identity::create(now());
            let dir = scratch(name);
            let (engine, events) = Engine::start(
                identity.clone(),
                dir.clone(),
                dissent_core::NodeConfig::default(),
                dissent_core::test_support::vault_for(&dir),
            )
            .await
            .expect("engine starts");
            Peer { engine, identity, dir, _events: events }
        };
        let alice = start_real("alice-banner-net").await;
        let bob = start_real("bob-banner-net").await;
        let id = rustaceans(&alice, &bob).await;
        let (_identity, _dir) = stop(bob).await;
        let mut slow = Vec::new();
        for round in 0..3 {
            let t = std::time::Instant::now();
            alice.engine.set_community_banner(id, banner(10 + round)).await.expect("set banner");
            let took = t.elapsed();
            eprintln!("banner {round} took {took:?}");
            if took > std::time::Duration::from_secs(1) {
                slow.push(took);
            }
        }
        let t = std::time::Instant::now();
        alice.engine.set_community_profile(id, "Still here.".into(), Vec::new()).await.expect("profile");
        eprintln!("profile took {:?}", t.elapsed());
        assert!(slow.is_empty(), "set-banner waited on the network: {slow:?}");
        finish(vec![alice]).await;
    })
    .await;
}

/// A small looping source GIF of `frames` frames, made with the `image` crate.
fn source_gif(frames: usize) -> Vec<u8> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, Rgba, RgbaImage};
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(Repeat::Finite(1)).unwrap();
        for i in 0..frames {
            let img = RgbaImage::from_fn(320, 200, |x, y| Rgba([(i * 60) as u8, x as u8, y as u8, 255]));
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1))).unwrap();
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_animated_banner_reaches_a_member_still_looping() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let crop = dissent_core::CropRect { x: 0, y: 10, w: 320, h: 180, rot: 0 };
        let prepared = alice.engine.prepare_animated_banner(source_gif(3), crop).await.expect("encode");
        alice
            .engine
            .set_community_banner_animated(id, prepared.gif.clone(), prepared.poster.clone())
            .await
            .expect("the owner sets an animated banner");
        let (gif, poster) = eventually("bob fetches the GIF and its poster", || {
            let v = bob.engine.community_view(id).ok()?;
            Some((v.banner?, v.banner_poster?))
        })
        .await;
        let bytes = std::fs::read(&gif).unwrap();
        assert_eq!(bytes, prepared.gif);
        assert!(gif.ends_with(".gif"), "{gif}");
        assert!(bytes.windows(11).any(|w| w == b"NETSCAPE2.0"), "it still loops");
        assert_eq!(std::fs::read(&poster).unwrap(), prepared.poster);
        // The same works for a personal banner, which bob sees on alice's profile.
        alice.engine.set_profile_banner_animated(prepared.gif.clone(), prepared.poster).await.expect("profile");
        let alice_root = alice.identity.root_id();
        let path = eventually("bob sees alice's animated profile banner", || bob.engine.user_profile(alice_root).banner).await;
        assert_eq!(std::fs::read(path).unwrap(), prepared.gif);
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn an_animated_icon_reaches_a_member_as_a_square_with_its_poster() {
    use image::ImageDecoder;
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        // A 200 by 200 square from the 320 by 200 source: the crop's shorter side, centred.
        let crop = dissent_core::CropRect { x: 0, y: 0, w: 200, h: 200, rot: 0 };
        let prepared = alice.engine.prepare_animated_icon(source_gif(3), crop).await.expect("encode");
        alice
            .engine
            .set_community_icon_animated(id, prepared.gif.clone(), prepared.poster.clone())
            .await
            .expect("the owner sets an animated icon");
        let (gif, poster) = eventually("bob fetches the icon GIF and its poster", || {
            let v = bob.engine.community_view(id).ok()?;
            Some((v.icon?, v.icon_poster?))
        })
        .await;
        let bytes = std::fs::read(&gif).unwrap();
        assert_eq!(bytes, prepared.gif);
        assert!(gif.ends_with(".gif"), "{gif}");
        assert!(bytes.windows(11).any(|w| w == b"NETSCAPE2.0"), "it still loops");
        let (w, h) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes)).unwrap().dimensions();
        assert_eq!(w, h, "the icon is square");
        assert!(w <= 256, "{w}");
        assert_eq!(std::fs::read(&poster).unwrap(), prepared.poster);

        // A later still icon replaces the animated one, so bob shows no poster and the still icon.
        alice.engine.clear_community_icon(id).await.expect("the owner removes the icon");
        eventually("bob's icon is removed", || bob.engine.community_view(id).ok()?.icon.is_none().then_some(())).await;
        assert!(bob.engine.community_view(id).unwrap().icon_poster.is_none());
        finish(vec![alice, bob]).await;
    })
    .await;
}
