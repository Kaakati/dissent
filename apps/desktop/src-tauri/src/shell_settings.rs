//! Desktop shell settings: whether closing the window keeps Dissent running in the tray, whether it opens at sign-in,
//! and whether desktop notifications are muted. Saved as `shell.json` in the data folder, beside the network settings.
//! A missing or unreadable file gives the defaults, which are all off.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profile::Mode;

const FILE: &str = "shell.json";
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// The argument the sign-in entry passes, so a start at login stays in the tray when "keep running" is on.
pub const BACKGROUND_ARG: &str = "--background";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    /// Closing the window hides it, and the engine keeps running in the tray.
    pub keep_running: bool,
    /// Dissent opens when the user signs in. Only the installed app offers this.
    pub start_at_login: bool,
    /// Desktop notifications for new messages are off. Sounds are not affected.
    pub notifications_muted: bool,
    /// The "still running in the tray" notice has been shown once.
    pub tray_notice_shown: bool,
    /// Emoticons such as `:D` are sent as typed, not converted to emoji. Off by default, so they convert.
    pub emoticons_as_text: bool,
    /// Photos and videos are sent with their location and camera details. Off by default, so they are removed.
    pub keep_photo_metadata: bool,
}

/// Why "start at login" is not offered in `mode`, for the settings page. `None` when it is offered. A portable build
/// runs from a folder that may be on a USB stick, so it never registers itself at sign-in. Dev and custom profiles run
/// from a build that is not installed, so they are not offered it either.
pub fn start_at_login_note(mode: Mode) -> Option<&'static str> {
    match mode {
        Mode::Default => None,
        Mode::Portable => Some("Portable builds don't open at sign-in. The app runs from a folder that may be on a USB stick."),
        _ => Some("Start at login is set up by the installed app."),
    }
}

/// The saved settings, or the defaults when the file is missing or cannot be read. A byte-order mark, which some
/// editors write, is skipped.
pub fn load(data_dir: &Path) -> ShellSettings {
    std::fs::read(data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(bytes.strip_prefix(BOM).unwrap_or(&bytes)).ok())
        .unwrap_or_default()
}

/// Writes the settings through a temporary file, so a crash never leaves half a file.
pub fn save(data_dir: &Path, settings: &ShellSettings) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    let tmp = data_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, data_dir.join(FILE))
}

/// Whether this launch came from the sign-in entry, which starts minimised to the tray.
pub fn launched_in_background() -> bool {
    std::env::args().any(|arg| arg == BACKGROUND_ARG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-shell-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn everything_is_off_until_the_user_turns_it_on() {
        let dir = scratch("defaults");
        assert_eq!(load(&dir), ShellSettings::default());
        assert!(!load(&dir).keep_photo_metadata, "photo details are removed unless the user keeps them");
        assert!(!load(&dir).keep_running && !load(&dir).start_at_login && !load(&dir).notifications_muted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_the_photo_setting_removes_photo_details() {
        let dir = scratch("older");
        std::fs::write(dir.join(FILE), br#"{"keep_running":true,"emoticons_as_text":true}"#).unwrap();
        let loaded = load(&dir);
        assert!(loaded.keep_running && loaded.emoticons_as_text && !loaded.keep_photo_metadata);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_settings_come_back_unchanged() {
        let dir = scratch("roundtrip");
        let saved = ShellSettings {
            keep_running: true,
            start_at_login: true,
            notifications_muted: true,
            tray_notice_shown: true,
            emoticons_as_text: true,
            keep_photo_metadata: true,
        };
        save(&dir, &saved).unwrap();
        assert_eq!(load(&dir), saved);
        assert!(!dir.join("shell.json.tmp").exists(), "the temporary file is renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_the_emoticon_setting_converts_emoticons() {
        let dir = scratch("emoticons");
        std::fs::write(dir.join(FILE), br#"{"keep_running": true, "start_at_login": false, "notifications_muted": false, "tray_notice_shown": true}"#).unwrap();
        let loaded = load(&dir);
        assert!(loaded.keep_running && !loaded.emoticons_as_text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_gives_the_defaults_rather_than_an_error() {
        let dir = scratch("damaged");
        std::fs::write(dir.join(FILE), b"{ not json").unwrap();
        assert_eq!(load(&dir), ShellSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_byte_order_mark_from_an_editor_is_skipped() {
        let dir = scratch("bom");
        std::fs::write(dir.join(FILE), b"\xEF\xBB\xBF{\"keep_running\": true}").unwrap();
        assert!(load(&dir).keep_running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_a_field_existed_keeps_the_rest() {
        let dir = scratch("partial");
        std::fs::write(dir.join(FILE), br#"{"keep_running": true}"#).unwrap();
        let loaded = load(&dir);
        assert!(loaded.keep_running);
        assert!(!loaded.start_at_login && !loaded.notifications_muted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_at_login_is_offered_only_by_the_installed_app() {
        assert_eq!(start_at_login_note(Mode::Default), None);
        assert!(start_at_login_note(Mode::Portable).is_some_and(|note| note.contains("USB stick")));
        assert!(start_at_login_note(Mode::Custom).is_some());
        assert!(start_at_login_note(Mode::Installed).is_some());
    }
}
