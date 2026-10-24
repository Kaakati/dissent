//! The access gate on loopback: an invite with the wrong secret cannot join, and tokens bind a
//! request to its community.

use dissent_discovery::Invite;
use dissent_identity::CommunityId;

use crate::membership::tests::Node;

#[tokio::test(flavor = "multi_thread")]
async fn an_invite_with_the_wrong_secret_cannot_join() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Gated".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let honest = alice.engine.create_invite(id).await.expect("invite");

        // The same genesis and peers, with a secret the founder never issued.
        let mut forged = Invite::parse(&honest).expect("parse");
        forged.secret = [9; 32];
        let mallory = Node::named("mallory").await;
        let refused = mallory.engine.join_community(forged.to_url().as_str()).await;
        assert!(refused.is_err(), "a join with the wrong secret must fail");
        assert!(
            mallory.engine.community_view(id).is_err(),
            "mallory must not hold the community"
        );

        // The real invite still works.
        mallory
            .engine
            .join_community(&honest)
            .await
            .expect("the honest invite joins");
    })
    .await;
}
