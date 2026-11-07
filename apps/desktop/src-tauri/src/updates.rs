//! In-app updates from GitHub releases. A check runs 10 seconds after startup and then every six hours.
//! Failures are logged and never shown, and never crash the app. Manual checks report their errors to the caller.
//!
//! While "Hide my IP address" is on, nothing is checked on its own: a check shows the IP to GitHub, which the relay
//! hides from everyone else. Through Tor, checks run as usual, through the Tor bridge, and never directly. The user checks by hand, warned, and is reminded once no check has run for
//! `REMIND_AFTER`, so a hidden device does not stay on an old version unknowingly.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use tracing::{info, warn};

use crate::commands::AppState;
use crate::network::IpMode;
use crate::profile;

/// The release page. A portable build can't update in place, so the user downloads the new portable zip here.
const RELEASES_PAGE: &str = "https://github.com/Kaakati/dissent/releases/latest";

/// Emitted when a background check finds a newer release.
pub const EVENT_AVAILABLE: &str = "dissent://update-available";
/// Emitted while an update downloads.
pub const EVENT_PROGRESS: &str = "dissent://update-progress";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a device that hides its IP may go without an update check before it is reminded.
const REMIND_AFTER: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// When the last update check finished, in milliseconds, next to `network.json`. A setting of the app, not of the
/// engine's store, as `network.json` is.
const LAST_CHECK_FILE: &str = "update-check.json";

/// Whether updates are checked on their own, and whether the user should be reminded to check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdatePolicy {
    /// Updates are checked on their own. False while the IP is hidden.
    pub automatic: bool,
    /// The IP is hidden, so a check shows it to GitHub. The UI says so before a check.
    pub hidden_ip: bool,
    /// When the last check finished, in milliseconds since the Unix epoch.
    pub last_checked_ms: Option<u64>,
    /// No check has run for `REMIND_AFTER` while checks are not automatic.
    pub remind: bool,
}

/// The policy for a device that does or doesn't hide its IP, last checked at `last_ms`, now `now_ms`.
pub fn policy(hidden_ip: bool, last_ms: Option<u64>, now_ms: u64) -> UpdatePolicy {
    let stale = last_ms.is_none_or(|at| now_ms.saturating_sub(at) >= REMIND_AFTER.as_millis() as u64);
    UpdatePolicy { automatic: !hidden_ip, hidden_ip, last_checked_ms: last_ms, remind: hidden_ip && stale }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn last_check(data_dir: &std::path::Path) -> Option<u64> {
    let text = std::fs::read_to_string(data_dir.join(LAST_CHECK_FILE)).ok()?;
    serde_json::from_str::<u64>(&text).ok()
}

fn note_check(data_dir: &std::path::Path) {
    let tmp = data_dir.join("update-check.json.tmp");
    let written = std::fs::write(&tmp, now_ms().to_string()).and_then(|()| std::fs::rename(&tmp, data_dir.join(LAST_CHECK_FILE)));
    if let Err(e) = written {
        warn!(error = %e, "could not note the update check");
    }
}

/// Whether a check would show the IP the relays hide: "Hide my IP address" without Tor. Through Tor, GitHub sees a
/// Tor exit, so checks run as usual.
fn check_shows_hidden_ip(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|state| crate::network::load(&state.data_dir).ip_mode() == IpMode::Relays)
}

/// The updater, set up the way this device goes out: through the Tor bridge in Tor mode, and refused when Tor mode is
/// on but the bridge isn't running, so a check never goes out directly.
async fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let tor = app
        .try_state::<AppState>()
        .is_some_and(|state| crate::network::load(&state.data_dir).ip_mode() == IpMode::Tor);
    if !tor {
        return app.updater().map_err(|e| e.to_string());
    }
    let proxy = crate::tor::running_proxy(app)
        .await
        .ok_or("Tor isn't connected, so updates aren't checked")?;
    app.updater_builder().proxy(proxy).build().map_err(|e| e.to_string())
}

/// Whether updates are checked on their own, and whether to remind the user to check.
#[tauri::command]
pub fn update_policy(state: tauri::State<'_, AppState>) -> UpdatePolicy {
    let hidden = crate::network::load(&state.data_dir).ip_mode() == IpMode::Relays;
    policy(hidden, last_check(&state.data_dir), now_ms())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release date as YYYY-MM-DD, when the release has one.
    pub date: Option<String>,
    /// Release notes as published. The UI shows them as plain text.
    pub notes: Option<String>,
    /// True in a portable build. It is told to download the new portable zip instead of installing.
    pub portable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

fn describe(update: &tauri_plugin_updater::Update) -> UpdateInfo {
    UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        date: update.date.map(|d| d.date().to_string()),
        notes: update.body.clone(),
        portable: profile::is_portable_exe(),
    }
}

/// A background check that failed counts as "no update". The failure is logged, never returned.
pub fn background_outcome<T, E: std::fmt::Display>(result: Result<Option<T>, E>) -> Option<T> {
    match result {
        Ok(found) => found,
        Err(e) => {
            warn!(error = %e, "update check failed");
            None
        }
    }
}

/// Asks the release feed whether a newer version exists.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = updater(&app).await?;
    let found = updater.check().await.map_err(|e| e.to_string())?;
    if let Some(state) = app.try_state::<AppState>() {
        note_check(&state.data_dir);
    }
    Ok(found.as_ref().map(describe))
}

/// Opens the release page in the browser. Portable builds send the user here to download the new zip.
#[tauri::command]
pub fn open_release_page(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(RELEASES_PAGE, None::<&str>).map_err(|e| e.to_string())
}

/// Downloads and installs the newest release, shuts the engine down, and restarts the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    if profile::is_portable_exe() {
        return Err("A portable build can't update in place. Download the new portable build instead.".into());
    }
    let update = updater(&app)
        .await?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update is available")?;

    let progress = app.clone();
    let mut downloaded: u64 = 0;
    // Downloaded through Tor in Tor mode, so the built-in Tor runs until then.
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress.emit(EVENT_PROGRESS, Progress { downloaded, total });
            },
            || info!("update downloaded, installing"),
        )
        .await
        .map_err(|e| e.to_string())?;
    // The installer replaces the built-in Tor's program, which Windows does not let it do while it runs.
    crate::tor::stop(&app).await;
    update.install(bytes).map_err(|e| e.to_string())?;

    let engine = state.engine.write().await.take();
    if let Some(engine) = engine {
        if let Err(e) = engine.shutdown().await {
            warn!(error = %e, "engine shutdown before update restart failed");
        }
    }
    app.restart();
}

/// Starts the background check loop. Off in debug builds and loopback mode, where checks would only add noise.
pub fn start_background(app: AppHandle) {
    let loopback = std::env::var_os("DISSENT_LOOPBACK").is_some_and(|v| v == "1");
    if cfg!(debug_assertions) || loopback {
        info!("automatic update checks are off in this build");
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            // Read each time, so turning the setting on stops the checks without a restart.
            if check_shows_hidden_ip(&app) {
                info!("automatic update check skipped: the IP is hidden");
            } else {
                let result = match updater(&app).await {
                    Ok(updater) => updater.check().await.map(|found| found.as_ref().map(describe)).map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                if let Some(state) = app.try_state::<AppState>() {
                    note_check(&state.data_dir);
                }
                if let Some(found) = background_outcome(result) {
                    info!(version = %found.version, "update available");
                    let _ = app.emit(EVENT_AVAILABLE, found);
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_device_is_reminded_after_two_weeks_without_a_check() {
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let open = policy(false, None, now);
        assert!(open.automatic && !open.remind, "a device that shows its IP checks on its own");
        assert!(policy(true, None, now).remind, "never checked");
        assert!(!policy(true, Some(now - 13 * day), now).remind, "checked within two weeks");
        let stale = policy(true, Some(now - 14 * day), now);
        assert!(stale.remind && !stale.automatic && stale.hidden_ip);
    }

    #[test]
    fn unreachable_feed_is_quiet() {
        let unreachable: Result<Option<UpdateInfo>, String> = Err("connection refused".into());
        assert_eq!(background_outcome(unreachable), None);
    }

    #[test]
    fn a_found_update_is_passed_through() {
        let found: Result<Option<u8>, String> = Ok(Some(2));
        assert_eq!(background_outcome(found), Some(2));
        let none: Result<Option<u8>, String> = Ok(None);
        assert_eq!(background_outcome(none), None);
    }
}
