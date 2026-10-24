//! Tauri commands exposed to the static frontend. Each one forwards to a `dissent_core::Engine`
//! method of the same name. Errors become `String` because Tauri hands them to JavaScript as text.

use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use dissent_core::{
    ChannelId, ChannelInfo, CommunityId, CommunitySummary, CommunityView, ConversationSummary,
    Engine, EventId, Identity, MessageItem, MeView, ModAction, NetStatusView, RootId, UserView, DmItem,
};
use dissent_identity::{keystore, mnemonic, DeviceKey};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tracing::warn;

use crate::events::forward_events;

/// Shared state. `engine` is `None` only while an identity import is restarting it.
pub struct AppState {
    pub data_dir: PathBuf,
    pub identity: Mutex<Identity>,
    pub engine: tokio::sync::RwLock<Option<Engine>>,
}

impl AppState {
    pub fn identity_path(&self) -> PathBuf {
        self.data_dir.join("identity.bin")
    }
}

/// Public view of the local identity. Never carries secret material.
#[derive(Serialize)]
pub struct IdentityView {
    pub root_id: String,
    pub device_id: String,
    pub suffix: String,
    pub has_root: bool,
}

fn identity_view(me: &MeView, has_root: bool) -> IdentityView {
    IdentityView {
        root_id: me.root_id.clone(),
        device_id: me.device_id.clone(),
        suffix: me.suffix.clone(),
        has_root,
    }
}

fn err<E: Display>(e: E) -> String {
    e.to_string()
}

fn parse<T: FromStr>(what: &str, s: &str) -> Result<T, String>
where
    T::Err: Display,
{
    s.parse().map_err(|e| format!("invalid {what}: {e}"))
}

fn parse_action(s: &str) -> Result<ModAction, String> {
    Ok(match s {
        "ban" => ModAction::Ban,
        "unban" => ModAction::Unban,
        "grant_admin" => ModAction::GrantAdmin,
        "revoke_admin" => ModAction::RevokeAdmin,
        "grant_moderator" => ModAction::GrantModerator,
        "revoke_moderator" => ModAction::RevokeModerator,
        other => return Err(format!("unknown moderation action: {other}")),
    })
}

pub fn now_ms() -> Result<u64, String> {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?;
    u64::try_from(d.as_millis()).map_err(err)
}

/// Clones the running engine out of state so no lock is held across an await.
async fn engine(state: &AppState) -> Result<Engine, String> {
    state
        .engine
        .read()
        .await
        .clone()
        .ok_or_else(|| "the engine is restarting, try again".to_string())
}

const DEFAULT_LIMIT: usize = 100;

// ---- identity ----

#[tauri::command]
pub async fn identity_status(state: State<'_, AppState>) -> Result<IdentityView, String> {
    let has_root = state.identity.lock().map_err(err)?.root.is_some();
    let me = engine(&state).await?.me();
    Ok(identity_view(&me, has_root))
}

#[tauri::command]
pub fn identity_export_phrase(state: State<'_, AppState>) -> Result<String, String> {
    let id = state.identity.lock().map_err(err)?;
    let root = id.root.as_ref().ok_or("this device holds no root key, so it cannot export a phrase")?;
    Ok(mnemonic::export(root))
}

/// Replaces the local identity: the imported root certifies a fresh device key, and the engine restarts on it.
#[tauri::command]
pub async fn identity_import_phrase(
    app: AppHandle,
    state: State<'_, AppState>,
    phrase: String,
) -> Result<IdentityView, String> {
    let root = mnemonic::import(&phrase).map_err(err)?;
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), now_ms()?);
    let identity = Identity { root: Some(root), device, cert };
    keystore::save(&identity, &state.identity_path()).map_err(err)?;

    restart_engine(&app, &state, identity).await?;
    let me = engine(&state).await?.me();
    Ok(identity_view(&me, true))
}

/// Stops the running engine and starts one on `identity`, with the saved network settings.
/// The identity's data folder is reused, so an earlier identity's data comes back when it is restored.
pub async fn restart_engine(app: &AppHandle, state: &AppState, identity: Identity) -> Result<(), String> {
    let old = state.engine.write().await.take();
    if let Some(old) = old {
        if let Err(e) = old.shutdown().await {
            warn!(error = %e, "old engine did not shut down cleanly");
        }
    }
    let profile = crate::storage::profile_dir(&state.data_dir, &identity);
    std::fs::create_dir_all(&profile).map_err(err)?;
    let settings = crate::network::load(&state.data_dir);
    let (engine, rx) = Engine::start(identity.clone(), profile, crate::node_config(&settings))
        .await
        .map_err(err)?;
    *state.identity.lock().map_err(err)? = identity;
    *state.engine.write().await = Some(engine);
    forward_events(app.clone(), rx);
    let _ = app.emit(crate::events::EVENT, dissent_core::EngineEvent::CommunitiesChanged);
    Ok(())
}

// ---- network settings ----

#[tauri::command]
pub fn get_network_settings(state: State<'_, AppState>) -> crate::network::NetworkSettings {
    crate::network::load(&state.data_dir)
}

/// Saves the relay settings and restarts the engine on them. Invalid settings are rejected before anything changes.
#[tauri::command]
pub async fn set_network_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: crate::network::NetworkSettings,
) -> Result<crate::network::NetworkSettings, String> {
    let saved = crate::network::save(&state.data_dir, &settings)?;
    let identity = state.identity.lock().map_err(err)?.clone();
    restart_engine(&app, &state, identity).await?;
    Ok(saved)
}

#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---- profile and contacts ----

#[tauri::command]
pub async fn me(state: State<'_, AppState>) -> Result<MeView, String> {
    Ok(engine(&state).await?.me())
}

#[tauri::command]
pub async fn set_display_name(state: State<'_, AppState>, name: String) -> Result<(), String> {
    engine(&state).await?.set_display_name(name).await.map_err(err)
}

#[tauri::command]
pub async fn contact_link(state: State<'_, AppState>) -> Result<String, String> {
    Ok(engine(&state).await?.contact_link())
}

#[tauri::command]
pub async fn add_contact(state: State<'_, AppState>, link: String) -> Result<UserView, String> {
    engine(&state).await?.add_contact(&link).await.map_err(err)
}

#[tauri::command]
pub async fn user_profile(state: State<'_, AppState>, user: String) -> Result<UserView, String> {
    let user: RootId = parse("user id", &user)?;
    Ok(engine(&state).await?.user_profile(user))
}

// ---- servers ----

#[tauri::command]
pub async fn list_communities(state: State<'_, AppState>) -> Result<Vec<CommunitySummary>, String> {
    Ok(engine(&state).await?.communities())
}

#[tauri::command]
pub async fn create_community(state: State<'_, AppState>, name: String) -> Result<CommunitySummary, String> {
    engine(&state).await?.create_community(name).await.map_err(err)
}

#[tauri::command]
pub async fn create_invite(state: State<'_, AppState>, community_id: String) -> Result<String, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.create_invite(community).await.map_err(err)
}

#[tauri::command]
pub async fn join_community(state: State<'_, AppState>, invite: String) -> Result<CommunitySummary, String> {
    engine(&state).await?.join_community(&invite).await.map_err(err)
}

#[tauri::command]
pub async fn leave_community(state: State<'_, AppState>, community_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.leave_community(community).await.map_err(err)
}

#[tauri::command]
pub async fn community_view(state: State<'_, AppState>, community_id: String) -> Result<CommunityView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.community_view(community).map_err(err)
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, AppState>,
    community_id: String,
    name: String,
) -> Result<ChannelInfo, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.create_channel(community, name).await.map_err(err)
}

#[tauri::command]
pub async fn moderate(
    state: State<'_, AppState>,
    community_id: String,
    action: String,
    user: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let action = parse_action(&action)?;
    let user: RootId = parse("user id", &user)?;
    engine(&state).await?.moderate(community, action, user).await.map_err(err)
}

#[tauri::command]
pub async fn claim_handle(state: State<'_, AppState>, community_id: String, handle: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.claim_handle(community, handle).await.map_err(err)
}

// ---- channel messages ----

#[tauri::command]
pub async fn channel_messages(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    limit: Option<usize>,
) -> Result<Vec<MessageItem>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    Ok(engine(&state).await?.channel_messages(community, channel, limit.unwrap_or(DEFAULT_LIMIT)))
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    text: String,
) -> Result<MessageItem, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    engine(&state).await?.send_message(community, channel, text).await.map_err(err)
}

#[tauri::command]
pub async fn edit_message(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
    text: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let message: EventId = parse("message id", &message_id)?;
    engine(&state).await?.edit_message(community, channel, message, text).await.map_err(err)
}

#[tauri::command]
pub async fn delete_message(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let message: EventId = parse("message id", &message_id)?;
    engine(&state).await?.delete_message(community, channel, message).await.map_err(err)
}

// ---- direct messages ----

#[tauri::command]
pub async fn list_conversations(state: State<'_, AppState>) -> Result<Vec<ConversationSummary>, String> {
    Ok(engine(&state).await?.conversations())
}

#[tauri::command]
pub async fn dm_messages(
    state: State<'_, AppState>,
    peer: String,
    limit: Option<usize>,
) -> Result<Vec<DmItem>, String> {
    let peer: RootId = parse("peer id", &peer)?;
    Ok(engine(&state).await?.dm_messages(peer, limit.unwrap_or(DEFAULT_LIMIT)))
}

#[tauri::command]
pub async fn send_dm(state: State<'_, AppState>, peer: String, text: String) -> Result<DmItem, String> {
    let peer: RootId = parse("peer id", &peer)?;
    engine(&state).await?.send_dm(peer, text).await.map_err(err)
}

#[tauri::command]
pub async fn net_status(state: State<'_, AppState>) -> Result<NetStatusView, String> {
    Ok(engine(&state).await?.net_status().await)
}
