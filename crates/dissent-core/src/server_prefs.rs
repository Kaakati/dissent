//! The preferences of one server on this device: a timed mute, which messages alert, whether
//! `@everyone` counts, and whether muted channels are hidden. They are this user's own choices, so
//! they live in a local file beside `rail_order.bin` and are never sent to anyone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, write_bin};
use crate::world::World;

/// A mute that lasts until it is turned back on. It is 2^63 rather than `u64::MAX` because the UI
/// reads numbers as JavaScript doubles, and only values up to 2^63 survive that round trip.
pub const MUTED_FOREVER: u64 = 1 << 63;

/// Which messages in a server alert this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    /// Every message alerts. A message that mentions nobody alerts only at this level.
    All,
    /// Only messages that mention this device alert.
    #[default]
    Mentions,
    /// Nothing alerts.
    Nothing,
}

/// The preferences of one server. A server with none saved uses the defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPrefs {
    /// The time the mute ends, in ms since the epoch. `None` is not muted.
    pub muted_until: Option<u64>,
    pub notify: Notify,
    /// Whether an `@everyone` counts as a mention, for alerts and unread counts.
    pub suppress_everyone: bool,
    /// Whether this server's muted channels are hidden from its sidebar.
    pub hide_muted_channels: bool,
}

impl ServerPrefs {
    /// These preferences as of `now`. A mute whose time has passed is dropped, so an expired mute
    /// ends without a restart.
    pub fn at(self, now: u64) -> Self {
        match self.muted_until {
            Some(until) if until <= now => Self { muted_until: None, ..self },
            _ => self,
        }
    }

    /// Whether the server is muted as of `now`.
    pub fn is_muted(&self, now: u64) -> bool {
        self.muted_until.is_some_and(|until| until > now)
    }

    /// Whether a new message in the server alerts this device. `mentioned` is whether the message
    /// mentions this device, by name or through `@everyone`. `by_name` is whether it names this
    /// device. A mention alerts through a mute, as it does in a muted channel.
    pub fn alerts(&self, now: u64, mentioned: bool, by_name: bool) -> bool {
        let prefs = self.at(now);
        if prefs.notify == Notify::Nothing {
            return false;
        }
        if mentioned {
            return by_name || !prefs.suppress_everyone;
        }
        prefs.notify == Notify::All && !prefs.is_muted(now)
    }
}

fn path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("server_prefs.bin")
}

/// Every saved server. A missing or unreadable file saves nothing, so the defaults are used.
fn read_all(path: &Path) -> BTreeMap<CommunityId, ServerPrefs> {
    read_bin(path).ok().flatten().unwrap_or_default()
}

/// The preferences of `community` as of now, for the alert and count logic.
pub(crate) fn of(w: &World, community: CommunityId) -> ServerPrefs {
    read_all(&path(w)).get(&community).copied().unwrap_or_default().at(now_ms())
}

impl Engine {
    /// The preferences of a server this device belongs to, as of now.
    pub fn server_prefs(&self, community: CommunityId) -> ServerPrefs {
        of(&self.read(), community)
    }

    /// Saves the preferences of a server this device belongs to.
    pub fn set_server_prefs(&self, community: CommunityId, prefs: ServerPrefs) -> Result<()> {
        if !self.read().communities.contains_key(&community) {
            return Err(CoreError::NotFound(format!("community {community}")));
        }
        let path = path(&self.read());
        let _serial = self.write();
        let mut all = read_all(&path);
        all.insert(community, prefs);
        write_bin(&path, &all)
    }

    /// Whether a new message in a server alerts this device, as of now. See [`ServerPrefs::alerts`].
    pub fn server_alerts(&self, community: CommunityId, mentioned: bool, by_name: bool) -> bool {
        self.server_prefs(community).alerts(now_ms(), mentioned, by_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn id(byte: u8) -> CommunityId {
        CommunityId([byte; 32])
    }

    #[test]
    fn a_timed_mute_ends_when_its_time_passes() {
        let muted = ServerPrefs { muted_until: Some(NOW + 1), ..Default::default() };
        assert!(muted.is_muted(NOW));
        assert_eq!(muted.at(NOW).muted_until, Some(NOW + 1));
        assert!(!muted.at(NOW + 1).is_muted(NOW + 1));
        assert_eq!(muted.at(NOW + 1).muted_until, None, "an expired mute is dropped on read");
    }

    #[test]
    fn a_forever_mute_never_expires() {
        let muted = ServerPrefs { muted_until: Some(MUTED_FOREVER), ..Default::default() };
        assert!(muted.at(u64::MAX / 2).is_muted(u64::MAX / 2));
        assert_eq!(MUTED_FOREVER as f64 as u64, MUTED_FOREVER, "it must survive a JavaScript round trip");
    }

    #[test]
    fn the_default_alerts_only_on_mentions() {
        let prefs = ServerPrefs::default();
        assert!(!prefs.alerts(NOW, false, false), "a plain message does not alert");
        assert!(prefs.alerts(NOW, true, true), "a mention alerts");
    }

    #[test]
    fn all_messages_alert_plain_messages_unless_muted() {
        let all = ServerPrefs { notify: Notify::All, ..Default::default() };
        assert!(all.alerts(NOW, false, false));
        let muted = ServerPrefs { muted_until: Some(NOW + 1), ..all };
        assert!(!muted.alerts(NOW, false, false), "a muted server alerts for no plain message");
        assert!(muted.alerts(NOW, true, true), "a mention alerts through a mute");
    }

    #[test]
    fn nothing_alerts_at_all() {
        let nothing = ServerPrefs { notify: Notify::Nothing, ..Default::default() };
        assert!(!nothing.alerts(NOW, false, false));
        assert!(!nothing.alerts(NOW, true, true));
    }

    #[test]
    fn suppressed_everyone_alerts_only_for_a_name() {
        let prefs = ServerPrefs { suppress_everyone: true, ..Default::default() };
        assert!(!prefs.alerts(NOW, true, false), "an @everyone alone does not alert");
        assert!(prefs.alerts(NOW, true, true), "a name still alerts");
    }

    #[test]
    fn a_corrupt_or_missing_file_means_the_defaults() {
        let dir = std::env::temp_dir().join(format!("dissent-server-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server_prefs.bin");
        assert!(read_all(&file).is_empty(), "a missing file saves nothing");
        std::fs::write(&file, [0xff, 0xff, 0xff]).unwrap();
        assert!(read_all(&file).is_empty(), "a corrupt file saves nothing");
        let saved = ServerPrefs { notify: Notify::Nothing, hide_muted_channels: true, ..Default::default() };
        write_bin(&file, &BTreeMap::from([(id(3), saved)])).unwrap();
        assert_eq!(read_all(&file).get(&id(3)), Some(&saved));
        assert_eq!(read_all(&file).get(&id(4)), None, "an unsaved server takes the defaults");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
