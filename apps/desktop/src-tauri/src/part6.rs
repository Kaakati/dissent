//! Tauri commands for the engine surfaces in `docs/design/07-visual-system.md` part 6: calls,
//! guardians, devices, safety numbers, notes and pins, and first run. Each command forwards to an
//! engine method. Names and argument names are the ones `ui/api.js` calls. The UI shows a surface
//! only when `capabilities` lists it.

use std::fmt::Display;
use std::str::FromStr;
use std::sync::Mutex;

use dissent_core::{
    CallView, ChannelId, ChannelInfo, ChannelPatch, ChannelVisibility, CommunityId, ConvId,
    DeviceView, Engine, EventId, GuardianView, Identity, InviteView, MessageItem, NoteKind,
    PendingDevice, RootId,
};
use dissent_identity::{keystore, DeviceId};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{restart_engine, AppState};

/// The new device's link request, between `link_device_code` and `finish_device_link`. Holds the
/// new device's secret key, so it lives only in memory.
#[derive(Default)]
pub struct PendingLink(pub Mutex<Option<PendingDevice>>);

/// Name shown for a device linked from this one, until the user renames it.
fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|n| n.trim().chars().take(32).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "New device".to_string())
}

fn err<E: Display>(e: E) -> String {
    e.to_string()
}

fn parse<T: FromStr>(what: &str, s: &str) -> Result<T, String>
where
    T::Err: Display,
{
    s.parse::<T>().map_err(|e| format!("bad {what}: {e}"))
}

async fn engine(state: &AppState) -> Result<Engine, String> {
    state
        .engine
        .read()
        .await
        .clone()
        .ok_or_else(|| "the engine is restarting, try again".to_string())
}

/// Sends the call state, or `null`, on the `dissent://call-state` event.
pub async fn emit_call_state(app: &AppHandle) {
    let state = app.state::<AppState>();
    let view: Option<CallView> = match state.engine.read().await.as_ref() {
        Some(engine) => engine.call_state(),
        None => None,
    };
    if let Err(e) = app.emit("dissent://call-state", view) {
        tracing::warn!(error = %e, "could not emit call state");
    }
}

// ---- capabilities ----

#[tauri::command]
pub fn capabilities() -> Vec<&'static str> {
    dissent_core::CAPABILITIES.to_vec()
}

// ---- notes, pins, read markers, safety ----

#[tauri::command]
pub async fn set_note(
    state: State<'_, AppState>,
    user: String,
    kind: String,
    text: String,
) -> Result<(), String> {
    let kind = match kind.as_str() {
        "note" => NoteKind::Note,
        "nickname" => NoteKind::Nickname,
        other => return Err(format!("unknown note kind {other}")),
    };
    let user: RootId = parse("user id", &user)?;
    engine(&state).await?.set_note(user, kind, text).map_err(err)
}

#[tauri::command]
pub async fn pin_conversation(state: State<'_, AppState>, key: String, pinned: bool) -> Result<(), String> {
    engine(&state).await?.pin_conversation(key, pinned).map_err(err)
}

#[tauri::command]
pub async fn mark_read(state: State<'_, AppState>, key: String) -> Result<(), String> {
    engine(&state).await?.mark_read(key).map_err(err)
}

#[tauri::command]
pub async fn pinned_messages(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
) -> Result<Vec<MessageItem>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    Ok(engine(&state).await?.pinned_messages(community, channel))
}

/// Pins or unpins one message. Pins stay on this device.
#[tauri::command]
pub async fn pin_message(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    message_id: String,
    pinned: bool,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let message: EventId = parse("message id", &message_id)?;
    engine(&state)
        .await?
        .pin_message(community, channel, message, pinned)
        .map_err(err)
}

#[tauri::command]
pub async fn mark_safety_verified(state: State<'_, AppState>, user: String) -> Result<(), String> {
    let user: RootId = parse("user id", &user)?;
    engine(&state).await?.mark_safety_verified(user).map_err(err)
}

// ---- calls ----

#[tauri::command]
pub async fn start_call(state: State<'_, AppState>, peer: String) -> Result<Option<CallView>, String> {
    let peer: RootId = parse("peer", &peer)?;
    let engine = engine(&state).await?;
    engine.call_dm(peer).await.map_err(err)?;
    Ok(engine.call_state())
}

#[tauri::command]
pub async fn join_call(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
) -> Result<Option<CallView>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let engine = engine(&state).await?;
    engine.join_call(community, channel).await.map_err(err)?;
    Ok(engine.call_state())
}

#[tauri::command]
pub async fn leave_call(state: State<'_, AppState>) -> Result<(), String> {
    engine(&state).await?.leave_call().await.map_err(err)
}

/// Makes a voice channel in a community. Joining it is a separate step.
#[tauri::command]
pub async fn create_voice_channel(
    state: State<'_, AppState>,
    community_id: String,
    name: String,
) -> Result<ChannelInfo, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.create_voice_channel(community, name).await.map_err(err)
}

/// Changes a channel. Fields left out stay as they are. `visibility` is `default`, `spoiler` or
/// `age`, and `relay` is a relay URL, or `automatic` for none.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_channel(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    name: Option<String>,
    slowmode_s: Option<u32>,
    visibility: Option<String>,
    bitrate_kbps: Option<u32>,
    user_limit: Option<u32>,
    relay: Option<String>,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    let visibility = match visibility.as_deref() {
        None => None,
        Some("default") => Some(ChannelVisibility::Default),
        Some("spoiler") => Some(ChannelVisibility::Spoiler),
        Some("age") => Some(ChannelVisibility::AgeRestricted),
        Some(other) => return Err(format!("unknown visibility {other}")),
    };
    let relay = relay.map(|r| if r == "automatic" { String::new() } else { r });
    let patch = ChannelPatch {
        name,
        slowmode_s,
        visibility,
        bitrate_kbps,
        user_limit,
        relay,
    };
    engine(&state)
        .await?
        .update_channel(community, channel, patch)
        .await
        .map_err(err)
}

/// Deletes a channel and its messages for everyone.
#[tauri::command]
pub async fn delete_channel(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    engine(&state).await?.delete_channel(community, channel).await.map_err(err)
}

/// The invites this device made for a community.
#[tauri::command]
pub async fn list_invites(
    state: State<'_, AppState>,
    community_id: String,
) -> Result<Vec<InviteView>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.list_invites(community).map_err(err)
}

/// Stops new joins through an invite. `invite_id` is the id that `list_invites` returned.
#[tauri::command]
pub async fn revoke_invite(
    state: State<'_, AppState>,
    community_id: String,
    invite_id: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let invite = dissent_core::parse_invite_id(&invite_id).map_err(err)?;
    engine(&state).await?.revoke_invite(community, invite).await.map_err(err)
}

#[tauri::command]
pub async fn call_state(state: State<'_, AppState>) -> Result<Option<CallView>, String> {
    Ok(engine(&state).await?.call_state())
}

#[tauri::command]
pub async fn set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    engine(&state).await?.set_mute(muted).await.map_err(err)
}

#[tauri::command]
pub async fn set_deafened(state: State<'_, AppState>, deafened: bool) -> Result<(), String> {
    engine(&state).await?.set_deafened(deafened).map_err(err)
}

#[tauri::command]
pub async fn set_camera(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    engine(&state).await?.set_camera(on).map_err(err)
}

#[tauri::command]
pub async fn send_call_message(state: State<'_, AppState>, text: String) -> Result<(), String> {
    engine(&state).await?.send_call_message(text).await.map_err(err)
}

// ---- guardians ----

#[tauri::command]
pub async fn list_guardians(state: State<'_, AppState>) -> Result<GuardianView, String> {
    engine(&state).await?.guardians().map_err(err)
}

#[tauri::command]
pub async fn sign_guardians(
    state: State<'_, AppState>,
    guardians: Vec<String>,
    threshold: u8,
) -> Result<GuardianView, String> {
    let roots = guardians
        .iter()
        .map(|g| parse::<RootId>("guardian id", g))
        .collect::<Result<Vec<_>, _>>()?;
    engine(&state).await?.set_guardians(roots, threshold).await.map_err(err)
}

// ---- devices ----

#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceView>, String> {
    engine(&state).await?.devices().map_err(err)
}

#[tauri::command]
pub async fn revoke_device(state: State<'_, AppState>, device_id: String) -> Result<Vec<DeviceView>, String> {
    let device: DeviceId = parse("device id", &device_id)?;
    engine(&state).await?.revoke_device(device).await.map_err(err)
}

/// On a new device: starts linking and returns the request link to show or paste on a device
/// that holds the root key.
#[tauri::command]
pub fn link_device_code(pending: State<'_, PendingLink>) -> Result<String, String> {
    let request = dissent_core::start_device_link(&default_device_name()).map_err(err)?;
    let link = request.link().map_err(err)?;
    *pending.0.lock().map_err(err)? = Some(request);
    Ok(link)
}

/// On a device that holds the root key: certifies the requesting device and returns the grant
/// link to send back to it.
#[tauri::command]
pub async fn approve_device(state: State<'_, AppState>, link: String) -> Result<String, String> {
    engine(&state).await?.approve_device(&link).map_err(err)
}

/// On the new device: takes the grant, saves the linked identity, and starts the engine on it.
#[tauri::command]
pub async fn finish_device_link(
    app: AppHandle,
    state: State<'_, AppState>,
    pending: State<'_, PendingLink>,
    grant: String,
) -> Result<(), String> {
    let request = pending
        .0
        .lock()
        .map_err(err)?
        .take()
        .ok_or("no device link is in progress")?;
    let identity: Identity = request.finish(&grant).map_err(err)?;
    keystore::save(&identity, &state.identity_path(), &state.vault).map_err(err)?;
    restart_engine(&app, &state, identity).await
}

/// Accepts a ringing direct call. Media starts only now.
#[tauri::command]
pub async fn accept_call(state: State<'_, AppState>, call_id: String) -> Result<(), String> {
    engine(&state).await?.accept_call(call_id).await.map_err(err)
}

/// Joins a group call that is already live. The call is linked to every member in it.
#[tauri::command]
pub async fn join_group_call(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<CallView>, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    let engine = engine(&state).await?;
    engine.join_group_call(conv).await.map_err(err)?;
    Ok(engine.call_state())
}

/// Joins the direct call with a contact, or calls them when none is live. A live call is joined
/// the same way: the device that stayed in it answers with a fresh key.
#[tauri::command]
pub async fn join_dm_call(state: State<'_, AppState>, peer: String) -> Result<Option<CallView>, String> {
    let peer: RootId = parse("peer", &peer)?;
    let engine = engine(&state).await?;
    engine.call_dm(peer).await.map_err(err)?;
    Ok(engine.call_state())
}

/// The payload of `dissent://call-ringing`, from the engine's `IncomingCall` event. The names,
/// the verification chip and who is already in the call come from the engine's own views.
pub(crate) fn ringing_payload(engine: &Engine, call_id: &str, from: &str, conv: Option<&str>) -> Value {
    // Verified means the safety number was compared, and the key has not changed since.
    let (caller_name, verified) = RootId::from_str(from)
        .map(|root| {
            let user = engine.user_profile(root);
            let verified = user.verified_ms.is_some() && user.key_changed_ms.is_none();
            (user.display_name, verified)
        })
        .unwrap_or_default();
    let caller = json!({ "root_id": from, "display_name": caller_name });
    match conv {
        Some(group_id) => {
            let summary = engine.conversations().into_iter().find(|c| c.id == group_id);
            let name = summary.as_ref().map(|c| c.name.clone()).unwrap_or_default();
            let total = summary.as_ref().map_or(0, |c| c.members.len());
            let mut members_in: Vec<Value> = summary
                .and_then(|c| c.live_call)
                .map(|live| live.participants)
                .unwrap_or_default()
                .into_iter()
                .map(|p| json!({ "root_id": p.root_id, "display_name": p.display_name }))
                .collect();
            // The caller is in the call it started, even before its roster has reached this device.
            if !members_in.iter().any(|m| m["root_id"].as_str() == Some(from)) {
                members_in.insert(0, caller.clone());
            }
            json!({
                "call_id": call_id,
                "scope": "group",
                "group_id": group_id,
                "name": name,
                "caller": caller,
                "verified": verified,
                "in_call": members_in.len(),
                "total": total,
                "members_in": members_in,
            })
        }
        None => json!({
            "call_id": call_id,
            "scope": "dm",
            "peer": from,
            "name": caller_name,
            "caller": caller,
            "verified": verified,
            "in_call": 1,
            "total": 2,
            "members_in": [],
        }),
    }
}

/// Declines a ringing direct call. The caller is told, and nothing connects.
#[tauri::command]
pub async fn decline_call(state: State<'_, AppState>, call_id: String) -> Result<(), String> {
    engine(&state).await?.decline_call(call_id).await.map_err(err)
}

/// Ends first run. Called only after the user confirms they wrote the recovery phrase down.
#[tauri::command]
pub async fn complete_first_run(state: State<'_, AppState>) -> Result<(), String> {
    engine(&state).await?.complete_first_run().map_err(err)
}

/// Each conversation key with the time it was last marked read, in milliseconds.
#[tauri::command]
pub async fn read_markers(
    state: State<'_, AppState>,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    Ok(engine(&state).await?.read_markers())
}

/// Hides a conversation from the sidebar. It comes back when a new message arrives.
#[tauri::command]
pub async fn close_conversation(state: State<'_, AppState>, key: String) -> Result<(), String> {
    engine(&state).await?.close_conversation(key).map_err(err)
}

/// Shows a closed conversation again.
#[tauri::command]
pub async fn reopen_conversation(state: State<'_, AppState>, key: String) -> Result<(), String> {
    engine(&state).await?.reopen_conversation(key).map_err(err)
}

/// Closed conversations, each with the time it was closed, in milliseconds.
#[tauri::command]
pub async fn closed_conversations(
    state: State<'_, AppState>,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    Ok(engine(&state).await?.closed_conversations())
}

/// Ignores a person. Their messages still arrive, but collapsed and without notifications.
#[tauri::command]
pub async fn ignore_user(state: State<'_, AppState>, user: String) -> Result<(), String> {
    let user: RootId = parse("user id", &user)?;
    engine(&state).await?.ignore_user(user).map_err(err)
}

/// Stops ignoring a person.
#[tauri::command]
pub async fn unignore_user(state: State<'_, AppState>, user: String) -> Result<(), String> {
    let user: RootId = parse("user id", &user)?;
    engine(&state).await?.unignore_user(user).map_err(err)
}

/// Everyone this device ignores, as base32 root ids.
#[tauri::command]
pub async fn ignored_users(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(engine(&state)
        .await?
        .ignored_users()
        .iter()
        .map(|u| u.to_string())
        .collect())
}
