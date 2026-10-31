//! Invite previews and short invites on loopback. A preview shows the community's name and counts,
//! a forged secret gets none, a loopback engine makes long invites, and a short invite needs the DHT.

use crate::common::*;
use dissent_core::{CommunityId, InviteStatus};
use dissent_discovery::{Invite, InviteLink, ShortInvite};

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_shows_the_name_and_counts_and_then_already_member() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id: CommunityId = alice
            .engine
            .create_community("Vault".into())
            .await
            .expect("alice creates the community")
            .id
            .parse()
            .unwrap();
        let link = alice.engine.create_invite(id).await.expect("invite");
        assert!(
            matches!(InviteLink::parse(&link), Ok(InviteLink::Full(_))),
            "a loopback engine has no DHT, so its invites use the long form: {link}"
        );

        let before = bob.engine.invite_preview(&link).await.expect("bob previews the invite");
        assert_eq!(before.name, "Vault");
        assert_eq!(before.member_count, 1);
        assert_eq!(before.invite, InviteStatus::Live);
        assert!(!before.already_member);

        bob.engine.join_community(&link).await.expect("bob joins");
        eventually("alice sees two members", || {
            alice.engine.community_view(id).ok().filter(|v| v.members.len() == 2)
        })
        .await;
        let after = bob.engine.invite_preview(&link).await.expect("bob previews again");
        assert!(after.already_member, "{after:?}");
        assert_eq!(after.member_count, 2);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forged_secret_gets_no_preview() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Gated".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let honest = alice.engine.create_invite(id).await.expect("invite");
        let mut forged = Invite::parse(&honest).expect("parse");
        forged.secret = [9; 32];

        let mallory = peer("mallory").await;
        let refused = mallory.engine.invite_preview(forged.to_url().as_str()).await;
        assert!(refused.is_err(), "a preview needs the real secret: {refused:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_invite_needs_the_dht_and_says_so() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Short".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let long = Invite::parse(&alice.engine.create_invite(id).await.expect("invite")).expect("parse");
        let short = ShortInvite {
            community: id,
            secret: long.secret,
            secret_generation: long.secret_generation,
            invite_id: long.invite_id,
        }
        .to_url()
        .to_string();

        let bob = peer("bob").await;
        let err = bob.engine.join_community(&short).await.expect_err("no DHT on loopback");
        assert!(err.to_string().contains("DHT"), "{err}");
        assert!(bob.engine.community_view(id).is_err(), "a refused join joins nothing");
        assert!(bob.engine.invite_preview(&short).await.is_err());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_full_link_is_the_long_form_that_older_versions_open() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Full".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let full = alice.engine.create_full_invite(id).await.expect("full invite");
        assert!(matches!(InviteLink::parse(&full), Ok(InviteLink::Full(_))), "{full}");
        let bob = peer("bob").await;
        bob.engine.join_community(&full).await.expect("a long link still joins");
    })
    .await;
}

/// A revoked invite gets no preview, as a forged secret gets none, so its holder learns no counts (privacy audit P-16).
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_invite_gets_no_preview() {
    bounded(async {
        let alice = peer("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Closed".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let link = alice.engine.create_invite(id).await.expect("invite");
        let invite_id = InviteLink::parse(&link).expect("parse").invite_id().expect("the invite has an id");
        alice.engine.revoke_invite(id, invite_id).await.expect("alice revokes it");

        let mallory = peer("mallory").await;
        let refused = mallory.engine.invite_preview(&link).await;
        let err = refused.expect_err("a revoked invite gets no preview");
        assert!(err.to_string().contains("no member answered"), "the same error as for a forged secret: {err}");
        finish(vec![alice, mallory]).await;
    })
    .await;
}
