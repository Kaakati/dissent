//! A seeder keeps a server's pictures available while its owner is offline. The owner sets an icon, a server
//! banner, an avatar and a profile banner, and goes offline. A new member joins through the seeder's invite and
//! gets every picture from the seeder alone.

use crate::common::*;
use dissent_core::{CommunityId, Engine, RootId};

fn picture(tag: u8) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 200));
    bytes
}

/// Whether `engine` holds the server's icon and banner, the owner's avatar in the server, and the owner's profile
/// banner.
fn holds_every_picture(engine: &Engine, id: CommunityId, owner: RootId) -> Option<()> {
    let view = engine.community_view(id).ok()?;
    let member = view.members.iter().find(|m| m.root_id == owner.to_string())?;
    let profile = engine.user_profile(owner);
    let all = view.icon.is_some() && view.banner.is_some() && member.avatar.is_some() && profile.banner.is_some();
    all.then_some(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_new_member_gets_the_pictures_from_the_seeder_alone() {
    bounded(async {
        let alice = peer("alice").await;
        let server = alice
            .engine
            .create_community("Pictures".into())
            .await
            .expect("create community");
        let id: CommunityId = server.id.parse().unwrap();
        alice.engine.set_community_icon(id, picture(1)).await.expect("icon");
        alice.engine.set_community_banner(id, picture(2)).await.expect("server banner");
        alice.engine.set_avatar(picture(3)).await.expect("avatar");
        alice.engine.set_profile_banner(picture(4)).await.expect("profile banner");
        let alice_root = alice.identity.root_id();
        let invite = alice.engine.create_invite(id).await.expect("invite");

        // The seeder joins as storage and fetches what it sees.
        let seeder = peer("seeder").await;
        seeder.engine.join_as_storage(&invite).await.expect("seeder joins as storage");
        eventually("the seeder holds every picture", || holds_every_picture(&seeder.engine, id, alice_root)).await;

        // Alice goes offline. A new member joins through the seeder's own invite.
        stop(alice).await;
        let seeder_invite = seeder.engine.create_invite(id).await.expect("seeder invite");
        let carol = peer("carol").await;
        carol.engine.join_community(&seeder_invite).await.expect("carol joins through the seeder");
        eventually("carol gets every picture from the seeder", || holds_every_picture(&carol.engine, id, alice_root)).await;
    })
    .await;
}
