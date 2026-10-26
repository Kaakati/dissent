//! Tauri commands for this device's per-server preferences: a timed mute, which messages alert,
//! whether `@everyone` counts, and whether muted channels are hidden. Names and argument keys match
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
