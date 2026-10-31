//! Tauri commands for offline delivery: the inboxes this person picks, and the people this device holds mail
//! for. See `docs/design/10-dm-delivery.md`. Names and argument keys match `ui-next/src/lib/api.ts`.

use dissent_core::{DeviceId, HostNoticeView, HostedView, InboxView, RootId, MAX_HOSTED, MAX_INBOXES};
use serde::Serialize;
use tauri::State;

use crate::commands::{engine, err, AppState};

/// What this device does for offline delivery, without asking any other device.
#[derive(Serialize)]
pub struct InboxSettings {
    /// This device holds mail for our own identity.
    pub serving_own: bool,
    /// The people this device holds mail for, besides us.
    pub hosted: Vec<HostedView>,
    /// Envelopes and bytes this device holds, for everyone it serves.
    pub envelopes: usize,
    pub bytes: u64,
    pub max_inboxes: usize,
    pub max_hosted: usize,
    /// Devices that told us they started or stopped holding our mail, newest first.
    pub notices: Vec<HostNoticeView>,
}

#[tauri::command]
pub async fn inbox_settings(state: State<'_, AppState>) -> Result<InboxSettings, String> {
    let engine = engine(&state).await?;
    let stats = engine.inbox_stats();
    Ok(InboxSettings {
        serving_own: stats.serving,
        hosted: engine.hosted_inboxes(),
        envelopes: stats.envelopes,
        bytes: stats.bytes,
        max_inboxes: MAX_INBOXES,
        max_hosted: MAX_HOSTED,
        notices: engine.inbox_notices(),
    })
}

/// Our chosen inboxes, each asked now whether it holds mail for us.
#[tauri::command]
pub async fn inbox_status(state: State<'_, AppState>) -> Result<Vec<InboxView>, String> {
    Ok(engine(&state).await?.inbox_status().await)
}

/// Devices that offer to hold our mail and are not chosen yet.
#[tauri::command]
pub async fn inbox_candidates(state: State<'_, AppState>) -> Result<Vec<InboxView>, String> {
    Ok(engine(&state).await?.inbox_candidates().await)
}

/// Replaces our inbox list. Contacts learn it, and the new inboxes get key packages.
#[tauri::command]
pub async fn set_inboxes(state: State<'_, AppState>, devices: Vec<String>) -> Result<(), String> {
    let devices = devices
        .iter()
        .map(|d| d.trim().parse::<DeviceId>().map_err(|e| format!("bad device id: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    engine(&state).await?.set_inboxes(devices).await.map_err(err)
}

#[tauri::command]
pub async fn set_serve_inbox(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    engine(&state).await?.set_serve_inbox(on).map_err(err)
}

#[tauri::command]
pub async fn host_inbox_for(state: State<'_, AppState>, root: String, on: bool) -> Result<(), String> {
    let root: RootId = root.trim().parse().map_err(|e| format!("bad user id: {e}"))?;
    engine(&state).await?.host_inbox_for(root, on).map_err(err)
}
