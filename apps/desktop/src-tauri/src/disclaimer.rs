//! The disclaimer's acceptance: the "Last updated" date of the notice the user accepted, saved as `disclaimer.json` in
//! the profile's data folder. The UI shows the notice again whenever the bundled notice's date differs from the saved
//! one, so a changed notice is accepted again. A missing or unreadable file means nothing was accepted yet.
//! Command names match `ui-next/src/lib/api.ts`.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::AppState;

const FILE: &str = "disclaimer.json";
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Acceptance {
    /// The notice's "Last updated" date, as the notice writes it (for example "October 29, 2026").
    accepted: Option<String>,
}

/// The accepted notice's date, or `None` when none was accepted. A byte-order mark, which some editors write, is skipped.
fn load(data_dir: &Path) -> Option<String> {
    std::fs::read(data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Acceptance>(bytes.strip_prefix(BOM).unwrap_or(&bytes)).ok())
        .and_then(|a| a.accepted)
}

/// Saves the accepted date through a temporary file, so a crash never leaves half a file.
fn save(data_dir: &Path, updated: &str) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let bytes = serde_json::to_vec_pretty(&Acceptance { accepted: Some(updated.to_owned()) }).map_err(io::Error::other)?;
    let tmp = data_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, data_dir.join(FILE))
}

#[tauri::command]
pub fn disclaimer_accepted(state: State<'_, AppState>) -> Option<String> {
    load(&state.data_dir)
}

#[tauri::command]
pub fn accept_disclaimer(state: State<'_, AppState>, updated: String) -> Result<(), String> {
    let updated = updated.trim();
    if updated.is_empty() || updated.len() > 64 {
        return Err("the disclaimer's date is missing".into());
    }
    save(&state.data_dir, updated).map_err(|e| format!("could not save the disclaimer's acceptance: {e}"))
}

/// Quit from the disclaimer: the same clean shutdown as the tray's Quit.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    crate::tray::quit(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-disclaimer-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nothing_is_accepted_in_a_new_profile() {
        let dir = scratch("new");
        assert_eq!(load(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_accepted_date_comes_back_and_a_newer_one_replaces_it() {
        let dir = scratch("roundtrip");
        save(&dir, "October 29, 2026").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("October 29, 2026"));
        save(&dir, "January 5, 2027").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("January 5, 2027"));
        assert!(!dir.join("disclaimer.json.tmp").exists(), "the temporary file is renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_byte_order_mark_is_skipped_and_a_broken_file_means_not_accepted() {
        let dir = scratch("bom");
        std::fs::write(dir.join(FILE), b"\xEF\xBB\xBF{\"accepted\":\"October 29, 2026\"}").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("October 29, 2026"));
        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        assert_eq!(load(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
