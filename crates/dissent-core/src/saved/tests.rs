//! Saved messages and reminders on loopback. Both survive a restart, and a reminder fires once.

use crate::membership::tests::Node;
use crate::saved::MessageRef;
use crate::testutil::bounded;

fn message(id: &str, ms: u64) -> MessageRef {
    MessageRef {
        community: Some("c1".into()),
        channel: Some("ch1".into()),
        peer: None,
        message_id: id.into(),
        text: format!("text {id}"),
        author_name: "Maya".into(),
        ms,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bookmark_is_saved_once_listed_and_removed() {
    bounded(async {
        let node = Node::named("saved-bookmarks").await;
        node.engine.add_bookmark(message("m1", 100)).expect("save");
        node.engine.add_bookmark(message("m1", 100)).expect("save again");
        node.engine.add_bookmark(message("m2", 200)).expect("save second");
        let listed = node.engine.list_bookmarks();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert_eq!(listed[0].message.message_id, "m2", "most recently saved first");
        node.engine.remove_bookmark(&message("m1", 100).key()).expect("remove");
        assert_eq!(node.engine.list_bookmarks().len(), 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bookmarks_survive_a_restart() {
    bounded(async {
        let node = Node::named("saved-restart").await;
        node.engine.add_bookmark(message("keep", 10)).expect("save");
        let (identity, dir, port) = node.stop().await;
        let again = Node::boot(identity, dir, port).await;
        assert_eq!(again.engine.list_bookmarks().len(), 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reminder_fires_once_when_due_and_survives_a_restart() {
    bounded(async {
        let node = Node::named("saved-reminder").await;
        let later = crate::engine::now_ms() + 3_600_000;
        let reminder = node.engine.add_reminder(message("r1", 5), later).expect("remind");
        let (identity, dir, port) = node.stop().await;

        let again = Node::boot(identity, dir, port).await;
        assert_eq!(again.engine.list_reminders(), vec![reminder.clone()], "still pending after a restart");
        assert!(again.engine.take_due_reminders(later - 1).expect("take").is_empty(), "not due yet");
        let fired = again.engine.take_due_reminders(later).expect("take");
        assert_eq!(fired, vec![reminder.clone()]);
        assert!(again.engine.take_due_reminders(later + 1).expect("take").is_empty(), "fires once");
        assert!(again.engine.list_reminders().is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_reminder_never_fires() {
    bounded(async {
        let node = Node::named("saved-cancel").await;
        let at = crate::engine::now_ms() + 60_000;
        let reminder = node.engine.add_reminder(message("r2", 5), at).expect("remind");
        node.engine.remove_reminder(&reminder.id).expect("cancel");
        assert!(node.engine.take_due_reminders(at + 1).expect("take").is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_unread_puts_the_read_marker_just_before_the_message() {
    bounded(async {
        let node = Node::named("saved-unread").await;
        node.engine.mark_read("c1|ch1".into()).expect("read");
        node.engine.mark_unread("c1|ch1".into(), 5_000).expect("unread");
        assert_eq!(node.engine.read_marker("c1|ch1"), Some(4_999));
    })
    .await;
}
