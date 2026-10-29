//! Tauri commands for files, transfers, the file store, group invites and link previews. Each one
//! forwards to the engine method of the same concept. Argument keys match what `ui-next/src/lib/api.ts` sends,
//! which Tauri maps from camelCase to snake_case.

use std::path::PathBuf;

use dissent_core::{
    CommunityId, ConvId, ConversationSummary, DmItem, JoinRequestView, MessageItem, RootId,
    StoreStats, TransferSettings, TransferView,
};
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::{engine, err, mentions_of_arg, parse, AppState, MentionsArg};

// ---- transfer settings and the file store ----

/// Input for `set_transfer_settings`. Only the auto-accept limit can be changed from the UI.
#[derive(Deserialize)]
pub struct TransferSettingsInput {
    /// Files at or under this many bytes download on arrival. `null` means always ask.
    pub auto_accept_bytes: Option<u64>,
}

#[tauri::command]
pub async fn transfer_settings(state: State<'_, AppState>) -> Result<TransferSettings, String> {
    Ok(engine(&state).await?.transfer_settings())
}

#[tauri::command]
pub async fn set_transfer_settings(
    state: State<'_, AppState>,
    settings: TransferSettingsInput,
) -> Result<TransferSettings, String> {
    engine(&state).await?.set_transfer_settings(settings.auto_accept_bytes).map_err(err)
}

/// Opens a folder dialog and makes the chosen folder the download folder. Cancelling changes nothing.
#[tauri::command]
pub async fn pick_download_dir(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TransferSettings, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(engine(&state).await?.transfer_settings());
    };
    let dir: PathBuf = picked.into_path().map_err(err)?;
    engine(&state).await?.set_download_dir(Some(dir)).map_err(err)
}

/// Makes the default folder the download folder again: the OS Downloads folder, or the profile's own folder for a
/// portable exe. Returns the settings, with the folder downloads go to now.
#[tauri::command]
pub async fn reset_download_dir(state: State<'_, AppState>) -> Result<TransferSettings, String> {
    engine(&state).await?.set_download_dir(None).map_err(err)
}

#[tauri::command]
pub async fn store_stats(state: State<'_, AppState>) -> Result<StoreStats, String> {
    engine(&state).await?.store_stats().await.map_err(err)
}

// ---- sending files ----

fn to_paths(paths: Vec<String>) -> Vec<PathBuf> {
    paths.into_iter().map(PathBuf::from).collect()
}

// The sender's message shows its own files from where they were shared, so the webview may read them before the
// message comes back.
fn allow_shared(app: &AppHandle, engine: &dissent_core::Engine) {
    crate::attachments::allow_saved(app, &engine.saved_files());
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // a Tauri command takes its arguments flat, as the UI sends them
pub async fn send_files(
    app: AppHandle,
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    paths: Vec<String>,
    // Files may go without a message: the UI sends null then.
    text: Option<String>,
    seed: Option<bool>,
    mentions: Option<MentionsArg>,
) -> Result<MessageItem, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel = parse("channel id", &channel_id)?;
    let mentions = mentions_of_arg(mentions)?;
    let engine = engine(&state).await?;
    let item = engine
        .send_files_with(community, channel, to_paths(paths), text.unwrap_or_default(), seed.unwrap_or(true), mentions)
        .await
        .map_err(err)?;
    allow_shared(&app, &engine);
    Ok(item)
}

#[tauri::command]
pub async fn send_dm_files(
    app: AppHandle,
    state: State<'_, AppState>,
    peer: String,
    paths: Vec<String>,
    text: Option<String>,
) -> Result<DmItem, String> {
    let peer: RootId = parse("peer id", &peer)?;
    let engine = engine(&state).await?;
    let item = engine.send_dm_files(peer, to_paths(paths), text.unwrap_or_default()).await.map_err(err)?;
    allow_shared(&app, &engine);
    Ok(item)
}

// ---- transfers ----

#[tauri::command]
pub async fn list_transfers(state: State<'_, AppState>) -> Result<Vec<TransferView>, String> {
    Ok(engine(&state).await?.transfers())
}

#[tauri::command]
pub async fn download_attachment(
    state: State<'_, AppState>,
    community_id: Option<String>,
    message_id: String,
    attachment_id: String,
) -> Result<TransferView, String> {
    let community: Option<CommunityId> = match community_id {
        Some(id) => Some(parse("community id", &id)?),
        None => None,
    };
    engine(&state).await?.download_attachment(community, message_id, attachment_id).await.map_err(err)
}

#[tauri::command]
pub async fn pause_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    engine(&state).await?.pause_transfer(&transfer_id).map_err(err)
}

#[tauri::command]
pub async fn resume_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    engine(&state).await?.resume_transfer(&transfer_id).map_err(err)
}

#[tauri::command]
pub async fn retry_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    engine(&state).await?.retry_transfer(&transfer_id).map_err(err)
}

#[tauri::command]
pub async fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    engine(&state).await?.cancel_transfer(&transfer_id).map_err(err)
}

#[tauri::command]
pub async fn remove_transfer(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    engine(&state).await?.remove_transfer(&transfer_id).map_err(err)
}

// ---- group invites and join requests ----

async fn invite_link(state: &AppState, group_id: &str) -> Result<String, String> {
    let conv: ConvId = parse("group id", group_id)?;
    engine(state).await?.create_group_invite(conv).map_err(err)
}

/// Makes an invite link for a group. The UI calls this `group_invite_link`.
#[tauri::command]
pub async fn create_group_invite(state: State<'_, AppState>, group_id: String) -> Result<String, String> {
    invite_link(&state, &group_id).await
}

/// The name the UI uses for `create_group_invite`.
#[tauri::command]
pub async fn group_invite_link(state: State<'_, AppState>, group_id: String) -> Result<String, String> {
    invite_link(&state, &group_id).await
}

#[tauri::command]
pub async fn revoke_group_invites(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    engine(&state).await?.revoke_group_invites(conv).map_err(err)
}

#[tauri::command]
pub async fn join_group_by_invite(state: State<'_, AppState>, link: String) -> Result<(), String> {
    engine(&state).await?.join_group_by_invite(&link).await.map_err(err)
}

#[tauri::command]
pub async fn group_join_requests(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<JoinRequestView>, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    Ok(engine(&state).await?.group_join_requests(conv))
}

#[tauri::command]
pub async fn approve_group_join(
    state: State<'_, AppState>,
    group_id: String,
    root_id: String,
) -> Result<ConversationSummary, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    let user: RootId = parse("user id", &root_id)?;
    engine(&state).await?.approve_group_join(conv, user).await.map_err(err)
}

#[tauri::command]
pub async fn decline_group_join(
    state: State<'_, AppState>,
    group_id: String,
    root_id: String,
) -> Result<(), String> {
    let conv: ConvId = parse("group id", &group_id)?;
    let user: RootId = parse("user id", &root_id)?;
    engine(&state).await?.decline_group_join(conv, user).map_err(err)
}

#[tauri::command]
pub async fn pending_group_joins(state: State<'_, AppState>) -> Result<usize, String> {
    Ok(engine(&state).await?.pending_group_joins())
}

// ---- link previews ----

#[tauri::command]
pub async fn link_previews(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(engine(&state).await?.link_previews())
}

#[tauri::command]
pub async fn set_link_previews(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    engine(&state).await?.set_link_previews(on).map_err(err)
}

#[tauri::command]
pub async fn link_preview_prompted(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(engine(&state).await?.link_preview_prompted())
}

#[tauri::command]
pub async fn mark_link_preview_prompted(state: State<'_, AppState>) -> Result<(), String> {
    engine(&state).await?.mark_link_preview_prompted().map_err(err)
}
