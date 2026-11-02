//! Custom roles and bots in Server Settings. See `dissent_core::custom_roles`.

use dissent_core::custom_roles::{role_spec, BotToken, RolesView};
use dissent_core::{CommunityId, RootId};
use dissent_events::RoleId;
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

#[tauri::command]
pub async fn roles_view(state: State<'_, AppState>, community_id: String) -> Result<RolesView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.roles_view(community).map_err(err)
}

/// `color` is `#rrggbb` or empty; `perms` are names from `RolesView::all_perms`. Returns the new role's id.
#[tauri::command]
pub async fn create_role(
    state: State<'_, AppState>,
    community_id: String,
    name: String,
    color: String,
    perms: Vec<String>,
    position: u16,
) -> Result<String, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let spec = role_spec(&name, &color, &perms, position).map_err(err)?;
    let role = engine(&state).await?.create_role(community, spec).await.map_err(err)?;
    Ok(role.to_string())
}

#[tauri::command]
pub async fn update_role(
    state: State<'_, AppState>,
    community_id: String,
    role_id: String,
    name: String,
    color: String,
    perms: Vec<String>,
    position: u16,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let role: RoleId = parse("role id", &role_id)?;
    let spec = role_spec(&name, &color, &perms, position).map_err(err)?;
    engine(&state).await?.update_role(community, role, spec).await.map_err(err)
}

#[tauri::command]
pub async fn delete_role(state: State<'_, AppState>, community_id: String, role_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let role: RoleId = parse("role id", &role_id)?;
    engine(&state).await?.delete_role(community, role).await.map_err(err)
}

#[tauri::command]
pub async fn assign_role(
    state: State<'_, AppState>,
    community_id: String,
    user_id: String,
    role_id: String,
    on: bool,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let user: RootId = parse("user id", &user_id)?;
    let role: RoleId = parse("role id", &role_id)?;
    engine(&state).await?.assign_role(community, user, role, on).await.map_err(err)
}

/// Returns the bot's token. It holds the bot's key: the UI shows it once and never stores it.
#[tauri::command]
pub async fn create_bot(
    state: State<'_, AppState>,
    community_id: String,
    name: String,
    role_ids: Vec<String>,
) -> Result<BotToken, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let roles = role_ids.iter().map(|r| parse("role id", r)).collect::<Result<Vec<RoleId>, String>>()?;
    engine(&state).await?.create_bot(community, name, roles).await.map_err(err)
}

#[tauri::command]
pub async fn unregister_bot(state: State<'_, AppState>, community_id: String, user_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let user: RootId = parse("user id", &user_id)?;
    let engine = engine(&state).await?;
    let name = engine
        .roles_view(community)
        .map_err(err)?
        .bots
        .into_iter()
        .find(|b| b.root_id == user_id)
        .map(|b| b.name)
        .unwrap_or_else(|| "bot".into());
    engine.register_bot(community, user, name, false).await.map_err(err)
}

#[tauri::command]
pub async fn set_custom_role_perms(state: State<'_, AppState>, community_id: String, on: bool, confirmed: Option<Vec<String>>) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let confirmed = crate::commands::confirmed_devices(confirmed)?;
    engine(&state).await?.set_custom_role_perms_confirmed(community, on, &confirmed).await.map_err(err)
}

/// Which member devices could not follow custom roles' permissions, for the owner to decide.
#[tauri::command]
pub async fn custom_role_perms_status(state: State<'_, AppState>, community_id: String) -> Result<dissent_core::custom_roles::GateStatus, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    Ok(engine(&state).await?.custom_role_perms_status(community).await)
}
