//! Replies, reactions, edits and deletes in direct messages and group DMs.
//!
//! Each one is a `groups::Action`, signed by the sending device and sealed in the conversation, so it
//! reaches every device of every member over the same queues and the offline inbox as a message does.
//! A reply is a message too, linked to the message it answers. The other actions are stored in
//! `conversations/<id>.acts` beside the messages, and applied when the messages are read:
//!
//! - an edit or a delete counts only when its author wrote the target message;
//! - a reaction is kept per person and emoji, and the latest action from each person wins;
//! - a delete also clears the target's text from the message log, on every device that gets it.
//!
//! Actions apply in HLC order and are stored once per id, so a repeated or late action changes nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use dissent_events::{normalize_emoji, Hlc};
use dissent_identity::{DeviceId, RootId};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::engine::{clean_message, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::groups::{msg_id, Action, Body, ConvBook, ConvId, ConvKind, ConvMeta, ConvMsg, Delivery};
use crate::store::{append_record, read_records, rewrite_records, Paths};
use crate::views::{DmItem, ReactionView, ReplyPreview};
use crate::world::World;

/// How long a send waits for the queues to flush before it returns as queued. A peer that answers
/// within this time has the message when the call returns.
pub(crate) const QUICK_SEND: Duration = Duration::from_millis(1500);
/// Most characters of an original quoted in a reply preview.
const PREVIEW_CHARS: usize = 120;

/// A stored action. `id` is the action's message id, so it is the same on every device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConvAct {
    pub id: [u8; 32],
    pub from: RootId,
    pub hlc: Hlc,
    pub action: Action,
}

/// Reads the stored actions of every conversation in the book.
pub(crate) fn load_acts(paths: &Paths, book: &ConvBook) -> BTreeMap<ConvId, Vec<ConvAct>> {
    book.convs
        .keys()
        .map(|id| (*id, read_records::<ConvAct>(&paths.conv_acts(id)).unwrap_or_default()))
        .collect()
}

/// What the stored actions say about one conversation's messages.
#[derive(Default)]
pub(crate) struct Overlay {
    /// The latest valid edit of each message.
    edits: HashMap<[u8; 32], String>,
    /// Messages whose author deleted them.
    deleted: BTreeSet<[u8; 32]>,
    /// The message each reply answers, by the reply's id.
    replies: HashMap<[u8; 32], [u8; 32]>,
    /// Per message, each emoji in the order it first got a reaction, with the people who use it.
    reactions: HashMap<[u8; 32], Vec<(String, BTreeSet<RootId>)>>,
}

impl Overlay {
    /// Applies `acts` in HLC order to `msgs`. An edit or a delete counts only from the target's author.
    fn build(acts: &[ConvAct], msgs: &[ConvMsg]) -> Self {
        let authors: HashMap<[u8; 32], RootId> = msgs.iter().map(|m| (m.id, m.from)).collect();
        let mut ordered: Vec<&ConvAct> = acts.iter().collect();
        ordered.sort_by_key(|a| a.hlc);
        let mut ov = Overlay::default();
        for act in ordered {
            match &act.action {
                Action::Reply { target, .. } => {
                    ov.replies.insert(act.id, *target);
                }
                Action::Edit { target, text } if authors.get(target) == Some(&act.from) => {
                    ov.edits.insert(*target, text.clone());
                }
                Action::Delete { target } if authors.get(target) == Some(&act.from) => {
                    ov.deleted.insert(*target);
                }
                Action::React { target, emoji, on } if authors.contains_key(target) => {
                    ov.react(*target, emoji, act.from, *on);
                }
                _ => {}
            }
        }
        ov
    }

    fn react(&mut self, target: [u8; 32], emoji: &str, who: RootId, on: bool) {
        let list = self.reactions.entry(target).or_default();
        let at = match list.iter().position(|(e, _)| e == emoji) {
            Some(at) => at,
            None if on => {
                list.push((emoji.to_string(), BTreeSet::new()));
                list.len() - 1
            }
            None => return,
        };
        if on {
            list[at].1.insert(who);
        } else {
            list[at].1.remove(&who);
        }
    }
}

/// The id of a message as the UI shows it: lowercase base32.
pub(crate) fn id_text(id: &[u8; 32]) -> String {
    data_encoding::BASE32_NOPAD.encode(id).to_lowercase()
}

/// Reads a message id from the UI.
pub(crate) fn parse_message_id(s: &str) -> Result<[u8; 32]> {
    let invalid = || CoreError::Invalid(format!("not a message id: {s}"));
    let bytes = data_encoding::BASE32_NOPAD
        .decode(s.to_uppercase().as_bytes())
        .map_err(|_| invalid())?;
    <[u8; 32]>::try_from(bytes).map_err(|_| invalid())
}

fn target_of(action: &Action) -> Option<[u8; 32]> {
    match action {
        Action::Reply { target, .. }
        | Action::React { target, .. }
        | Action::Edit { target, .. }
        | Action::Delete { target } => Some(*target),
        Action::Text(_) | Action::Rename(_) | Action::Leave => None,
    }
}

/// True when a delete by `from` of message `id` has already been stored.
fn delete_arrived(acts: &[ConvAct], id: [u8; 32], from: RootId) -> bool {
    acts.iter()
        .any(|a| a.from == from && matches!(&a.action, Action::Delete { target } if *target == id))
}

/// Clears the stored text of message `target`, and rewrites the log so the text is gone from disk.
fn clear_text(w: &mut World, conv: ConvId, target: [u8; 32]) -> Result<()> {
    let Some(msgs) = w.conv_msgs.get_mut(&conv) else {
        return Ok(());
    };
    let Some(msg) = msgs.iter_mut().find(|m| m.id == target) else {
        return Ok(());
    };
    if msg.text.is_empty() {
        return Ok(());
    }
    msg.text.clear();
    rewrite_records(&w.paths.conv_log(&conv), msgs.as_slice())
}

impl Engine {
    /// The messages of `conv`, oldest first, with every stored action applied.
    pub(crate) fn conv_items(&self, conv: ConvId) -> Vec<DmItem> {
        let w = self.read();
        let msgs = w.conv_msgs.get(&conv).map_or(&[][..], Vec::as_slice);
        let acts = w.conv_acts.get(&conv).map_or(&[][..], Vec::as_slice);
        let ov = Overlay::build(acts, msgs);
        let by_id: HashMap<[u8; 32], &ConvMsg> = msgs.iter().map(|m| (m.id, m)).collect();
        let mut sorted: Vec<&ConvMsg> = msgs.iter().collect();
        sorted.sort_by_key(|m| m.hlc);
        sorted
            .into_iter()
            .map(|m| self.view(&w, &ov, &by_id, m))
            .collect()
    }

    /// The view of message `id` in `conv`, once it is stored.
    pub(crate) fn item_of(&self, conv: ConvId, id: [u8; 32]) -> Result<DmItem> {
        let text = id_text(&id);
        self.conv_items(conv)
            .into_iter()
            .find(|m| m.id == text)
            .ok_or_else(|| CoreError::NotFound(format!("message {text}")))
    }

    fn view(&self, w: &World, ov: &Overlay, by_id: &HashMap<[u8; 32], &ConvMsg>, m: &ConvMsg) -> DmItem {
        let deleted = ov.deleted.contains(&m.id);
        let delivery = if m.from == self.inner.me {
            w.conv_book.status.get(&m.id).map_or("sending", |s| s.as_str())
        } else {
            "delivered"
        };
        let text = ov.edits.get(&m.id).map_or(m.text.as_str(), String::as_str);
        DmItem {
            id: id_text(&m.id),
            from: m.from.to_string(),
            text: if deleted { String::new() } else { crate::attachments::text_of(text) },
            attachments: if deleted { Vec::new() } else { w.attachment_views(text) },
            embeds: if deleted { Vec::new() } else { crate::attachments::embeds_of(text) },
            ms: m.hlc.wall_ms,
            pending: delivery == "sending",
            delivery: delivery.to_string(),
            queued: delivery == "queued_inbox",
            reason: w.conv_book.failures.get(&m.id).cloned(),
            reply_to: ov.replies.get(&m.id).map(|target| reply_preview(w, ov, by_id, *target)),
            reactions: match (deleted, ov.reactions.get(&m.id)) {
                (false, Some(list)) => reactions_view(w, list),
                _ => Vec::new(),
            },
            edited: ov.edits.contains_key(&m.id),
            deleted,
        }
    }

    /// Sends `action` into `conv` and applies it here. Returns the action's message id.
    ///
    /// The action is stored and applied before it is sent. A message (text or reply) written before
    /// the conversation has a group waits locally until it does. The send waits at most `QUICK_SEND`
    /// for the queues to flush. A message still waiting then is marked queued, and the flush keeps
    /// going in the background.
    pub(crate) async fn post_action(&self, conv: ConvId, action: Action) -> Result<[u8; 32]> {
        let meta = self.active_meta(&conv)?;
        let me = self.inner.me;
        let body = Body {
            conv,
            from: me,
            hlc: self.stamp(),
            action,
        };
        let id = msg_id(&body)?;
        let is_message = matches!(body.action, Action::Text(_) | Action::Reply { .. });
        match &body.action {
            Action::Text(text) => self.store_text(conv, id, me, body.hlc, text.clone(), None)?,
            Action::Reply { target, text } => {
                self.store_text(conv, id, me, body.hlc, text.clone(), Some(*target))?
            }
            other => self.record_action(conv, id, me, body.hlc, other.clone())?,
        }
        if meta.kind == ConvKind::Dm {
            self.note_seen(meta.members.iter().copied().find(|m| *m != me));
        }
        if meta.ready {
            self.seal_and_queue(body)?;
        } else {
            let mut guard = self.write();
            guard.conv_book.unsent.push(body);
            Self::save_book(&guard)?;
        }
        self.flush_quickly().await;
        if is_message && !self.read().conv_book.status.contains_key(&id) {
            self.raise_status(id, Delivery::QueuedInbox);
        }
        self.notify_conv(&meta);
        Ok(id)
    }

    /// Flushes the queues in the background, and waits for that at most `QUICK_SEND`.
    async fn flush_quickly(&self) {
        let (done, flushed) = tokio::sync::oneshot::channel();
        let engine = Engine {
            inner: self.inner.clone(),
        };
        self.spawn_task(async move {
            engine.flush_conv_queues().await;
            let _ = done.send(());
        });
        let _ = tokio::time::timeout(QUICK_SEND, flushed).await;
    }

    /// Stores a text or a reply, and its reply link. A message whose delete arrived first is stored
    /// without its text.
    pub(crate) fn store_text(
        &self,
        conv: ConvId,
        id: [u8; 32],
        from: RootId,
        hlc: Hlc,
        text: String,
        reply_to: Option<[u8; 32]>,
    ) -> Result<()> {
        let gone = self
            .read()
            .conv_acts
            .get(&conv)
            .is_some_and(|acts| delete_arrived(acts, id, from));
        let stored = ConvMsg {
            id,
            conv,
            from,
            hlc,
            text: if gone { String::new() } else { text },
        };
        {
            let mut guard = self.write();
            append_record(&guard.paths.conv_log(&conv), &stored)?;
            guard.conv_msgs.entry(conv).or_default().push(stored);
        }
        if let Some(target) = reply_to {
            self.record_action(conv, id, from, hlc, Action::Reply { target, text: String::new() })?;
        }
        Ok(())
    }

    /// Stores an action and applies it. A delete clears its target's text. An action already stored
    /// is ignored. An edit or a delete of a stored message by anyone but its author is refused.
    pub(crate) fn record_action(&self, conv: ConvId, id: [u8; 32], from: RootId, hlc: Hlc, action: Action) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        if w.conv_acts.get(&conv).is_some_and(|acts| acts.iter().any(|a| a.id == id)) {
            return Ok(());
        }
        let author = target_of(&action).and_then(|t| {
            w.conv_msgs
                .get(&conv)?
                .iter()
                .find(|m| m.id == t)
                .map(|m| m.from)
        });
        let changes_text = matches!(action, Action::Edit { .. } | Action::Delete { .. });
        if changes_text && author.is_some_and(|a| a != from) {
            return Err(CoreError::Invalid("only the author may change this message".into()));
        }
        let act = ConvAct { id, from, hlc, action };
        append_record(&w.paths.conv_acts(&conv), &act)?;
        if let Action::Delete { target } = act.action {
            if author == Some(from) {
                clear_text(w, conv, target)?;
            }
        }
        w.conv_acts.entry(conv).or_default().push(act);
        Ok(())
    }

    /// Notes when this device first saw a direct-message peer.
    fn note_seen(&self, peer: Option<RootId>) {
        let Some(peer) = peer else {
            return;
        };
        let mut guard = self.write();
        guard.contact_times.note_seen(peer, now_ms());
        if let Err(e) = guard.contact_times.save(&guard.paths) {
            warn!(error = %e, "could not save first-seen times");
        }
    }

    /// A text or reply from a member. Stores it, mirrors and downloads its files, and acknowledges it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_text(
        &self,
        meta: &ConvMeta,
        id: [u8; 32],
        from: RootId,
        hlc: Hlc,
        text: String,
        reply_to: Option<[u8; 32]>,
        device: DeviceId,
        root: RootId,
    ) -> Result<()> {
        let conv = meta.id;
        let gone = self
            .read()
            .conv_acts
            .get(&conv)
            .is_some_and(|acts| delete_arrived(acts, id, from));
        if !gone {
            self.maybe_mirror(&text);
            self.auto_download(None, id_text(&id), &text);
        }
        self.store_text(conv, id, from, hlc, text, reply_to)?;
        if meta.kind == ConvKind::Dm {
            self.note_seen(Some(from));
        }
        self.notify_conv(meta);
        self.queue_receipt(conv, id, device, root)
    }

    /// Fails unless the conversation has its group, which reactions, edits and deletes need.
    fn require_ready(&self, conv: ConvId) -> Result<()> {
        if !self.active_meta(&conv)?.ready {
            return Err(CoreError::Invalid("this conversation is still being set up".into()));
        }
        Ok(())
    }

    /// The author of message `target`, when it is stored here and not deleted.
    fn live_message_author(&self, conv: ConvId, target: [u8; 32]) -> Result<RootId> {
        let w = self.read();
        let msgs = w.conv_msgs.get(&conv).map_or(&[][..], Vec::as_slice);
        let acts = w.conv_acts.get(&conv).map_or(&[][..], Vec::as_slice);
        let msg = msgs
            .iter()
            .find(|m| m.id == target)
            .ok_or_else(|| CoreError::NotFound(format!("message {}", id_text(&target))))?;
        if Overlay::build(acts, msgs).deleted.contains(&target) {
            return Err(CoreError::Invalid("this message was deleted".into()));
        }
        Ok(msg.from)
    }

    /// Checks that `target` is our own message and not deleted yet.
    fn own_live_message(&self, conv: ConvId, target: [u8; 32]) -> Result<()> {
        if self.live_message_author(conv, target)? != self.inner.me {
            return Err(CoreError::Invalid("only the author may edit or delete this message".into()));
        }
        Ok(())
    }

    /// Replies to message `target` in `conv`. The reply is a message of its own.
    pub async fn conv_reply(&self, conv: ConvId, target: &str, text: String) -> Result<DmItem> {
        let target = parse_message_id(target)?;
        self.live_message_author(conv, target)?;
        let text = clean_message(&text)?;
        let id = self.post_action(conv, Action::Reply { target, text }).await?;
        self.item_of(conv, id)
    }

    /// Adds (`on`) or removes our reaction `emoji` on message `target`.
    pub async fn conv_react(&self, conv: ConvId, target: &str, emoji: String, on: bool) -> Result<()> {
        let target = parse_message_id(target)?;
        let emoji = normalize_emoji(&emoji).map_err(|e| CoreError::Invalid(format!("emoji: {e:?}")))?;
        self.live_message_author(conv, target)?;
        self.require_ready(conv)?;
        self.post_action(conv, Action::React { target, emoji, on }).await?;
        Ok(())
    }

    /// Replaces the text of our own message `target`.
    pub async fn conv_edit(&self, conv: ConvId, target: &str, text: String) -> Result<DmItem> {
        let target = parse_message_id(target)?;
        let text = clean_message(&text)?;
        self.own_live_message(conv, target)?;
        self.require_ready(conv)?;
        self.post_action(conv, Action::Edit { target, text }).await?;
        self.item_of(conv, target)
    }

    /// Deletes our own message `target` everywhere it is held.
    pub async fn conv_delete(&self, conv: ConvId, target: &str) -> Result<()> {
        let target = parse_message_id(target)?;
        self.own_live_message(conv, target)?;
        self.require_ready(conv)?;
        self.post_action(conv, Action::Delete { target }).await?;
        Ok(())
    }

    /// Replies to `target` in the 1:1 conversation with `peer`.
    pub async fn dm_reply(&self, peer: RootId, target: &str, text: String) -> Result<DmItem> {
        let conv = self.ensure_dm(peer)?;
        self.conv_reply(conv, target, text).await
    }

    /// Adds or removes our reaction on `target` in the 1:1 conversation with `peer`.
    pub async fn dm_react(&self, peer: RootId, target: &str, emoji: String, on: bool) -> Result<()> {
        let conv = self.ensure_dm(peer)?;
        self.conv_react(conv, target, emoji, on).await
    }

    /// Edits our own message `target` in the 1:1 conversation with `peer`.
    pub async fn dm_edit(&self, peer: RootId, target: &str, text: String) -> Result<DmItem> {
        let conv = self.ensure_dm(peer)?;
        self.conv_edit(conv, target, text).await
    }

    /// Deletes our own message `target` in the 1:1 conversation with `peer`.
    pub async fn dm_delete(&self, peer: RootId, target: &str) -> Result<()> {
        let conv = self.ensure_dm(peer)?;
        self.conv_delete(conv, target).await
    }
}

fn reply_preview(w: &World, ov: &Overlay, by_id: &HashMap<[u8; 32], &ConvMsg>, target: [u8; 32]) -> ReplyPreview {
    let id = id_text(&target);
    let Some(orig) = by_id.get(&target) else {
        return ReplyPreview {
            id,
            author: String::new(),
            author_name: String::new(),
            text: None,
            locked: false,
        };
    };
    let text: Option<String> = if ov.deleted.contains(&target) {
        None
    } else {
        let plain = ov.edits.get(&target).map_or(orig.text.as_str(), String::as_str);
        Some(crate::attachments::text_of(plain).chars().take(PREVIEW_CHARS).collect())
    };
    ReplyPreview {
        id,
        author: orig.from.to_string(),
        author_name: w.display_name(&orig.from),
        text,
        locked: false,
    }
}

fn reactions_view(w: &World, list: &[(String, BTreeSet<RootId>)]) -> Vec<ReactionView> {
    list.iter()
        .filter(|(_, who)| !who.is_empty())
        .map(|(emoji, who)| ReactionView {
            emoji: emoji.clone(),
            count: who.len(),
            me: who.contains(&w.me),
            names: who.iter().take(3).map(|r| w.display_name(r)).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(n: u8) -> RootId {
        RootId([n; 32])
    }

    fn hlc(ms: u64) -> Hlc {
        Hlc {
            wall_ms: ms,
            counter: 0,
        }
    }

    fn msg(id: u8, from: RootId, text: &str) -> ConvMsg {
        ConvMsg {
            id: [id; 32],
            conv: ConvId([0; 32]),
            from,
            hlc: hlc(u64::from(id)),
            text: text.into(),
        }
    }

    fn act(id: u8, from: RootId, ms: u64, action: Action) -> ConvAct {
        ConvAct {
            id: [id; 32],
            from,
            hlc: hlc(ms),
            action,
        }
    }

    /// The (emoji, count) pairs a message shows.
    fn shown_reactions(ov: &Overlay, target: [u8; 32]) -> Vec<(String, usize)> {
        ov.reactions
            .get(&target)
            .into_iter()
            .flatten()
            .filter(|(_, who)| !who.is_empty())
            .map(|(e, who)| (e.clone(), who.len()))
            .collect()
    }

    #[test]
    fn existing_action_variants_keep_their_encoding() {
        // Stored and signed bodies must still read: new variants are appended, never inserted.
        assert_eq!(postcard::to_allocvec(&Action::Text("a".into())).unwrap(), vec![0, 1, b'a']);
        assert_eq!(postcard::to_allocvec(&Action::Leave).unwrap(), vec![2]);
        let delete = Action::Delete { target: [7; 32] };
        let bytes = postcard::to_allocvec(&delete).unwrap();
        assert_eq!(postcard::from_bytes::<Action>(&bytes).unwrap(), delete);
    }

    #[test]
    fn a_message_log_written_before_actions_existed_still_loads() {
        let dir = std::env::temp_dir().join(format!("dissent-conv-log-{}", now_ms()));
        let paths = Paths::new(dir.clone());
        let conv = ConvId([9; 32]);
        let old = msg(1, root(1), "from before");
        append_record(&paths.conv_log(&conv), &old).unwrap();
        let mut book = ConvBook::default();
        book.convs.insert(
            conv,
            ConvMeta {
                id: conv,
                kind: ConvKind::Dm,
                creator: root(1),
                name: String::new(),
                name_hlc: Hlc::default(),
                members: vec![root(1), root(2)],
                ready: true,
                left: false,
                created_ms: 0,
            },
        );
        let msgs = crate::groups::load_messages(&paths, &book);
        assert_eq!(msgs[&conv], vec![old]);
        assert!(load_acts(&paths, &book)[&conv].is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_author_can_edit_or_delete() {
        let (alice, bob) = (root(1), root(2));
        let msgs = [msg(5, alice, "hello")];
        let forged = [
            act(10, bob, 10, Action::Edit { target: [5; 32], text: "hijacked".into() }),
            act(11, bob, 11, Action::Delete { target: [5; 32] }),
        ];
        let ov = Overlay::build(&forged, &msgs);
        assert!(ov.edits.is_empty());
        assert!(ov.deleted.is_empty());
        let fixed = [act(12, alice, 12, Action::Edit { target: [5; 32], text: "fixed".into() })];
        assert_eq!(Overlay::build(&fixed, &msgs).edits[&[5; 32]], "fixed");
    }

    #[test]
    fn reactions_toggle_and_the_latest_action_wins_in_any_order() {
        let (alice, bob) = (root(1), root(2));
        let msgs = [msg(5, alice, "hi")];
        // The unreact has the later clock but arrives first. Bob's later action still wins.
        let acts = [
            act(21, bob, 30, Action::React { target: [5; 32], emoji: "👍".into(), on: false }),
            act(20, bob, 20, Action::React { target: [5; 32], emoji: "👍".into(), on: true }),
        ];
        assert!(shown_reactions(&Overlay::build(&acts, &msgs), [5; 32]).is_empty());
        let acts = [
            act(22, alice, 5, Action::React { target: [5; 32], emoji: "🔥".into(), on: true }),
            act(23, bob, 6, Action::React { target: [5; 32], emoji: "🔥".into(), on: true }),
        ];
        assert_eq!(shown_reactions(&Overlay::build(&acts, &msgs), [5; 32]), vec![("🔥".to_string(), 2)]);
    }

    #[test]
    fn a_reply_links_to_its_target_and_a_delete_marks_the_target() {
        let (alice, bob) = (root(1), root(2));
        let msgs = [msg(5, alice, "original"), msg(6, bob, "answer")];
        let acts = [
            act(6, bob, 6, Action::Reply { target: [5; 32], text: String::new() }),
            act(7, alice, 7, Action::Delete { target: [5; 32] }),
        ];
        let ov = Overlay::build(&acts, &msgs);
        assert_eq!(ov.replies[&[6; 32]], [5; 32]);
        assert!(ov.deleted.contains(&[5; 32]));
    }
}
