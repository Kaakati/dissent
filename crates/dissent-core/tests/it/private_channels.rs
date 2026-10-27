//! Private channels: who can read one, and who stops reading it when they are removed. See
//! `docs/design/11-private-channels.md`. Networked, so run through
//! `scripts/fwtest.sh dissent-core it --ignored private_channels`.

use crate::common::*;
use dissent_core::ChannelId;
use std::time::Duration;

fn channel_named(engine: &dissent_core::Engine, id: dissent_core::CommunityId, name: &str) -> ChannelId {
    channel_id(engine, id, name).unwrap_or_else(|| panic!("channel {name} is listed"))
}

fn lists_channel(engine: &dissent_core::Engine, id: dissent_core::CommunityId, name: &str) -> bool {
    channel_id(engine, id, name).is_some()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_listed_member_reads_a_private_channel_and_others_do_not() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;

        alice
            .engine
            .create_channel(id, "vault".into())
            .await
            .expect("vault");
        let vault = channel_named(&alice.engine, id, "vault");
        alice
            .engine
            .set_channel_access(id, vault, true, vec![], vec![bob.identity.root_id()])
            .await
            .expect("alice makes vault private for bob");

        // Bob sees the channel once the access change reaches him.
        eventually("bob lists the private channel", || {
            lists_channel(&bob.engine, id, "vault").then_some(())
        })
        .await;

        alice
            .engine
            .send_message(id, vault, "eyes only".into())
            .await
            .expect("send into the private channel");
        wait_seen(&bob, id, vault, "eyes only").await;

        // Carol is in the community but not listed on the channel, so she gets neither the name nor the text.
        alice
            .engine
            .create_channel(id, "lounge".into())
            .await
            .expect("lounge");
        let lounge = channel_named(&alice.engine, id, "lounge");
        alice
            .engine
            .send_message(id, lounge, "marker".into())
            .await
            .expect("send marker");
        wait_seen(&carol, id, lounge, "marker").await;
        assert!(
            !lists_channel(&carol.engine, id, "vault"),
            "carol does not list the private channel"
        );
        assert!(
            !has_message(&carol.engine, id, vault, "eyes only"),
            "carol reads none of the private text"
        );

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn removing_a_reader_cuts_off_new_messages() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;

        alice
            .engine
            .create_channel(id, "vault".into())
            .await
            .expect("vault");
        let vault = channel_named(&alice.engine, id, "vault");
        alice
            .engine
            .set_channel_access(id, vault, true, vec![], vec![bob.identity.root_id()])
            .await
            .expect("bob reads vault");
        eventually("bob lists the private channel", || {
            lists_channel(&bob.engine, id, "vault").then_some(())
        })
        .await;
        alice
            .engine
            .send_message(id, vault, "before the cut".into())
            .await
            .expect("send before the removal");
        wait_seen(&bob, id, vault, "before the cut").await;

        // The removal starts a new generation: the next message is sealed under a key bob never gets.
        alice
            .engine
            .set_channel_access(id, vault, true, vec![], vec![])
            .await
            .expect("alice removes bob from vault");
        alice
            .engine
            .send_message(id, vault, "after the cut".into())
            .await
            .expect("send after the removal");

        // A marker in a public channel shows that bob has caught up on the community's events.
        alice
            .engine
            .create_channel(id, "lounge".into())
            .await
            .expect("lounge");
        let lounge = channel_named(&alice.engine, id, "lounge");
        alice
            .engine
            .send_message(id, lounge, "caught up".into())
            .await
            .expect("send marker");
        wait_seen(&bob, id, lounge, "caught up").await;
        eventually_within(
            Duration::from_secs(30),
            "bob stops listing the private channel",
            || (!lists_channel(&bob.engine, id, "vault")).then_some(()),
        )
        .await;
        assert!(
            !has_message(&bob.engine, id, vault, "after the cut"),
            "bob reads nothing sent after his removal"
        );

        finish(vec![alice, bob]).await;
    })
    .await;
}

/// Alice makes `vault` private for bob, sends a message, then adds carol as a reader. Returns once
/// carol lists the channel and alice has sent one more message after the change.
async fn newcomer_added_after_a_message(
    history: dissent_events::HistorySetting,
) -> (Peer, Peer, Peer, dissent_core::CommunityId, ChannelId) {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let id = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(id).await.expect("invite");
    join_ready(&alice, &carol, &invite).await;
    alice
        .engine
        .set_history_visibility(id, history)
        .await
        .expect("history setting");

    alice
        .engine
        .create_channel(id, "vault".into())
        .await
        .expect("vault");
    let vault = channel_named(&alice.engine, id, "vault");
    alice
        .engine
        .set_channel_access(id, vault, true, vec![], vec![bob.identity.root_id()])
        .await
        .expect("bob reads vault");
    eventually("bob lists the private channel", || {
        lists_channel(&bob.engine, id, "vault").then_some(())
    })
    .await;
    alice
        .engine
        .send_message(id, vault, "before carol".into())
        .await
        .expect("send before the newcomer");
    wait_seen(&bob, id, vault, "before carol").await;

    alice
        .engine
        .set_channel_access(
            id,
            vault,
            true,
            vec![],
            vec![bob.identity.root_id(), carol.identity.root_id()],
        )
        .await
        .expect("carol joins vault");
    eventually("carol lists the private channel", || {
        lists_channel(&carol.engine, id, "vault").then_some(())
    })
    .await;
    alice
        .engine
        .send_message(id, vault, "after carol".into())
        .await
        .expect("send after the newcomer");
    wait_seen(&carol, id, vault, "after carol").await;
    (alice, bob, carol, id, vault)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_newcomer_under_since_join_reads_from_the_join_on() {
    bounded(async {
        let (alice, bob, carol, id, vault) =
            newcomer_added_after_a_message(dissent_events::HistorySetting::SinceJoin).await;
        // The keys carol can ask for now are the new generation's only. Give her device time to try for
        // the older one, which the responders refuse.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !has_message(&carol.engine, id, vault, "before carol"),
            "carol reads nothing from before she joined"
        );
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_newcomer_under_all_reads_the_earlier_messages() {
    bounded(async {
        let (alice, bob, carol, id, vault) =
            newcomer_added_after_a_message(dissent_events::HistorySetting::All).await;
        wait_seen(&carol, id, vault, "before carol").await;
        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

fn message_with(
    engine: &dissent_core::Engine,
    id: dissent_core::CommunityId,
    channel: ChannelId,
    text: &str,
) -> Option<dissent_core::MessageItem> {
    engine
        .channel_messages(id, channel, 100_000)
        .into_iter()
        .find(|m| m.text == text)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn replies_reactions_edits_and_deletes_work_in_a_private_channel() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .create_channel(id, "vault".into())
            .await
            .expect("vault");
        let vault = channel_named(&alice.engine, id, "vault");
        alice
            .engine
            .set_channel_access(id, vault, true, vec![], vec![bob.identity.root_id()])
            .await
            .expect("bob reads vault");
        eventually("bob lists the private channel", || {
            lists_channel(&bob.engine, id, "vault").then_some(())
        })
        .await;

        alice
            .engine
            .send_message(id, vault, "original".into())
            .await
            .expect("post");
        wait_seen(&bob, id, vault, "original").await;
        let original: dissent_core::EventId = message_with(&bob.engine, id, vault, "original")
            .expect("bob has the original")
            .id
            .parse()
            .expect("id");

        // Bob replies, and reacts to the original.
        bob.engine
            .send_reply(id, vault, original, "answer".into())
            .await
            .expect("reply");
        bob.engine
            .react(id, vault, original, "👍".into())
            .await
            .expect("react");
        wait_seen(&alice, id, vault, "answer").await;
        eventually("alice sees bob's reaction", || {
            let m = message_with(&alice.engine, id, vault, "original")?;
            m.reactions.iter().any(|r| r.emoji == "👍" && r.count == 1).then_some(())
        })
        .await;
        let answer: dissent_core::EventId = message_with(&alice.engine, id, vault, "answer")
            .expect("alice has the reply")
            .id
            .parse()
            .expect("id");
        assert!(
            message_with(&alice.engine, id, vault, "answer")
                .expect("reply")
                .reply_to
                .is_some(),
            "the reply points at its original"
        );

        // Alice edits the original, and deletes the reply.
        alice
            .engine
            .edit_message(id, vault, original, "edited".into())
            .await
            .expect("edit");
        wait_seen(&bob, id, vault, "edited").await;
        assert!(message_with(&bob.engine, id, vault, "edited").expect("edited").edited);
        alice
            .engine
            .delete_message(id, vault, answer)
            .await
            .expect("delete");
        eventually("bob sees the reply deleted", || {
            bob.engine
                .channel_messages(id, vault, 100_000)
                .into_iter()
                .find(|m| m.id == answer.to_string())
                .filter(|m| m.deleted)
                .map(|_| ())
        })
        .await;

        finish(vec![alice, bob]).await;
    })
    .await;
}
