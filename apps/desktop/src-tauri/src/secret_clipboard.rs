//! Copying a secret (the recovery phrase) with the OS clipboard's own privacy marks, which the web clipboard API cannot
//! set: on Windows the entry is kept out of clipboard history (Win+V) and cloud clipboard sync, on macOS it carries the
//! `org.nspasteboard.ConcealedType` marker that clipboard managers skip, and on Linux the `x-kde-passwordManagerHint`
//! that desktop clipboard managers skip. The UI clears it again after 60 s with `clear_secret`.
//! Command names match `ui-next/src/lib/api.ts`.

use std::sync::Mutex;

use arboard::Clipboard;
use tauri::State;

/// The clipboard handle. On Linux (X11) the copied text is served by this process for as long as the handle lives, so
/// it is kept rather than dropped after each copy.
#[derive(Default)]
pub struct SecretClipboard(Mutex<Option<Clipboard>>);

impl SecretClipboard {
    fn with<T>(&self, run: impl FnOnce(&mut Clipboard) -> Result<T, arboard::Error>) -> Result<T, String> {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Clipboard::new().map_err(|e| format!("the clipboard is not available: {e}"))?);
        }
        let clipboard = guard.as_mut().expect("set above");
        run(clipboard).map_err(|e| format!("could not use the clipboard: {e}"))
    }
}

/// Writes `text` with the platform's "do not keep" marks.
fn write_secret(clipboard: &mut Clipboard, text: &str) -> Result<(), arboard::Error> {
    let set = clipboard.set();
    #[cfg(windows)]
    let set = {
        use arboard::SetExtWindows;
        set.exclude_from_history().exclude_from_cloud()
    };
    #[cfg(target_os = "macos")]
    let set = {
        use arboard::SetExtApple;
        set.exclude_from_history()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let set = {
        use arboard::SetExtLinux;
        set.exclude_from_history()
    };
    set.text(text)
}

/// Whether the clipboard, holding `current`, still holds the secret and so should be cleared. Something copied since,
/// or a clipboard that cannot be read, is left alone.
fn still_holds(current: Option<&str>, secret: &str) -> bool {
    !secret.is_empty() && current == Some(secret)
}

#[tauri::command]
pub fn copy_secret(state: State<'_, SecretClipboard>, text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("nothing to copy".into());
    }
    state.with(|c| write_secret(c, &text))
}

/// Clears the clipboard if it still holds `text`. Returns whether it did.
#[tauri::command]
pub fn clear_secret(state: State<'_, SecretClipboard>, text: String) -> Result<bool, String> {
    state.with(|c| {
        let current = c.get_text().ok();
        if !still_holds(current.as_deref(), &text) {
            return Ok(false);
        }
        c.clear().map(|()| true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_secret_itself_is_cleared() {
        assert!(still_holds(Some("harbor velvet ocean"), "harbor velvet ocean"));
        assert!(!still_holds(Some("something else"), "harbor velvet ocean"), "a later copy is left alone");
        assert!(!still_holds(None, "harbor velvet ocean"), "an unreadable or empty clipboard is left alone");
        assert!(!still_holds(Some(""), ""), "an empty secret never clears");
    }

    // Uses the real OS clipboard, which CI runners and headless sessions lack, and overwrites what the user copied.
    #[test]
    #[ignore]
    fn a_copied_secret_reads_back_and_clears() {
        let mut clipboard = Clipboard::new().unwrap();
        write_secret(&mut clipboard, "dissent clipboard test").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "dissent clipboard test");
        let state = SecretClipboard(Mutex::new(Some(clipboard)));
        let cleared = state.with(|c| {
            let current = c.get_text().ok();
            if !still_holds(current.as_deref(), "dissent clipboard test") {
                return Ok(false);
            }
            c.clear().map(|()| true)
        });
        assert_eq!(cleared, Ok(true));
        assert!(state.with(|c| c.get_text()).map_or(true, |t| t.is_empty()), "the clipboard is empty after the clear");
    }
}
