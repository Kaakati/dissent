//! Local state that never leaves this device: private notes and nicknames, pinned conversations
//! and messages, read markers, safety-number checks, and whether the recovery phrase was saved.
//! It is stored beside the other data files and never sent to a peer.

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, write_bin};
use crate::views::{ConversationSummary, MessageItem};
use crate::world::World;
use dissent_events::{ChannelId, EventId};
use dissent_identity::{CommunityId, RootId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Longest note or nickname, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;

/// The features this engine implements. The UI shows a surface only when its name is listed here.
pub const CAPABILITIES: &[&str] = &[
    "calls",
    "mute",
    "guardians",
    "devices",
    "key_change",
    "safety",
    "notes",
    "pins",
    "onboarding",
    "close_dm",
    "ignore",
    "status",
    "avatars",
    "member_only",
    "contacts",
    "groups",
    "transfers",
    "embeds",
    "multi_source",
    "mentions",
    "server_prefs",
    "private_channels",
    "forward_calls",
    "channel_order",
    "channel_categories",
    "dm_mentions",
];

/// Which text a user note stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// A private note about a person.
    Note,
    /// A name only this device shows for a person.
    Nickname,
}

#[derive(Default, Serialize, Deserialize)]
struct LocalState {
    notes: BTreeMap<RootId, String>,
    nicknames: BTreeMap<RootId, String>,
    pinned_conversations: BTreeSet<String>,
    /// Keyed by `community/channel`.
    pinned_messages: BTreeMap<String, BTreeSet<EventId>>,
    /// Conversation key to the time it was last marked read.
    read_ms: BTreeMap<String, u64>,
    phrase_saved: bool,
}

/// The per-user fields that `user_profile` adds.
pub(crate) struct UserFields {
    pub key_changed_ms: Option<u64>,
    pub verified_ms: Option<u64>,
    pub note: Option<String>,
    pub nickname: Option<String>,
    pub ignored: bool,
}

fn load(w: &World) -> LocalState {
    match read_bin(&w.paths.local_state()) {
        Ok(state) => state.unwrap_or_default(),
        Err(_) => LocalState::default(),
    }
}

/// When a conversation was last marked read, in milliseconds. `key` is `dm:<peer>` or `group:<id>`.
pub(crate) fn read_marker_in(w: &World, key: &str) -> Option<u64> {
    load(w).read_ms.get(key).copied()
}

fn verified_path(w: &World) -> PathBuf {
    w.paths.identity_changes().with_file_name("safety_verified.bin")
}

fn verified(w: &World) -> BTreeMap<RootId, u64> {
    read_bin(&verified_path(w)).ok().flatten().unwrap_or_default()
}

/// True until the recovery phrase has been shown, on a device with no communities or contacts.
/// A device that already holds data is treated as set up.
pub(crate) fn first_run(w: &World) -> bool {
    !load(w).phrase_saved && w.communities.is_empty() && w.contacts.is_empty()
}

/// The fields `user_profile` adds for `user`. A key change counts only while it is newer than
/// the last safety-number check.
pub(crate) fn user_fields(w: &World, user: &RootId) -> UserFields {
    let state = load(w);
    let verified_ms = verified(w).get(user).copied();
    let changed_ms = crate::identity_watch::last_change(&w.identity_changes, user);
    let key_changed_ms = match (changed_ms, verified_ms) {
        (Some(changed), Some(checked)) if changed <= checked => None,
        (changed, _) => changed,
    };
    UserFields {
        key_changed_ms,
        verified_ms,
        note: state.notes.get(user).cloned(),
        nickname: state.nicknames.get(user).cloned(),
        ignored: is_ignored(w, user),
    }
}

fn channel_key(community: &CommunityId, channel: &ChannelId) -> String {
    format!("{community}/{channel}")
}

fn check_text(text: &str) -> Result<()> {
    if text.chars().count() > MAX_NOTE_CHARS {
        return Err(CoreError::Invalid(format!(
            "text is longer than {MAX_NOTE_CHARS} characters"
        )));
    }
    Ok(())
}

impl Engine {
    /// The features this engine implements. See [`CAPABILITIES`].
    pub fn capabilities(&self) -> Vec<&'static str> {
        CAPABILITIES.to_vec()
    }

    /// Runs one read-modify-write of the local state under the world lock.
    fn update_local<T>(&self, f: impl FnOnce(&mut LocalState) -> T) -> Result<T> {
        let path = self.read().paths.local_state();
        let _serial = self.write();
        let mut state: LocalState = read_bin(&path)?.unwrap_or_default();
        let out = f(&mut state);
        write_bin(&path, &state)?;
        Ok(out)
    }

    /// Saves a private note or nickname for `user`. Empty text removes it.
    pub fn set_note(&self, user: RootId, kind: NoteKind, text: String) -> Result<()> {
        let text = text.trim().to_string();
        check_text(&text)?;
        self.update_local(|s| {
            let map = match kind {
                NoteKind::Note => &mut s.notes,
                NoteKind::Nickname => &mut s.nicknames,
            };
            if text.is_empty() {
                map.remove(&user);
            } else {
                map.insert(user, text);
            }
        })
    }

    /// Pins or unpins a conversation in the sidebar. `key` is the UI's conversation key.
    pub fn pin_conversation(&self, key: String, pinned: bool) -> Result<()> {
        self.update_local(|s| {
            if pinned {
                s.pinned_conversations.insert(key);
            } else {
                s.pinned_conversations.remove(&key);
            }
        })
    }

    /// The conversation keys pinned in the sidebar.
    pub fn pinned_conversations(&self) -> Vec<String> {
        load(&self.read()).pinned_conversations.into_iter().collect()
    }

    /// Pins or unpins one message in a channel. Pins stay on this device.
    pub fn pin_message(
        &self,
        community: CommunityId,
        channel: ChannelId,
        message: EventId,
        pinned: bool,
    ) -> Result<()> {
        self.update_local(|s| {
            let pins = s.pinned_messages.entry(channel_key(&community, &channel)).or_default();
            if pinned {
                pins.insert(message);
            } else {
                pins.remove(&message);
            }
        })
    }

    /// The pinned messages of a channel, oldest first.
    pub fn pinned_messages(&self, community: CommunityId, channel: ChannelId) -> Vec<MessageItem> {
        let pinned = load(&self.read())
            .pinned_messages
            .get(&channel_key(&community, &channel))
            .cloned()
            .unwrap_or_default();
        let ids: BTreeSet<String> = pinned.iter().map(ToString::to_string).collect();
        self.channel_messages(community, channel, 100_000)
            .into_iter()
            .filter(|m| ids.contains(&m.id))
            .collect()
    }

    /// Marks a conversation read now. Its unread marker clears.
    pub fn mark_read(&self, key: String) -> Result<()> {
        let now = now_ms();
        self.update_local(|s| {
            s.read_ms.insert(key, now);
        })
    }

    /// Marks a conversation unread from `before_ms` on: messages at or after that time count as unread.
    pub fn mark_unread(&self, key: String, before_ms: u64) -> Result<()> {
        self.update_local(|s| {
            s.read_ms.insert(key, before_ms.saturating_sub(1));
        })
    }

    /// When a conversation was last marked read, in milliseconds.
    pub fn read_marker(&self, key: &str) -> Option<u64> {
        load(&self.read()).read_ms.get(key).copied()
    }

    /// Ends first run. The UI calls it after the user confirms they wrote the recovery phrase down.
    /// Exporting the phrase does not end it.
    pub fn complete_first_run(&self) -> Result<()> {
        self.update_local(|s| s.phrase_saved = true)
    }

    /// Records that the safety number for `user` was compared now. This clears any key change
    /// that happened before now.
    pub fn mark_safety_verified(&self, user: RootId) -> Result<()> {
        let (path, now) = (verified_path(&self.read()), now_ms());
        let _serial = self.write();
        let mut map: BTreeMap<RootId, u64> = read_bin(&path)?.unwrap_or_default();
        map.insert(user, now);
        write_bin(&path, &map)
    }
}

impl Engine {
    /// Every conversation key with the time it was last marked read, in milliseconds.
    pub fn read_markers(&self) -> BTreeMap<String, u64> {
        load(&self.read()).read_ms
    }
}

/// Hidden conversations and ignored people. A separate file, so `local.bin` keeps its layout.
#[derive(Default, Serialize, Deserialize)]
struct ViewState {
    /// Conversation key to the time it was closed, in milliseconds.
    closed: BTreeMap<String, u64>,
    ignored: BTreeSet<RootId>,
}

fn view_path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("local_view.bin")
}

fn load_view(w: &World) -> ViewState {
    read_bin(&view_path(w)).ok().flatten().unwrap_or_default()
}

/// The key a conversation is closed under, the same key the UI uses: `dm:<peer>` or `group:<id>`.
pub(crate) fn close_key(conv: &ConversationSummary) -> String {
    if conv.kind == "dm" {
        format!("dm:{}", conv.peer)
    } else {
        format!("group:{}", conv.id)
    }
}

/// How far a message's sender timestamp may trail the close and still count. Ordinary clock skew fits in it.
const ARRIVAL_SKEW_MS: u64 = 10 * 60 * 1000;

/// Whether a message sent at `sent_ms` should be recorded as an arrival, given when its conversation was closed. An
/// open conversation records every arrival. A closed one records only a message sent no earlier than the close minus
/// `ARRIVAL_SKEW_MS`: a message queued while this device was offline still counts, and old history synced later does not.
pub(crate) fn records_arrival(closed_ms: Option<u64>, sent_ms: u64) -> bool {
    closed_ms.is_none_or(|closed_ms| sent_ms.saturating_add(ARRIVAL_SKEW_MS) >= closed_ms)
}

/// Whether a closed conversation stays out of the list: its newest recorded arrival is not after the close.
pub(crate) fn hidden_by_close(closed: &BTreeMap<String, u64>, arrived: &BTreeMap<String, u64>, conv: &ConversationSummary) -> bool {
    let key = close_key(conv);
    closed
        .get(&key)
        .is_some_and(|closed_ms| arrived.get(&key).is_none_or(|arrived_ms| arrived_ms <= closed_ms))
}

/// When a message last arrived on this device in each conversation that it counted for, on this device's clock. Kept in
/// its own file, so `local_view.bin` keeps its layout.
fn arrivals_path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("local_arrivals.bin")
}

/// The arrivals on disk. A file that does not decode is copied to `<name>.bad` (the first such copy is kept), with a
/// warning, and the map starts empty, so one bad file cannot stop every later arrival from being recorded. The next
/// write replaces it. Other read errors still fail.
fn load_arrivals(path: &Path) -> Result<BTreeMap<String, u64>> {
    match read_bin(path) {
        Ok(found) => Ok(found.unwrap_or_default()),
        Err(CoreError::Encoding(reason)) => {
            let bad = path.with_extension("bin.bad");
            tracing::warn!(%reason, path = %path.display(), "arrivals file does not decode; starting empty");
            if !bad.exists() {
                fs::copy(path, &bad)?;
            }
            Ok(BTreeMap::new())
        }
        Err(e) => Err(e),
    }
}

impl Engine {
    /// Records that a message reached this device in `key` (`dm:<peer>` or `group:<id>`), sent at `sent_ms` by its
    /// sender, when `records_arrival` says it counts.
    pub(crate) fn note_arrival(&self, key: String, sent_ms: u64) -> Result<()> {
        let closed = self.closed_conversations();
        if !records_arrival(closed.get(&key).copied(), sent_ms) {
            return Ok(());
        }
        let path = arrivals_path(&self.read());
        let _serial = self.write();
        let mut arrivals = load_arrivals(&path)?;
        arrivals.insert(key, now_ms());
        write_bin(&path, &arrivals)
    }

    /// The local arrival time of the newest counted message in each conversation.
    pub(crate) fn arrivals(&self) -> BTreeMap<String, u64> {
        read_bin(&arrivals_path(&self.read())).ok().flatten().unwrap_or_default()
    }
}

/// Whether `user` is ignored on this device.
pub(crate) fn is_ignored(w: &World, user: &RootId) -> bool {
    load_view(w).ignored.contains(user)
}

impl Engine {
    /// Hides a conversation from the sidebar. It comes back when a new message arrives.
    pub fn close_conversation(&self, key: String) -> Result<()> {
        self.update_view(|s| {
            s.closed.insert(key, now_ms());
        })
    }

    /// Shows a closed conversation again.
    pub fn reopen_conversation(&self, key: String) -> Result<()> {
        self.update_view(|s| {
            s.closed.remove(&key);
        })
    }

    /// Closed conversations, each with the time it was closed, in milliseconds. The UI compares
    /// that with the conversation's last message to bring it back.
    pub fn closed_conversations(&self) -> BTreeMap<String, u64> {
        load_view(&self.read()).closed
    }

    /// Ignores a person. Their messages still arrive, but the UI collapses them and never notifies.
    /// Only this device knows.
    pub fn ignore_user(&self, user: RootId) -> Result<()> {
        self.update_view(|s| {
            s.ignored.insert(user);
        })
    }

    /// Stops ignoring a person.
    pub fn unignore_user(&self, user: RootId) -> Result<()> {
        self.update_view(|s| {
            s.ignored.remove(&user);
        })
    }

    /// Everyone this device ignores.
    pub fn ignored_users(&self) -> Vec<RootId> {
        load_view(&self.read()).ignored.into_iter().collect()
    }

    fn update_view<T>(&self, f: impl FnOnce(&mut ViewState) -> T) -> Result<T> {
        let path = view_path(&self.read());
        let _serial = self.write();
        let mut state: ViewState = read_bin(&path)?.unwrap_or_default();
        let out = f(&mut state);
        write_bin(&path, &state)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::ConvMember;

    #[test]
    fn an_undecodable_arrivals_file_is_set_aside_and_started_empty() {
        let dir = std::env::temp_dir().join(format!("dissent-arrivals-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("local_arrivals.bin");
        let garbage: &[u8] = b"\xff\xff not postcard";
        fs::write(&path, garbage).unwrap();
        assert!(read_bin::<BTreeMap<String, u64>>(&path).is_err(), "the fixture must not decode");

        assert!(load_arrivals(&path).unwrap().is_empty(), "a bad file reads as empty");
        assert_eq!(fs::read(dir.join("local_arrivals.bin.bad")).unwrap(), garbage, "the bad copy keeps the bytes");

        // Recording an arrival writes the file again, and the next load reads it back.
        let mut arrivals = load_arrivals(&path).unwrap();
        arrivals.insert("dm:peer".into(), 7);
        write_bin(&path, &arrivals).unwrap();
        assert_eq!(load_arrivals(&path).unwrap().get("dm:peer"), Some(&7));

        // A second bad file does not replace the first bad copy.
        fs::write(&path, b"\xfe\xfe").unwrap();
        load_arrivals(&path).unwrap();
        assert_eq!(fs::read(dir.join("local_arrivals.bin.bad")).unwrap(), garbage);
        fs::remove_dir_all(&dir).ok();
    }

    const DAY: u64 = 24 * 60 * 60 * 1000;
    const MINUTE: u64 = 60 * 1000;
    /// When the conversation was closed, on this device's clock.
    const CLOSED: u64 = 1_800_000_000_000;

    fn dm(peer: &str) -> ConversationSummary {
        ConversationSummary {
            id: format!("conv-{peer}"),
            kind: "dm".into(),
            peer: peer.into(),
            members: Vec::<ConvMember>::new(),
            name: String::new(),
            display_name: peer.into(),
            last_text: String::new(),
            last_ms: 0,
            status: None,
            avatar: None,
            live_call: None,
            contact_since_ms: None,
            first_seen_ms: None,
            unread: 0,
            mentions: 0,
        }
    }

    /// Whether bob's conversation, closed at `CLOSED`, is hidden once a message is recorded at `arrived_ms`.
    fn hidden_after(arrived_ms: Option<u64>) -> bool {
        let closed = BTreeMap::from([("dm:bob".to_string(), CLOSED)]);
        let arrived: BTreeMap<String, u64> = arrived_ms.map(|a| ("dm:bob".to_string(), a)).into_iter().collect();
        hidden_by_close(&closed, &arrived, &dm("bob"))
    }

    #[test]
    fn a_closed_conversation_with_no_arrival_since_the_close_stays_hidden() {
        assert!(hidden_after(None));
        let closed = BTreeMap::from([("dm:bob".to_string(), CLOSED)]);
        assert!(!hidden_by_close(&closed, &BTreeMap::new(), &dm("carol")), "other conversations are never hidden");
        assert!(!hidden_by_close(&BTreeMap::new(), &BTreeMap::new(), &dm("bob")), "an open conversation is never hidden");
    }

    #[test]
    fn an_arrival_after_the_close_brings_the_conversation_back() {
        assert!(!hidden_after(Some(CLOSED + MINUTE)));
        assert!(hidden_after(Some(CLOSED)), "an arrival in the close's own millisecond is not after it");
    }

    #[test]
    fn a_late_delivery_of_a_message_sent_after_the_close_reopens_it() {
        // A friend sends after the close, but this device is offline. The outbox delivers it two hours later.
        let sent = CLOSED + 30 * MINUTE;
        assert!(records_arrival(Some(CLOSED), sent), "sent after the close, so recorded");
        assert!(!hidden_after(Some(CLOSED + 120 * MINUTE)), "it arrived after the close, so the row comes back");
    }

    #[test]
    fn a_message_sent_before_the_close_and_delivered_after_it_reopens_it() {
        let sent = CLOSED - 3 * MINUTE;
        assert!(records_arrival(Some(CLOSED), sent));
        assert!(!hidden_after(Some(CLOSED + 120 * MINUTE)));
    }

    #[test]
    fn old_history_synced_after_the_close_is_not_recorded_and_stays_hidden() {
        // Sent two days before the close, delivered by sync an hour after it.
        assert!(!records_arrival(Some(CLOSED), CLOSED - 2 * DAY));
        assert!(hidden_after(None));
    }

    #[test]
    fn the_ten_minute_limit_is_inclusive() {
        assert!(records_arrival(Some(CLOSED), CLOSED - 10 * MINUTE));
        assert!(!records_arrival(Some(CLOSED), CLOSED - 10 * MINUTE - 1));
    }

    #[test]
    fn a_sender_clock_behind_the_close_does_not_keep_the_conversation_hidden() {
        assert!(records_arrival(Some(CLOSED), CLOSED - 4 * MINUTE));
    }

    #[test]
    fn an_open_conversation_records_every_arrival() {
        assert!(records_arrival(None, CLOSED - 2 * DAY));
    }
}
