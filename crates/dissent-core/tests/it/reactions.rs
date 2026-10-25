//! Reactions: the emoji is sealed under the community's group. Members count it, and a storage
//! member who cannot read the group sees the reaction exist but never learns which emoji.


use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
async fn members_see_the_emoji_and_a_storage_member_does_not() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let community = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(community).await.expect("invite");
    carol
        .engine
        .join_as_storage(&invite)
        .await
        .expect("carol joins as storage");
    let general = channel_id(&alice.engine, community, "general").expect("general channel");
    let posted = alice
        .engine
        .send_message(community, general, "react to me".into())
        .await
        .expect("alice posts");
    eventually("bob reads the message", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .any(|m| m.text == "react to me")
            .then_some(())
    })
    .await;

    let msg_id: dissent_core::EventId = posted.id.parse().expect("id");
    alice
        .engine
        .react(community, general, msg_id, "👍".into())
        .await
        .expect("alice reacts");
    eventually("bob counts the reaction", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .any(|m| m.id == posted.id && m.reactions.iter().any(|r| r.emoji == "👍" && r.count == 1))
            .then_some(())
    })
    .await;

    // Carol cannot read the group, so the reaction is not counted for her.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        carol
            .engine
            .channel_messages(community, general, 10)
            .iter()
            .all(|m| m.reactions.is_empty()),
        "a storage member does not learn the emoji"
    );

    alice
        .engine
        .unreact(community, general, msg_id, "👍".into())
        .await
        .expect("alice removes it");
    eventually("bob sees the reaction removed", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .any(|m| m.id == posted.id && m.reactions.is_empty())
            .then_some(())
    })
    .await;

    finish(vec![alice, bob, carol]).await;
}
