//! Tauri commands exposed to the static frontend. Each one forwards to a `dissent_core::Engine`
//! method of the same name. Errors become `String` because Tauri hands them to JavaScript as text.

use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use dissent_core::{
    ChannelAccessView, ChannelId, ChannelInfo, CommunityId, CommunitySummary, InvitePreviewView, Role,
    Engine, EventId, Identity, MessageItem, MeView, Mentions, ModAction, NetStatusView, RootId, UserView, DmItem,
};
use dissent_identity::{keystore, mnemonic, DeviceKey, Vault};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::warn;

use crate::avatar_cmds::for_ui;
use crate::events::forward_events;

/// Shared state. `engine` is `None` only while an identity import is restarting it.
pub struct AppState {
    pub data_dir: PathBuf,
    /// Encrypts identity.bin at rest. The key comes from the OS credential store.
    pub vault: std::sync::Arc<Vault>,
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

pub(crate) fn err<E: Display>(e: E) -> String {
    e.to_string()
}

pub(crate) fn parse<T: FromStr>(what: &str, s: &str) -> Result<T, String>
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
pub(crate) async fn engine(state: &AppState) -> Result<Engine, String> {
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
    let identity = Identity { root: Some(root), device, cert, device_name: String::new() };
    keystore::save(&identity, &state.identity_path(), &state.vault).map_err(err)?;

    restart_engine(&app, &state, identity).await?;
    let me = engine(&state).await?.me();
    Ok(identity_view(&me, true))
}

/// Stops the running engine and starts one on `identity`, with the saved network settings.
/// The identity's data folder is reused, so an earlier identity's data comes back when it is restored.
pub async fn restart_engine(app: &AppHandle, state: &AppState, identity: Identity) -> Result<(), String> {
    crate::voice::stop(app).await;
    let old = state.engine.write().await.take();
    if let Some(old) = old {
        if let Err(e) = old.shutdown().await {
            warn!(error = %e, "old engine did not shut down cleanly");
        }
    }
    let profile = crate::storage::profile_dir(&state.data_dir, &identity);
    std::fs::create_dir_all(&profile).map_err(err)?;
    let settings = crate::network::load(&state.data_dir);
    let (engine, rx) = Engine::start_with_options(
        identity.clone(),
        profile,
        crate::node_config(&settings),
        state.vault.clone(),
        dissent_core::Timings::default(),
        crate::attachments::default_download_dir(app),
    )
    .await
    .map_err(err)?;
    crate::attachments::prepare_engine(app, &engine);
    crate::fake_capture(&engine);
    crate::voice::prepare(&engine, &app.state::<crate::voice::Voice>());
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

/// The relays in Settings: the home relay, and each relay with the latency the node measured. Reads the
/// settings file and the running node. When the node is not running, nothing is measured yet.
#[tauri::command]
pub async fn relay_status(state: State<'_, AppState>) -> Result<crate::network::RelayStatus, String> {
    let settings = crate::network::load(&state.data_dir);
    let running = state.engine.read().await.clone();
    let (community, hosted, report) = match running {
        Some(engine) => (engine.joined_community_relays(), engine.hosted_relays(), engine.relay_report()),
        None => (Vec::new(), Vec::new(), dissent_core::RelayReport::default()),
    };
    Ok(crate::network::status(&settings, &community, &hosted, &report))
}

#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---- profile and contacts ----

#[tauri::command]
pub async fn me(state: State<'_, AppState>) -> Result<Value, String> {
    for_ui(&engine(&state).await?.me())
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
pub async fn user_profile(state: State<'_, AppState>, user: String) -> Result<Value, String> {
    let user: RootId = parse("user id", &user)?;
    for_ui(&engine(&state).await?.user_profile_full(user).await)
}

// ---- servers ----

#[tauri::command]
pub async fn list_communities(state: State<'_, AppState>) -> Result<Value, String> {
    for_ui(&engine(&state).await?.communities())
}

/// Saves this device's order of the server rail. Local to the profile; nothing is sent.
#[tauri::command]
pub async fn set_server_order(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let ids = ids
        .iter()
        .map(|id| parse::<CommunityId>("community id", id))
        .collect::<Result<Vec<_>, _>>()?;
    engine(&state).await?.set_server_order(ids).map_err(err)
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

/// The long form of a community's invite, which versions before short invites can open.
#[tauri::command]
pub async fn create_full_invite(state: State<'_, AppState>, community_id: String) -> Result<String, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.create_full_invite(community).await.map_err(err)
}

#[tauri::command]
pub async fn join_community(state: State<'_, AppState>, invite: String) -> Result<CommunitySummary, String> {
    engine(&state).await?.join_community(&invite).await.map_err(err)
}

/// The card for an invite link, before joining. Takes either the long or the short form. The JSON
/// has `community`, `name`, `icon`, `member_count`, `online_count`, `created_ms`, `invite`
/// (`live`, `revoked`, `expired` or `unknown`) and `already_member`.
#[tauri::command]
pub async fn invite_preview(state: State<'_, AppState>, link: String) -> Result<InvitePreviewView, String> {
    engine(&state).await?.invite_preview(&link).await.map_err(err)
}

#[tauri::command]
pub async fn leave_community(state: State<'_, AppState>, community_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.leave_community(community).await.map_err(err)
}

#[tauri::command]
pub async fn community_view(state: State<'_, AppState>, community_id: String) -> Result<Value, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    for_ui(&engine(&state).await?.community_view(community).map_err(err)?)
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

/// Whether member-only control is on, and whether the owner may turn it on yet. See `MemberOnlyStatus`.
#[tauri::command]
pub async fn member_only_status(
    state: State<'_, AppState>,
    community_id: String,
) -> Result<dissent_core::MemberOnlyStatus, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.member_only_status(community).await.map_err(err)
}

/// Turns member-only control on or off. Owner only, and on only once every member device supports it.
#[tauri::command]
pub async fn set_member_only(
    state: State<'_, AppState>,
    community_id: String,
    on: bool,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_member_only(community, on).await.map_err(err)
}

/// Sets who may read a channel. `private` false makes it public again. Roles are `admin` or `moderator`;
/// members are root ids. Owner and admin only, as replay checks.
#[tauri::command]
pub async fn set_channel_access(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    private: bool,
    roles: Vec<String>,
    members: Vec<String>,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let (roles, members) = access_of(&roles, &members)?;
    engine(&state)
        .await?
        .set_channel_access(community, channel, private, roles, members)
        .await
        .map_err(err)
}

/// Creates a private channel in one call, so there is no public moment for it. Roles and members as
/// `set_channel_access`. Owner and admin only.
#[tauri::command]
pub async fn create_private_channel(
    state: State<'_, AppState>,
    community_id: String,
    name: String,
    roles: Vec<String>,
    members: Vec<String>,
) -> Result<ChannelInfo, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let (roles, members) = access_of(&roles, &members)?;
    engine(&state)
        .await?
        .create_private_channel(community, name, roles, members)
        .await
        .map_err(err)
}

/// Reads a channel's roles and member ids from the command arguments.
fn access_of(roles: &[String], members: &[String]) -> Result<(Vec<Role>, Vec<RootId>), String> {
    let roles = roles
        .iter()
        .map(|role| match role.as_str() {
            "admin" => Ok(Role::Admin),
            "moderator" => Ok(Role::Moderator),
            other => Err(format!("unknown role: {other}")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let members = members
        .iter()
        .map(|m| parse::<RootId>("member", m))
        .collect::<Result<Vec<_>, String>>()?;
    Ok((roles, members))
}

/// Who may read a channel now. Only its readers and the owner or admins get the list.
#[tauri::command]
pub async fn channel_access(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
) -> Result<ChannelAccessView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    engine(&state).await?.channel_access(community, channel).map_err(err)
}

/// Sets which past control content a new member is given keys for: `all` or `since_join`.
#[tauri::command]
pub async fn set_history_visibility(
    state: State<'_, AppState>,
    community_id: String,
    setting: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let setting = match setting.as_str() {
        "all" => dissent_core::HistorySetting::All,
        "since_join" => dissent_core::HistorySetting::SinceJoin,
        other => return Err(format!("unknown history setting: {other}")),
    };
    engine(&state).await?.set_history_visibility(community, setting).await.map_err(err)
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
    mentions: Option<MentionsArg>,
) -> Result<MessageItem, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let mentions = mentions_of_arg(mentions)?;
    engine(&state).await?.send_message_with(community, channel, text, mentions).await.map_err(err)
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

/// Who a message mentions, as the UI sends it: user ids in base32, and whether it mentions everyone.
#[derive(serde::Deserialize)]
pub struct MentionsArg {
    pub users: Vec<String>,
    pub everyone: bool,
}

/// The mentions a send command carries. Absent means none.
pub(crate) fn mentions_of_arg(arg: Option<MentionsArg>) -> Result<Mentions, String> {
    let Some(arg) = arg else {
        return Ok(Mentions::default());
    };
    let users = arg
        .users
        .iter()
        .map(|u| parse::<RootId>("user id", u).map(|r| r.0))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Mentions { users, everyone: arg.everyone })
}

#[tauri::command]
pub async fn send_reply(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
    text: String,
    mentions: Option<MentionsArg>,
) -> Result<MessageItem, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let target: EventId = parse("message id", &message_id)?;
    let mentions = mentions_of_arg(mentions)?;
    engine(&state).await?.send_reply_with(community, channel, target, text, mentions).await.map_err(err)
}

#[tauri::command]
pub async fn react(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let target: EventId = parse("message id", &message_id)?;
    engine(&state).await?.react(community, channel, target, emoji).await.map_err(err)
}

#[tauri::command]
pub async fn unreact(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
    emoji: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let target: EventId = parse("message id", &message_id)?;
    engine(&state).await?.unreact(community, channel, target, emoji).await.map_err(err)
}

// ---- direct messages ----

#[tauri::command]
pub async fn list_conversations(state: State<'_, AppState>) -> Result<Value, String> {
    for_ui(&engine(&state).await?.sidebar_conversations())
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
pub async fn send_dm(
    state: State<'_, AppState>,
    peer: String,
    text: String,
    mentions: Option<MentionsArg>,
) -> Result<DmItem, String> {
    let peer: RootId = parse("peer id", &peer)?;
    let mentions = mentions_of_arg(mentions)?;
    engine(&state).await?.send_dm_with(peer, text, mentions).await.map_err(err)
}

/// Sends a message that was refused, again. `message_id` is the id the chat shows.
#[tauri::command]
pub async fn retry_message(state: State<'_, AppState>, message_id: String) -> Result<(), String> {
    engine(&state).await?.retry_message(&message_id).await.map_err(err)
}

#[tauri::command]
pub async fn net_status(state: State<'_, AppState>) -> Result<NetStatusView, String> {
    Ok(engine(&state).await?.net_status().await)
}

/// Safety number for a pair of users: 12 groups of five digits, the same on both sides.
#[tauri::command]
pub async fn safety_number(state: State<'_, AppState>, user: String) -> Result<String, String> {
    let them: RootId = parse("user id", &user)?;
    let mine: RootId = parse("root id", &engine(&state).await?.me().root_id)?;
    Ok(dissent_identity::safety_number(&mine, &them))
}
