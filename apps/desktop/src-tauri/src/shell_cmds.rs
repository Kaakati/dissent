//! The desktop shell settings as shared state, and the Tauri commands that change them. A change is applied in one
//! place: the sign-in entry first, then the saved file, the tray's check marks, and the UI's event. Names and argument
//! keys match `ui-next/src/lib/api.ts`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tracing::warn;

use crate::profile::Mode;
use crate::shell_settings::{self, ShellSettings};
use crate::tray;

/// The event that carries the settings to the UI after a change, from the window or the tray.
pub const EVENT: &str = "dissent://shell-settings";

/// The shell settings, shared by the window's close handler, the tray and the commands.
pub struct ShellState {
    settings: Mutex<ShellSettings>,
    data_dir: PathBuf,
    login_note: Option<&'static str>,
}

impl ShellState {
    pub fn new(data_dir: &Path, mode: Mode) -> Self {
        Self {
            settings: Mutex::new(shell_settings::load(data_dir)),
            data_dir: data_dir.to_path_buf(),
            login_note: shell_settings::start_at_login_note(mode),
        }
    }

    pub fn get(&self) -> ShellSettings {
        *self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records that the "still running in the tray" notice has been shown, so it is shown only once.
    pub fn mark_tray_notice_shown(&self) -> Result<(), String> {
        self.update(|s| s.tray_notice_shown = true).map(|_| ())
    }

    /// Changes the settings and saves them. Returns the new settings.
    fn update(&self, change: impl FnOnce(&mut ShellSettings)) -> Result<ShellSettings, String> {
        let mut guard = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = *guard;
        change(&mut next);
        shell_settings::save(&self.data_dir, &next).map_err(|e| format!("could not save the desktop settings: {e}"))?;
        *guard = next;
        Ok(next)
    }
}

/// What the settings page shows. `start_at_login_available` says whether the toggle can be used in this build.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ShellView {
    pub keep_running: bool,
    pub start_at_login: bool,
    pub start_at_login_available: bool,
    /// Why start at login is not offered, when it is not. `null` when it is.
    pub start_at_login_note: Option<&'static str>,
    pub notifications_muted: bool,
    pub emoticons_as_text: bool,
    pub keep_photo_metadata: bool,
}

#[derive(Deserialize)]
pub struct ShellSettingsInput {
    pub keep_running: bool,
    pub start_at_login: bool,
    pub notifications_muted: bool,
    /// Absent from a window older than this setting, which leaves it as it was.
    #[serde(default)]
    pub emoticons_as_text: Option<bool>,
    /// Absent from a window older than this setting, which leaves it as it was.
    #[serde(default)]
    pub keep_photo_metadata: Option<bool>,
}

fn view(state: &ShellState, settings: ShellSettings) -> ShellView {
    ShellView {
        keep_running: settings.keep_running,
        start_at_login: settings.start_at_login,
        start_at_login_available: state.login_note.is_none(),
        start_at_login_note: state.login_note,
        notifications_muted: settings.notifications_muted,
        emoticons_as_text: settings.emoticons_as_text,
        keep_photo_metadata: settings.keep_photo_metadata,
    }
}

/// Registers or removes the sign-in entry. The entry starts Dissent with `--background`.
fn set_start_at_login(app: &AppHandle, on: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if on { manager.enable() } else { manager.disable() };
    result.map_err(|e| format!("could not change start at login: {e}"))
}

/// Applies a whole change: the sign-in entry, then the saved file, the tray and the UI. A refused sign-in entry
/// leaves the saved settings as they were.
fn apply(app: &AppHandle, state: &ShellState, input: ShellSettingsInput) -> Result<ShellView, String> {
    if input.start_at_login && state.login_note.is_some() {
        return Err("start at login is not available in this build".into());
    }
    if input.start_at_login != state.get().start_at_login {
        set_start_at_login(app, input.start_at_login)?;
    }
    let saved = state.update(|s| {
        s.keep_running = input.keep_running;
        s.start_at_login = input.start_at_login;
        s.notifications_muted = input.notifications_muted;
        if let Some(as_text) = input.emoticons_as_text {
            s.emoticons_as_text = as_text;
        }
        if let Some(keep) = input.keep_photo_metadata {
            s.keep_photo_metadata = keep;
        }
    })?;
    apply_photo_metadata(app, saved.keep_photo_metadata);
    tray::sync_mute(app, saved.notifications_muted);
    let shown = view(state, saved);
    if let Err(e) = app.emit(EVENT, shown) {
        warn!(error = %e, "could not tell the window about the desktop settings");
    }
    Ok(shown)
}

/// Tells the engine whether photos keep their location and camera details. The engine keeps no copy of the choice, so
/// this runs on every start and on every change.
pub fn apply_photo_metadata(app: &AppHandle, keep: bool) {
    let state = app.state::<crate::commands::AppState>();
    // try_read, not block_on: this can run inside the async runtime, where blocking on it panics.
    let Ok(guard) = state.engine.try_read() else { return };
    if let Some(engine) = guard.as_ref() {
        engine.set_keep_photo_metadata(keep);
    }
}

/// Turns notification muting on or off from the tray. The change goes through the same path as the settings page.
pub fn set_muted(app: &AppHandle, muted: bool) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let current = state.get();
    apply(
        app,
        &state,
        ShellSettingsInput {
            keep_running: current.keep_running,
            start_at_login: current.start_at_login,
            notifications_muted: muted,
            emoticons_as_text: None,
            keep_photo_metadata: None,
        },
    )
    .map(|_| ())
}

#[tauri::command]
pub fn shell_settings(state: State<'_, ShellState>) -> ShellView {
    view(&state, state.get())
}

#[tauri::command]
pub fn set_shell_settings(app: AppHandle, state: State<'_, ShellState>, settings: ShellSettingsInput) -> Result<ShellView, String> {
    apply(&app, &state, settings)
}
