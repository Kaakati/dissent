//! Tauri commands for contacts, direct conversations and group conversations. Each forwards to
//! the engine method of the same concept. Names and argument keys match `ui/api.js`.

use dissent_core::{
    ConversationSummary, ConvId, DmItem, DmPolicy, RootId, UserView,
};
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

const DEFAULT_LIMIT: usize = 100;

fn parse_peers(peers: &[String]) -> Result<Vec<RootId>, String> {
    peers.iter().map(|p| parse("peer id", p)).collect()
}

// ---- contacts ----

#[tauri::command]
pub async fn list_contacts(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    crate::avatar_cmds::for_ui(&engine(&state).await?.list_contacts())
}

#[tauri::command]
pub async fn send_contact_request(state: State<'_, AppState>, link: String) -> Result<UserView, String> {
    engine(&state).await?.add_contact(&link).await.map_err(err)
}

/// Whether people may add us by address (the user record is published).
#[tauri::command]
pub async fn address_discovery(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(engine(&state).await?.address_discovery())
}

#[tauri::command]
pub async fn set_address_discovery(state: State<'_, AppState>, on: bool) -> Result<bool, String> {
    engine(&state).await?.set_address_discovery(on).map_err(err)?;
    Ok(on)
}

#[tauri::command]
pub async fn accept_contact(state: State<'_, AppState>, root_id: String) -> Result<UserView, String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.accept_request(user).await.map_err(err)
}

#[tauri::command]
pub async fn decline_contact(state: State<'_, AppState>, root_id: String) -> Result<(), String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.decline_request(user).map_err(err)
}

#[tauri::command]
pub async fn cancel_contact_request(state: State<'_, AppState>, root_id: String) -> Result<(), String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.cancel_contact_request(user).map_err(err)
}

#[tauri::command]
pub async fn remove_contact(state: State<'_, AppState>, root_id: String) -> Result<(), String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.remove_contact(user).map_err(err)
}

#[tauri::command]
pub async fn block_user(state: State<'_, AppState>, root_id: String) -> Result<(), String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.block_user(user).map_err(err)
}

#[tauri::command]
pub async fn unblock_user(state: State<'_, AppState>, root_id: String) -> Result<(), String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.unblock_user(user).map_err(err)
}

#[tauri::command]
pub async fn peer_contact_link(state: State<'_, AppState>, root_id: String) -> Result<String, String> {
    let user: RootId = parse("contact id", &root_id)?;
    engine(&state).await?.peer_contact_link(user).map_err(err)
}

#[tauri::command]
pub async fn dm_policy(state: State<'_, AppState>) -> Result<DmPolicy, String> {
    Ok(engine(&state).await?.dm_policy())
}

#[tauri::command]
pub async fn set_dm_policy(state: State<'_, AppState>, policy: DmPolicy) -> Result<(), String> {
    engine(&state).await?.set_dm_policy(policy).map_err(err)
}

// ---- conversations and groups ----

#[tauri::command]
pub async fn pinned_conversations(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(engine(&state).await?.pinned_conversations())
}

#[tauri::command]
pub async fn create_group(
    state: State<'_, AppState>,
    name: String,
    peers: Vec<String>,
) -> Result<ConversationSummary, String> {
    let members = parse_peers(&peers)?;
    engine(&state).await?.create_group(name, members).await.map_err(err)
}

#[tauri::command]
pub async fn group_messages(
    state: State<'_, AppState>,
    group_id: String,
    limit: Option<usize>,
) -> Result<Vec<DmItem>, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    Ok(engine(&state).await?.conversation_messages(conv, limit.unwrap_or(DEFAULT_LIMIT)))
}

#[tauri::command]
pub async fn send_group_message(
    state: State<'_, AppState>,
    group_id: String,
    text: String,
) -> Result<DmItem, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.send_to_conversation(conv, text).await.map_err(err)
}

#[tauri::command]
pub async fn add_group_members(
    state: State<'_, AppState>,
    group_id: String,
    peers: Vec<String>,
) -> Result<ConversationSummary, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    let members = parse_peers(&peers)?;
    engine(&state).await?.group_add(conv, members).await.map_err(err)
}

#[tauri::command]
pub async fn leave_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.group_leave(conv).await.map_err(err)
}
