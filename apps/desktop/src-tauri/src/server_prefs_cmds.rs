//! Tauri commands for this device's per-server preferences: a timed mute, which messages alert,
//! whether `@everyone` counts, whether muted channels are hidden, and whether this device helps
//! forward the server's calls. Names and argument keys match
//! `ui-next/src/lib/api.ts`.

use dissent_core::{CommunityId, ServerPrefs};
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

#[tauri::command]
pub async fn get_server_prefs(state: State<'_, AppState>, community_id: String) -> Result<ServerPrefs, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    Ok(engine(&state).await?.server_prefs(community))
}

#[tauri::command]
pub async fn set_server_prefs(state: State<'_, AppState>, community_id: String, prefs: ServerPrefs) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_server_prefs(community, prefs).map_err(err)
}

/// Whether a new message in the server alerts this device now. `mentioned` is whether it mentions
/// this device, by name or through `@everyone`, and `by_name` whether it names this device.
#[tauri::command]
pub async fn server_alerts(state: State<'_, AppState>, community_id: String, mentioned: bool, by_name: bool) -> Result<bool, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    Ok(engine(&state).await?.server_alerts(community, mentioned, by_name))
}

/// Whether this device helps forward voice calls in the server ("Help forward calls in this server").
#[tauri::command]
pub async fn get_forward_calls(state: State<'_, AppState>, community_id: String) -> Result<bool, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    Ok(engine(&state).await?.forwards_calls(community))
}

/// Turns forwarding calls in the server on or off for this device. Off by default.
#[tauri::command]
pub async fn set_forward_calls(state: State<'_, AppState>, community_id: String, on: bool) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_forward_calls(community, on).map_err(err)
}
