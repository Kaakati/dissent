//! Server Settings: the owner renames a server, and owners and admins set its banner and profile.
//! An uploaded banner arrives as base64, a 16:9 WebP or PNG already cropped to at most 960 by 540 px,
//! like the icon in `avatar_cmds`. A preset banner is a small id the UI draws.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use dissent_core::{CommunityId, CropRect, Trait};
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

#[tauri::command]
pub async fn rename_community(state: State<'_, AppState>, community_id: String, name: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.rename_community(community, name).await.map_err(err)
}

#[tauri::command]
pub async fn set_community_banner(state: State<'_, AppState>, community_id: String, bytes: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let bytes = STANDARD.decode(&bytes).map_err(err)?;
    engine(&state).await?.set_community_banner(community, bytes).await.map_err(err)
}

#[tauri::command]
pub async fn clear_community_banner(state: State<'_, AppState>, community_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.clear_community_banner(community).await.map_err(err)
}

#[tauri::command]
pub async fn set_community_banner_preset(state: State<'_, AppState>, community_id: String, preset: u8) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_community_banner_preset(community, preset).await.map_err(err)
}

/// `traits` is a list of `{ emoji, label }`.
#[tauri::command]
pub async fn set_community_profile(
    state: State<'_, AppState>,
    community_id: String,
    description: String,
    traits: Vec<Trait>,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_community_profile(community, description, traits).await.map_err(err)
}

/// A prepared animated banner, as base64: the looping GIF and its still poster frame.
#[derive(serde::Serialize)]
pub struct PreparedBanner {
    gif: String,
    poster: String,
}

/// Re-encodes an animated GIF for a banner: the original bytes and the crop in source pixels. Runs off
/// the UI's thread; the editor shows a spinner until it answers.
#[tauri::command]
pub async fn prepare_animated_banner(state: State<'_, AppState>, bytes: String, crop: CropRect) -> Result<PreparedBanner, String> {
    let bytes = STANDARD.decode(&bytes).map_err(err)?;
    let out = engine(&state).await?.prepare_animated_banner(bytes, crop).await.map_err(err)?;
    Ok(PreparedBanner {
        gif: STANDARD.encode(out.gif),
        poster: STANDARD.encode(out.poster),
    })
}

#[tauri::command]
pub async fn set_community_banner_animated(state: State<'_, AppState>, community_id: String, gif: String, poster: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let (gif, poster) = (STANDARD.decode(&gif).map_err(err)?, STANDARD.decode(&poster).map_err(err)?);
    engine(&state).await?.set_community_banner_animated(community, gif, poster).await.map_err(err)
}

#[tauri::command]
pub async fn set_profile_banner_animated(state: State<'_, AppState>, gif: String, poster: String) -> Result<(), String> {
    let (gif, poster) = (STANDARD.decode(&gif).map_err(err)?, STANDARD.decode(&poster).map_err(err)?);
    engine(&state).await?.set_profile_banner_animated(gif, poster).await.map_err(err)
}

/// The relays this person hosts for a server, for its boost. An empty list withdraws.
#[tauri::command]
pub async fn set_hosted_relays(state: State<'_, AppState>, community_id: String, relays: Vec<String>) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_hosted_relays(community, relays).await.map_err(err)
}

/// Lets members, and not only admins and the owner, publish relays they host. Owners and admins only.
#[tauri::command]
pub async fn set_members_host_relays(state: State<'_, AppState>, community_id: String, on: bool) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_members_host_relays(community, on).await.map_err(err)
}

/// Asks a member of the server to run the relay check on `url` from their own network. It is feedback
/// for the host only: the answer is the relay's latency in milliseconds when it passed, or the reason
/// it did not. A relay counts toward a level only after the app's own check passes, never from this.
#[tauri::command]
pub async fn check_relay(state: State<'_, AppState>, community_id: String, url: String) -> Result<u64, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.relay_check(community, &url).await
}
