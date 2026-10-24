//! Storage-only seeders on loopback. A storage member keeps and serves the community's history,
//! but holds no MLS group state, so it cannot read messages, and it cannot post.


use super::tests::{eventually, general, Node};
use crate::CommunityId;


#[tokio::test(flavor = "multi_thread")]
async fn storage_seeder_serves_history_and_is_never_in_the_group() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Stored".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let channel = general(&alice.engine, id);
        let invite = alice.engine.create_invite(id).await.expect("invite");

        // The seeder joins as storage: it syncs the history but is never added to the group.
        let seeder = Node::named("seeder").await;
        seeder
            .engine
            .join_as_storage(&invite)
            .await
            .expect("seeder joins as storage");
        for text in ["one", "two", "three"] {
            alice
                .engine
                .send_message(id, channel, text.into())
                .await
                .expect("alice posts");
        }
        eventually("the seeder holds the history", || {
            seeder.engine.channel_messages(id, channel, 100).len() == 3
        })
        .await;

        // It cannot read any of it: the text is locked, and it has no group state.
        assert!(seeder
            .engine
            .channel_messages(id, channel, 100)
            .iter()
            .all(|m| m.locked && m.text.is_empty()));
        assert_eq!(seeder.engine.mls_epoch(id), None);

        // It cannot post, either.
        assert!(seeder
            .engine
            .send_message(id, channel, "from storage".into())
            .await
            .is_err());

        // The owner goes offline. A later joiner gets the history from the seeder, still locked.
        let (alice_id, alice_dir, alice_port) = alice.stop().await;
        let seeder_invite = seeder.engine.create_invite(id).await.expect("storage invites");
        let carol = Node::named("carol").await;
        carol
            .engine
            .join_community(&seeder_invite)
            .await
            .expect("carol joins through the seeder");
        eventually("carol holds the history", || {
            carol.engine.channel_messages(id, channel, 100).len() == 3
        })
        .await;
        assert!(carol
            .engine
            .channel_messages(id, channel, 100)
            .iter()
            .all(|m| m.locked));
        assert_eq!(carol.engine.mls_epoch(id), None, "no person was online to add her");

        // A person comes back. Carol is added to the group, and reads what is posted from now on.
        let alice = Node::boot(alice_id, alice_dir, alice_port).await;
        eventually("alice knows carol", || {
            alice.engine.community_view(id).map(|v| v.members.len()).ok() == Some(3)
        })
        .await;
        carol.engine.mls_join_missing(id).await;
        eventually("carol is in the group", || carol.engine.mls_epoch(id).is_some()).await;
        alice
            .engine
            .send_message(id, channel, "after carol".into())
            .await
            .expect("alice posts");
        eventually("carol reads it", || {
            carol
                .engine
                .channel_messages(id, channel, 100)
                .iter()
                .any(|m| m.text == "after carol" && !m.locked)
        })
        .await;
    })
    .await;
}
