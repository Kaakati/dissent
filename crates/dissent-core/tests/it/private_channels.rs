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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn files_in_a_private_channel_reach_its_readers_only() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        // A storage member keeps the community's events. It cannot read a private channel.
        let seeder = peer("seeder").await;
        seeder
            .engine
            .join_as_storage(&invite)
            .await
            .expect("seeder joins as storage");

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

        let dir = scratch("private-files");
        let path = dir.join("vault-plan.txt");
        std::fs::write(&path, b"eyes only file").expect("write source file");
        let sent = alice
            .engine
            .send_files(id, vault, vec![path], "see the file".into(), true)
            .await
            .expect("alice posts a file in vault");
        assert_eq!(sent.attachments.len(), 1);
        let attachment = sent.attachments[0].id.clone();

        // Bob reads the message and downloads the file from its key in the channel.
        eventually("bob reads the file message", || {
            message_with(&bob.engine, id, vault, "see the file")
                .filter(|m| m.attachments.len() == 1)
                .map(|_| ())
        })
        .await;
        bob.engine
            .download_attachment(Some(id), sent.id.clone(), attachment.clone())
            .await
            .expect("bob downloads the file");
        eventually("bob has the file", || {
            message_with(&bob.engine, id, vault, "see the file")
                .and_then(|m| m.attachments.first().map(|a| a.state == "downloaded"))
                .filter(|done| *done)
                .map(|_| ())
        })
        .await;
        let local = message_with(&bob.engine, id, vault, "see the file")
            .and_then(|m| m.attachments.first().and_then(|a| a.local_path.clone()))
            .expect("bob has a local path for the file");
        assert_eq!(std::fs::read(&local).expect("read the downloaded file"), b"eyes only file");

        // Carol is in the community but not on the list: no channel, no text, no file.
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
            .expect("marker");
        wait_seen(&carol, id, lounge, "marker").await;
        assert!(!lists_channel(&carol.engine, id, "vault"));
        assert!(message_with(&carol.engine, id, vault, "see the file").is_none());

        // The seeder stores the message's event, and the file's hash is not in its log.
        let log = seeder.dir.join("communities").join(id.to_string()).join("events.log");
        let message_id: dissent_core::EventId = sent.id.parse().expect("event id");
        eventually("the seeder stores the file message", || {
            let bytes = std::fs::read(&log).ok()?;
            contains(&bytes, &message_id.0).then_some(())
        })
        .await;
        let hash = data_encoding::BASE32_NOPAD
            .decode(attachment.to_uppercase().as_bytes())
            .expect("attachment id is base32");
        let bytes = std::fs::read(&log).expect("seeder log");
        assert!(!contains(&bytes, &hash), "the seeder's log holds no hash of the private file");

        finish(vec![alice, bob, carol, seeder]).await;
    })
    .await;
}

async fn one_event_channel_reaches_its_reader(member_only: bool) {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        if member_only {
            alice
                .engine
                .set_member_only(id, true)
                .await
                .expect("member-only control on");
            eventually("bob sees member-only control", || {
                bob.engine
                    .community_view(id)
                    .ok()
                    .filter(|v| v.member_only)
                    .map(|_| ())
            })
            .await;
        }

        // One event makes the channel and its access, so bob lists it as soon as it lands.
        let made = alice
            .engine
            .create_private_channel(id, "vault".into(), vec![], vec![bob.identity.root_id()])
            .await
            .expect("alice makes vault in one event");
        assert!(made.private);
        let vault: ChannelId = made.id.parse().expect("id");
        eventually("bob lists the private channel", || {
            lists_channel(&bob.engine, id, "vault").then_some(())
        })
        .await;
        alice
            .engine
            .send_message(id, vault, "one step".into())
            .await
            .expect("send");
        wait_seen(&bob, id, vault, "one step").await;

        // Carol is not on the list, and never lists the channel.
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
            .expect("marker");
        wait_seen(&carol, id, lounge, "marker").await;
        assert!(!lists_channel(&carol.engine, id, "vault"));

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_channel_made_in_one_event_reaches_its_reader() {
    one_event_channel_reaches_its_reader(false).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_channel_made_in_one_event_under_member_only_reaches_its_reader() {
    one_event_channel_reaches_its_reader(true).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_banned_member_cannot_read_a_private_channels_name_after_the_rotation() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let id = rustaceans(&alice, &bob).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;

        // The ban rotates the community secret. A channel made after it is sealed under the new secret.
        alice
            .engine
            .moderate(id, dissent_core::ModAction::Ban, bob.identity.root_id())
            .await
            .expect("alice bans bob");
        alice
            .engine
            .create_channel(id, "after-ban".into())
            .await
            .expect("marker");
        eventually("carol sees the marker", || {
            lists_channel(&carol.engine, id, "after-ban").then_some(())
        })
        .await;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let made = alice
            .engine
            .create_private_channel(
                id,
                "vault-after-ban".into(),
                vec![],
                vec![carol.identity.root_id()],
            )
            .await
            .expect("alice makes a private channel after the ban");
        let vault: ChannelId = made.id.parse().expect("id");
        alice
            .engine
            .set_channel_access(id, vault, true, vec![], vec![carol.identity.root_id()])
            .await
            .expect("alice changes its access after the ban");
        eventually("carol lists the private channel", || {
            lists_channel(&carol.engine, id, "vault-after-ban").then_some(())
        })
        .await;

        // A banned member receives no further events, so bob's log holds none of them. The wire bytes are
        // the same as alice's log, so the name is checked there: it is sealed, and bob has no key for it.
        let log = alice.dir.join("communities").join(id.to_string()).join("events.log");
        let bytes = std::fs::read(&log).expect("alice's log");
        assert!(contains(&bytes, &vault.0), "the channel's event is in the log");
        assert!(
            !contains(&bytes, b"vault-after-ban"),
            "the name of a private channel made after the ban is sealed"
        );
        assert!(!lists_channel(&bob.engine, id, "vault-after-ban"));

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

/// Counts the message announcements for one channel that a receiver holds.
fn message_announcements(rx: &mut tokio::sync::broadcast::Receiver<dissent_core::EngineEvent>, channel: ChannelId) -> usize {
    let mut n = 0;
    while let Ok(ev) = rx.try_recv() {
        if matches!(&ev, dissent_core::EngineEvent::Message { channel_id, .. } if *channel_id == channel.to_string()) {
            n += 1;
        }
    }
    n
}

/// A private message is announced once to a listed reader. A member who is not listed gets no announcement for it.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_private_message_is_announced_to_its_readers_only() {
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
        eventually("bob lists the private channel", || {
            lists_channel(&bob.engine, id, "vault").then_some(())
        })
        .await;
        let mut bob_events = bob.engine.subscribe();
        let mut carol_events = carol.engine.subscribe();

        alice
            .engine
            .send_message(id, vault, "for bob only".into())
            .await
            .expect("send into the private channel");
        wait_seen(&bob, id, vault, "for bob only").await;

        // Carol's marker shows that the channel traffic has reached her, so her silence is meaningful.
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
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(message_announcements(&mut bob_events, vault), 1, "bob is announced once");
        assert_eq!(message_announcements(&mut carol_events, vault), 0, "carol is not announced");

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}
