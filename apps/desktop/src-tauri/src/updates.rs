//! In-app updates from GitHub releases. A check runs 10 seconds after startup and then every six hours.
//! Failures are logged and never shown, and never crash the app. Manual checks report their errors to the caller.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use tracing::{info, warn};

use crate::commands::AppState;
use crate::profile;

/// The release page. A portable build can't update in place, so the user downloads the new portable zip here.
const RELEASES_PAGE: &str = "https://github.com/Kaakati/dissent/releases/latest";

/// Emitted when a background check finds a newer release.
pub const EVENT_AVAILABLE: &str = "dissent://update-available";
/// Emitted while an update downloads.
pub const EVENT_PROGRESS: &str = "dissent://update-progress";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

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
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| e.to_string())?;
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
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update is available")?;

    let progress = app.clone();
    let mut downloaded: u64 = 0;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress.emit(EVENT_PROGRESS, Progress { downloaded, total });
            },
            || info!("update downloaded, installing"),
        )
        .await
        .map_err(|e| e.to_string())?;

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
            let result = match app.updater() {
                Ok(updater) => updater.check().await.map(|found| found.as_ref().map(describe)),
                Err(e) => Err(e),
            };
            if let Some(found) = background_outcome(result) {
                info!(version = %found.version, "update available");
                let _ = app.emit(EVENT_AVAILABLE, found);
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
