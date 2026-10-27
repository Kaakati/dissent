//! Paged reads of a conversation's messages, for scrolling back through its history and for live
//! updates that read only the newest page. The page is the `limit` messages before `before_id`,
//! oldest first. Names and argument keys match `ui-next/src/lib/api.ts`.

use dissent_core::{CommunityId, ChannelId, DmItem, MessageItem, RootId};
use dissent_core::{ConvId};
use tauri::State;

use crate::commands::{engine, parse, AppState};

#[tauri::command]
pub async fn channel_messages_before(
    state: State<'_, AppState>,
    community_id: String,
    channel_id: String,
    before_id: Option<String>,
    limit: usize,
) -> Result<Vec<MessageItem>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let channel: ChannelId = parse("channel id", &channel_id)?;
    Ok(engine(&state).await?.channel_messages_before(community, channel, before_id.as_deref(), limit))
}

#[tauri::command]
pub async fn dm_messages_before(
    state: State<'_, AppState>,
    peer: String,
    before_id: Option<String>,
    limit: usize,
) -> Result<Vec<DmItem>, String> {
    let peer: RootId = parse("peer id", &peer)?;
    Ok(engine(&state).await?.dm_messages_before(peer, before_id.as_deref(), limit))
}

#[tauri::command]
pub async fn group_messages_before(
    state: State<'_, AppState>,
    group_id: String,
    before_id: Option<String>,
    limit: usize,
) -> Result<Vec<DmItem>, String> {
    let conv: ConvId = parse("group id", &group_id)?;
    Ok(engine(&state).await?.group_messages_before(conv, before_id.as_deref(), limit))
}
