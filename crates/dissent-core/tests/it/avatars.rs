//! Profile pictures and community icons. A contact sees a new picture, a member sees the server
//! icon, a member cannot change the icon, and a picture over the cap is refused. The networked tests
//! are `#[ignore]`d and run through `scripts/fwtest.sh`.

use crate::common::*;
use dissent_core::CommunityId;

/// Stand-in bytes for a cropped WebP. The engine stores what it is given, so any content does.
fn picture(tag: u8) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 200));
    bytes
}

/// The decrypted file the view points at must hold exactly the bytes that were set.
fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).expect("the picture file exists")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picture_over_256_kb_is_refused_and_nothing_is_set() {
    bounded(async {
        let alice = peer("alice").await;
        assert!(alice.engine.set_avatar(vec![7; 256 * 1024 + 1]).await.is_err());
        assert!(alice.engine.set_avatar(Vec::new()).await.is_err());
        assert!(alice.engine.me().avatar.is_none());
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn my_picture_is_on_my_profile_and_survives_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        let bytes = picture(1);
        alice.engine.set_avatar(bytes.clone()).await.expect("alice sets a picture");
        let path = alice.engine.me().avatar.expect("the picture has a file");
        assert_eq!(read(&path), bytes);

        let (identity, dir) = stop(alice).await;
        let again = start(identity, dir).await;
        assert_eq!(again.engine.me().avatar.as_deref(), Some(path.as_str()));
        finish(vec![again]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_contact_sees_a_new_picture_and_its_removal() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        make_contacts_ready(&alice, &bob).await;
        let alice_root = alice.identity.root_id();

        let bytes = picture(2);
        alice.engine.set_avatar(bytes.clone()).await.expect("alice sets a picture");
        let path = eventually("bob fetches alice's picture", || {
            bob.engine.user_profile(alice_root).avatar
        })
        .await;
        assert_eq!(read(&path), bytes);

        alice.engine.clear_avatar().await.expect("alice removes it");
        eventually("bob sees the picture removed", || {
            bob.engine.user_profile(alice_root).avatar.is_none().then_some(())
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_server_member_sees_the_icon_and_a_member_cannot_change_it() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let server = alice
            .engine
            .create_community("Makers".into())
            .await
            .expect("create community");
        let id: CommunityId = server.id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("create invite");
        bob.engine.join_community(&invite).await.expect("bob joins");
        let bob_root = bob.identity.root_id().to_string();
        eventually("alice lists bob as a member", || {
            alice
                .engine
                .community_view(id)
                .ok()
                .filter(|v| v.members.iter().any(|m| m.root_id == bob_root))
                .map(|_| ())
        })
        .await;

        // A member is refused before anything is sent.
        assert!(bob.engine.set_community_icon(id, picture(3)).await.is_err());
        assert!(bob.engine.clear_community_icon(id).await.is_err());

        let bytes = picture(4);
        alice
            .engine
            .set_community_icon(id, bytes.clone())
            .await
            .expect("the owner sets the icon");
        let icon_of = |engine: &dissent_core::Engine| {
            engine
                .communities()
                .into_iter()
                .find(|c| c.id == server.id)
                .and_then(|c| c.icon)
        };
        let path = eventually("bob fetches the icon", || icon_of(&bob.engine)).await;
        assert_eq!(read(&path), bytes);
        assert!(alice.engine.community_view(id).unwrap().icon.is_some());

        alice
            .engine
            .clear_community_icon(id)
            .await
            .expect("the owner removes the icon");
        eventually("bob sees the icon removed", || icon_of(&bob.engine).is_none().then_some(())).await;
        finish(vec![alice, bob]).await;
    })
    .await;
}
