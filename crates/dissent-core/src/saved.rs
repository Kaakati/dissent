//! Saved messages and reminders. Local only: nothing here is sent to anyone. Each list lives in its own
//! file, so no existing stored struct changes. A reminder survives a restart, and fires once when its
//! time comes, as a `ReminderDue` event that the desktop turns into a notification.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::engine::{now_ms, Engine, Inner};
use crate::error::Result;
use crate::store::{read_bin, write_bin};
use crate::views::EngineEvent;
use std::sync::{Arc, Weak};

/// How often the reminder clock checks for due reminders.
const CLOCK_EVERY: Duration = Duration::from_secs(15);

/// Where a message is: a channel of a community, or a direct conversation with a person. Ids are the
/// strings the views use, so the UI can jump to the message from them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRef {
    /// The community id, for a channel message.
    pub community: Option<String>,
    /// The channel id, for a channel message.
    pub channel: Option<String>,
    /// The other person, for a direct message.
    pub peer: Option<String>,
    pub message_id: String,
    pub text: String,
    pub author_name: String,
    /// When the message was sent, in ms since the Unix epoch.
    pub ms: u64,
}

impl MessageRef {
    /// Identifies the message on this device, for saving it once.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.community.as_deref().unwrap_or(""),
            self.channel.as_deref().unwrap_or(""),
            self.peer.as_deref().unwrap_or(""),
            self.message_id
        )
    }
}

/// A saved message, with the time it was saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub key: String,
    pub message: MessageRef,
    pub saved_ms: u64,
}

/// A reminder about a message, due at `at_ms`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub at_ms: u64,
    pub message: MessageRef,
}

fn bookmarks_path(engine: &Engine) -> PathBuf {
    engine.read().paths.local_state().with_file_name("bookmarks.bin")
}

fn reminders_path(engine: &Engine) -> PathBuf {
    engine.read().paths.local_state().with_file_name("reminders.bin")
}

impl Engine {
    /// Saves a message. Saving one twice keeps the first save.
    pub fn add_bookmark(&self, message: MessageRef) -> Result<()> {
        let path = bookmarks_path(self);
        let _serial = self.write();
        let mut all: Vec<Bookmark> = read_bin(&path)?.unwrap_or_default();
        let key = message.key();
        if !all.iter().any(|b| b.key == key) {
            all.push(Bookmark {
                key,
                message,
                saved_ms: now_ms(),
            });
        }
        write_bin(&path, &all)
    }

    /// Removes a saved message by its key.
    pub fn remove_bookmark(&self, key: &str) -> Result<()> {
        let path = bookmarks_path(self);
        let _serial = self.write();
        let mut all: Vec<Bookmark> = read_bin(&path)?.unwrap_or_default();
        all.retain(|b| b.key != key);
        write_bin(&path, &all)
    }

    /// Saved messages, most recently saved first.
    pub fn list_bookmarks(&self) -> Vec<Bookmark> {
        let path = bookmarks_path(self);
        let mut all: Vec<Bookmark> = read_bin(&path).ok().flatten().unwrap_or_default();
        all.sort_by_key(|b| std::cmp::Reverse(b.saved_ms));
        all
    }

    /// Asks to be reminded about a message at `at_ms`. Returns the reminder, with its id.
    pub fn add_reminder(&self, message: MessageRef, at_ms: u64) -> Result<Reminder> {
        let path = reminders_path(self);
        let _serial = self.write();
        let mut all: Vec<Reminder> = read_bin(&path)?.unwrap_or_default();
        let reminder = Reminder {
            id: format!("{:016x}", rand::random::<u64>()),
            at_ms,
            message,
        };
        all.push(reminder.clone());
        write_bin(&path, &all)?;
        Ok(reminder)
    }

    /// Reminders not yet fired, soonest first.
    pub fn list_reminders(&self) -> Vec<Reminder> {
        let path = reminders_path(self);
        let mut all: Vec<Reminder> = read_bin(&path).ok().flatten().unwrap_or_default();
        all.sort_by_key(|r| r.at_ms);
        all
    }

    /// Cancels a reminder by its id.
    pub fn remove_reminder(&self, id: &str) -> Result<()> {
        let path = reminders_path(self);
        let _serial = self.write();
        let mut all: Vec<Reminder> = read_bin(&path)?.unwrap_or_default();
        all.retain(|r| r.id != id);
        write_bin(&path, &all)
    }

    /// Takes the reminders due at `now` out of the list, and returns them. Each fires once.
    pub fn take_due_reminders(&self, now: u64) -> Result<Vec<Reminder>> {
        let path = reminders_path(self);
        let _serial = self.write();
        let all: Vec<Reminder> = read_bin(&path)?.unwrap_or_default();
        let (due, later): (Vec<Reminder>, Vec<Reminder>) = all.into_iter().partition(|r| r.at_ms <= now);
        if !due.is_empty() {
            write_bin(&path, &later)?;
        }
        Ok(due)
    }
}

/// Fires due reminders as they come, until the engine is dropped.
pub(crate) async fn reminder_clock(inner: Weak<Inner>) {
    loop {
        tokio::time::sleep(CLOCK_EVERY).await;
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let engine = Engine { inner: Arc::clone(&inner) };
        match engine.take_due_reminders(now_ms()) {
            Ok(due) => {
                for reminder in due {
                    engine.emit(EngineEvent::ReminderDue { reminder });
                }
            }
            Err(e) => warn!(error = %e, "could not read reminders"),
        }
    }
}

#[cfg(test)]
mod tests;
