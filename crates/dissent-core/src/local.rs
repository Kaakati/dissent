//! Local state that never leaves this device: private notes and nicknames, pinned conversations
//! and messages, read markers, safety-number checks, and whether the recovery phrase was saved.
//! It is stored beside the other data files and never sent to a peer.

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, write_bin};
use crate::views::MessageItem;
use crate::world::World;
use dissent_events::{ChannelId, EventId};
use dissent_identity::{CommunityId, RootId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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
