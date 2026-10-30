//! Personal profile banners. A member of a shared server and a contact with no shared server both
//! see a new banner on the person's profile, and a preset and a removal travel too. The networked
//! tests are `#[ignore]`d and run through `scripts/fwtest.sh`.

use crate::common::*;

/// Stand-in bytes for a cropped 16:9 WebP. Larger than an avatar may be, under the banner cap.
fn banner(tag: u8) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 300 * 1024));
    bytes
}

#[tokio::test(flavor = "multi_thread")]
async fn my_banner_is_on_my_profile_and_survives_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        assert!(alice.engine.set_profile_banner(vec![1; 512 * 1024 + 1]).await.is_err());
        assert!(alice.engine.set_profile_banner_preset(10).await.is_err());
        alice.engine.set_profile_banner(banner(1)).await.expect("set banner");
        let path = alice.engine.me().banner.expect("the banner has a file");
        assert_eq!(picture(&alice.engine, &path), banner(1));
        assert_eq!(std::path::Path::new(&path).parent(), Some(alice.dir.join("avatars").as_path()));
        let me = alice.identity.root_id();
        assert_eq!(alice.engine.user_profile(me).banner.as_deref(), Some(path.as_str()));

        let (identity, dir) = stop(alice).await;
        let again = start(identity, dir).await;
        assert_eq!(again.engine.me().banner.as_deref(), Some(path.as_str()));
        again.engine.set_profile_banner_preset(3).await.expect("preset");
        let me = again.engine.me();
        assert_eq!((me.banner_preset, me.banner), (Some(3), None));
        again.engine.clear_profile_banner().await.expect("clear");
        let me = again.engine.me();
        assert_eq!((me.banner_preset, me.banner), (None, None));
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_server_member_and_a_contact_see_a_new_banner() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        // Bob shares a server with Alice. Carol is only her contact.
        rustaceans(&alice, &bob).await;
        make_contacts_ready(&alice, &carol).await;
        let alice_root = alice.identity.root_id();

        let bytes = banner(7);
        alice.engine.set_profile_banner(bytes.clone()).await.expect("alice sets a banner");
        for (who, engine) in [("bob, in the server", &bob.engine), ("carol, a contact", &carol.engine)] {
            let path = eventually(&format!("{who} fetches alice's banner"), || engine.user_profile(alice_root).banner).await;
            assert_eq!(picture(engine, &path), bytes, "{who}");
        }

        alice.engine.set_profile_banner_preset(5).await.expect("preset");
        for (who, engine) in [("bob", &bob.engine), ("carol", &carol.engine)] {
            eventually(&format!("{who} sees the preset"), || {
                let v = engine.user_profile(alice_root);
                (v.banner_preset == Some(5) && v.banner.is_none()).then_some(())
            })
            .await;
        }
        alice.engine.clear_profile_banner().await.expect("clear");
        for (who, engine) in [("bob", &bob.engine), ("carol", &carol.engine)] {
            eventually(&format!("{who} sees the banner removed"), || {
                let v = engine.user_profile(alice_root);
                (v.banner_preset.is_none() && v.banner.is_none()).then_some(())
            })
            .await;
        }
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}
