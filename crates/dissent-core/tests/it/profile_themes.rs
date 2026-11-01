//! Profile colours. A member of a shared server and a contact with no shared server both see a new theme on the
//! person's profile, a clear travels too, and a contact made after the change still gets it. The networked tests are
//! `#[ignore]`d and run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_events::ProfileTheme;

const YELLOW: [u8; 3] = [255, 255, 0];
const NAVY: [u8; 3] = [10, 20, 90];

#[tokio::test(flavor = "multi_thread")]
async fn my_theme_is_on_my_profile_is_sealed_and_survives_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        let me = alice.identity.root_id();
        assert_eq!(alice.engine.me().theme, None, "no theme by default");
        alice.engine.set_profile_theme(NAVY, YELLOW).await.expect("set");
        let theme = Some(ProfileTheme { primary: NAVY, accent: YELLOW });
        assert_eq!(alice.engine.me().theme, theme);
        assert_eq!(alice.engine.user_profile(me).theme, theme);

        let on_disk = std::fs::read(alice.dir.join("themes.bin")).expect("the store exists");
        assert!(!on_disk.windows(6).any(|w| w == [10, 20, 90, 255, 255, 0]), "the colours must be sealed at rest");

        let (identity, dir) = stop(alice).await;
        let again = start(identity, dir).await;
        assert_eq!(again.engine.me().theme, theme);
        again.engine.clear_profile_theme().await.expect("clear");
        assert_eq!(again.engine.me().theme, None);
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_server_member_and_a_contact_see_a_new_theme_and_its_removal() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        // Bob shares a server with Alice. Carol is only her contact.
        rustaceans(&alice, &bob).await;
        make_contacts_ready(&alice, &carol).await;
        let alice_root = alice.identity.root_id();

        alice.engine.set_profile_theme(NAVY, YELLOW).await.expect("set");
        let theme = ProfileTheme { primary: NAVY, accent: YELLOW };
        for (who, engine) in [("bob, in the server", &bob.engine), ("carol, a contact", &carol.engine)] {
            eventually(&format!("{who} sees the theme"), || (engine.user_profile(alice_root).theme == Some(theme)).then_some(())).await;
        }
        alice.engine.clear_profile_theme().await.expect("clear");
        for (who, engine) in [("bob", &bob.engine), ("carol", &carol.engine)] {
            eventually(&format!("{who} sees the theme removed"), || engine.user_profile(alice_root).theme.is_none().then_some(())).await;
        }
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_contact_made_after_the_change_gets_the_theme_both_ways() {
    bounded(async {
        let alice = peer("alice").await;
        let carol = peer("carol").await;
        alice.engine.set_profile_theme(NAVY, YELLOW).await.expect("alice");
        carol.engine.set_profile_theme(YELLOW, NAVY).await.expect("carol");
        make_contacts_ready(&alice, &carol).await;
        let (alice_root, carol_root) = (alice.identity.root_id(), carol.identity.root_id());
        eventually("carol sees alice's theme", || {
            (carol.engine.user_profile(alice_root).theme == Some(ProfileTheme { primary: NAVY, accent: YELLOW })).then_some(())
        })
        .await;
        eventually("alice sees carol's theme", || {
            (alice.engine.user_profile(carol_root).theme == Some(ProfileTheme { primary: YELLOW, accent: NAVY })).then_some(())
        })
        .await;
        finish(vec![alice, carol]).await;
    })
    .await;
}
