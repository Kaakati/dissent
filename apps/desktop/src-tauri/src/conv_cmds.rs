//! Tauri commands for the actions on direct messages and group DMs: reply, react, unreact, edit and
//! delete. Each forwards to the engine. Names and argument keys match `ui/api.js`.

use dissent_core::{ConvId, DmItem, RootId};
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

#[tauri::command]
pub async fn dm_reply(
    state: State<'_, AppState>,
    peer: String,
    message_id: String,
    text: String,
) -> Result<DmItem, String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.dm_reply(peer, &message_id, text).await.map_err(err)
}

#[tauri::command]
pub async fn dm_react(
    state: State<'_, AppState>,
    peer: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.dm_react(peer, &message_id, emoji, true).await.map_err(err)
}

#[tauri::command]
pub async fn dm_unreact(
    state: State<'_, AppState>,
    peer: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.dm_react(peer, &message_id, emoji, false).await.map_err(err)
}

#[tauri::command]
pub async fn dm_edit(
    state: State<'_, AppState>,
    peer: String,
    message_id: String,
    text: String,
) -> Result<DmItem, String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.dm_edit(peer, &message_id, text).await.map_err(err)
}

#[tauri::command]
pub async fn dm_delete(state: State<'_, AppState>, peer: String, message_id: String) -> Result<(), String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.dm_delete(peer, &message_id).await.map_err(err)
}

#[tauri::command]
pub async fn group_reply(
    state: State<'_, AppState>,
    group_id: String,
    message_id: String,
    text: String,
) -> Result<DmItem, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.conv_reply(conv, &message_id, text).await.map_err(err)
}

#[tauri::command]
pub async fn group_react(
    state: State<'_, AppState>,
    group_id: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.conv_react(conv, &message_id, emoji, true).await.map_err(err)
}

#[tauri::command]
pub async fn group_unreact(
    state: State<'_, AppState>,
    group_id: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.conv_react(conv, &message_id, emoji, false).await.map_err(err)
}

#[tauri::command]
pub async fn group_edit(
    state: State<'_, AppState>,
    group_id: String,
    message_id: String,
    text: String,
) -> Result<DmItem, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.conv_edit(conv, &message_id, text).await.map_err(err)
}

#[tauri::command]
pub async fn group_delete(state: State<'_, AppState>, group_id: String, message_id: String) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.conv_delete(conv, &message_id).await.map_err(err)
}
