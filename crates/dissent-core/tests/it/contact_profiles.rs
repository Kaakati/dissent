//! Direct profiles between contacts who share no server. A picture set before two people became
//! contacts used to never arrive, since a direct profile went out only when it changed. Now each run
//! exchanges profiles with a contact the first time it hears their beacon. A device on the old path
//! (v0.1.6) still gets ours pushed. Networked: run through `scripts/fwtest.sh`.

use crate::common::*;
use std::time::Duration;

/// Stand-in bytes for a cropped WebP. The engine stores what it is given.
fn picture(tag: u8) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 200));
    bytes
}

fn avatar_of(viewer: &Peer, of: &Peer) -> Option<Vec<u8>> {
    let path = viewer.engine.user_profile(of.identity.root_id()).avatar?;
    std::fs::read(path).ok()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_picture_set_before_the_contact_arrives_both_ways_and_after_a_restart() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        // Both set a picture first, then become contacts. They share no server.
        alice.engine.set_avatar(picture(1)).await.expect("alice's picture");
        bob.engine.set_avatar(picture(2)).await.expect("bob's picture");
        make_contacts_ready(&alice, &bob).await;

        let got = eventually("alice sees bob's picture", || avatar_of(&alice, &bob)).await;
        assert_eq!(got, picture(2));
        let got = eventually("bob sees alice's picture", || avatar_of(&bob, &alice)).await;
        assert_eq!(got, picture(1));

        let (identity, dir) = stop(alice).await;
        let alice = start(identity, dir).await;
        assert_eq!(avatar_of(&alice, &bob), Some(picture(2)), "the picture is kept across a restart");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_contact_on_the_old_path_gets_our_picture_and_sends_theirs_on_change() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        dissent_core::test_support::act_as_v016_profiles(&bob.engine);
        alice.engine.set_avatar(picture(3)).await.expect("alice's picture");
        bob.engine.set_avatar(picture(4)).await.expect("bob's picture");
        make_contacts_ready(&alice, &bob).await;

        // Alice is on the current path: her sync falls back to a push, which the old path takes.
        let got = eventually("bob, on the old path, sees alice's picture", || avatar_of(&bob, &alice)).await;
        assert_eq!(got, picture(3));
        // The old path has no way to answer for its own earlier picture.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(avatar_of(&alice, &bob), None, "an old peer cannot send an earlier picture");
        // It sends a new one when it changes, as it always did.
        bob.engine.set_avatar(picture(5)).await.expect("bob changes his picture");
        let got = eventually("alice sees bob's new picture", || avatar_of(&alice, &bob)).await;
        assert_eq!(got, picture(5));
        finish(vec![alice, bob]).await;
    })
    .await;
}
