//! Links that open Dissent: `dissent://invite/…`, `dissent://contact/…`, `dissent://message/…` and `dissent://link/…`.
//! The OS starts the installed app with the link as its argument, or hands the link to the running copy. This module
//! only checks a link and queues it for the UI, and it brings the window forward. The UI shows what the link would do,
//! and acts only when the person confirms in a dialog.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Listener, Manager, Runtime};
use tauri_plugin_deep_link::DeepLinkExt;

/// The scheme this app takes, without `://`.
pub const SCHEME: &str = "dissent";
/// The longest link taken, in bytes. A longer one is ignored.
pub const MAX_LINK_BYTES: usize = 4096;
/// The event that carries links to the UI once it has taken its first batch.
pub const EVENT: &str = "dissent://deep-link";
/// The event the deep-link plugin emits when it sees a link (on macOS, when the app is opened with one).
const PLUGIN_EVENT: &str = "deep-link://new-url";

/// Links waiting for the UI, and whether the UI has taken its first batch. Before that, links wait here; after it, each
/// new link is sent as an event.
#[derive(Default)]
pub struct Queue {
    pending: Mutex<Vec<String>>,
    ui_ready: AtomicBool,
}

/// The link if it is a `dissent://` URL this app takes: within the length cap, and with no control characters. The
/// scheme is matched without regard to case, as URL schemes are.
pub fn accept(raw: &str) -> Option<String> {
    if raw.len() > MAX_LINK_BYTES || raw.chars().any(char::is_control) {
        return None;
    }
    let prefix = raw.get(..SCHEME.len() + "://".len())?;
    prefix.eq_ignore_ascii_case(&format!("{SCHEME}://")).then(|| raw.to_string())
}

/// The `dissent://` links among command-line arguments. The first argument is the program itself, so it is skipped.
pub fn links_in<'a>(args: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    args.into_iter().skip(1).filter_map(accept).collect()
}

/// Takes links from a launch or from a second launch. They are queued for the UI, and the window comes to the front.
/// Nothing acts on a link until the UI shows it and the person confirms.
pub fn offer<R: Runtime>(app: &AppHandle<R>, links: Vec<String>) {
    if links.is_empty() {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let queue = app.state::<Queue>();
    let mut pending = queue.pending.lock().unwrap_or_else(|e| e.into_inner());
    pending.extend(links);
    if queue.ui_ready.load(Ordering::SeqCst) {
        let ready: Vec<String> = pending.drain(..).collect();
        drop(pending);
        let _ = app.emit(EVENT, ready);
    }
}

/// Registers the scheme and takes the link this process was started with. Called from setup, and only for the default
/// profile: the other profiles never claim links or take them.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    // A debug build does not claim the scheme, so it cannot take the installed app's links from the OS.
    if !cfg!(debug_assertions) {
        if let Err(e) = app.deep_link().register_all() {
            tracing::warn!(error = %e, "could not register the dissent:// scheme");
        }
    }
    // Windows and Linux hand the link over as an argument, which the plugin reads when it starts.
    match app.deep_link().get_current() {
        Ok(Some(urls)) => offer(app, urls.iter().filter_map(|u| accept(u.as_str())).collect()),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "could not read the link the app was started with"),
    }
    // macOS delivers a link to the running app as an event from the plugin.
    let handle = app.clone();
    app.listen(PLUGIN_EVENT, move |event| {
        if let Ok(urls) = serde_json::from_str::<Vec<String>>(event.payload()) {
            offer(&handle, urls.iter().filter_map(|u| accept(u)).collect());
        }
    });
}

/// The UI's first call, once its listener for `EVENT` is set up. Returns the links that arrived before it, and later
/// links are sent as events.
#[tauri::command]
pub fn take_deep_links(queue: tauri::State<'_, Queue>) -> Vec<String> {
    queue.ui_ready.store(true, Ordering::SeqCst);
    std::mem::take(&mut *queue.pending.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dissent_link_is_accepted_whatever_the_case_of_its_scheme() {
        assert_eq!(accept("dissent://invite/abc").as_deref(), Some("dissent://invite/abc"));
        assert_eq!(accept("DISSENT://contact/dsu1x").as_deref(), Some("DISSENT://contact/dsu1x"));
    }

    #[test]
    fn other_schemes_and_plain_text_are_refused() {
        assert_eq!(accept("https://example.com/dissent://invite/abc"), None);
        assert_eq!(accept("dissentx://invite/abc"), None);
        assert_eq!(accept("dissent:/invite"), None);
        assert_eq!(accept(""), None);
    }

    #[test]
    fn a_link_over_the_length_cap_is_refused() {
        let long = format!("dissent://invite/{}", "a".repeat(MAX_LINK_BYTES));
        assert_eq!(accept(&long), None);
        let at_cap = format!("dissent://invite/{}", "a".repeat(MAX_LINK_BYTES - "dissent://invite/".len()));
        assert_eq!(at_cap.len(), MAX_LINK_BYTES);
        assert!(accept(&at_cap).is_some());
    }

    #[test]
    fn control_characters_are_refused() {
        assert_eq!(accept("dissent://invite/abc\n"), None);
        assert_eq!(accept("dissent://invite/a\u{0007}bc"), None);
        assert_eq!(accept("dissent://invite/a\tbc"), None);
    }

    #[test]
    fn only_the_arguments_after_the_program_are_read() {
        let args = ["dissent.exe", "dissent://contact/dsu1x", "--other", "dissent://link/code"];
        assert_eq!(links_in(args), ["dissent://contact/dsu1x", "dissent://link/code"]);
        assert!(links_in(["dissent://invite/abc"]).is_empty(), "the first argument is the program");
        assert!(links_in(["dissent.exe", "https://example.com"]).is_empty());
    }
}
