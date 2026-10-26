//! Tauri commands for typing indicators. Argument keys match `ui-next/src/lib/api.ts`. The engine emits
//! `TypingChanged`, and `events.rs` forwards it here as `dissent://typing-changed`.

use dissent_core::{ChannelId, CommunityId, ConvId, RootId, TypingTarget};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::warn;

use crate::commands::{engine, err, parse, AppState};

/// Who is typing changed. Payload: `{ target, names }`, with `target` in the UI's shape.
pub const TYPING_EVENT: &str = "dissent://typing-changed";

/// Where someone is typing, named the way the UI names it: a channel, a DM peer, or a group.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetArg {
    Channel {
        #[serde(rename = "communityId")]
        community_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    Dm {
        peer: String,
    },
    Group {
        #[serde(rename = "groupId")]
        group_id: String,
    },
}

impl TargetArg {
    fn target(&self) -> Result<TypingTarget, String> {
        Ok(match self {
            TargetArg::Channel {
                community_id,
                channel_id,
            } => TypingTarget::Channel {
                community: parse::<CommunityId>("community id", community_id)?,
                channel: parse::<ChannelId>("channel id", channel_id)?,
            },
            TargetArg::Dm { peer } => TypingTarget::Dm(parse::<RootId>("peer id", peer)?),
            TargetArg::Group { group_id } => TypingTarget::Group(parse::<ConvId>("group id", group_id)?),
        })
    }
}

/// The UI's shape of `target`, the same keys `TargetArg` takes.
fn target_json(target: &TypingTarget) -> Value {
    match target {
        TypingTarget::Channel { community, channel } => json!({
            "kind": "channel",
            "communityId": community.to_string(),
            "channelId": channel.to_string(),
        }),
        TypingTarget::Dm(peer) => json!({ "kind": "dm", "peer": peer.to_string() }),
        TypingTarget::Group(conv) => json!({ "kind": "group", "groupId": conv.to_string() }),
    }
}

/// Says we are typing in `target`. The UI calls it on keystrokes. The engine throttles it.
#[tauri::command]
pub async fn typing(state: State<'_, AppState>, target: TargetArg) -> Result<(), String> {
    engine(&state).await?.typing(target.target()?).map_err(err)
}

/// Says we stopped typing in `target`. The UI calls it on send and when the composer is cleared.
#[tauri::command]
pub async fn stop_typing(state: State<'_, AppState>, target: TargetArg) -> Result<(), String> {
    engine(&state).await?.stop_typing(target.target()?).map_err(err)
}

/// The display names of the people typing in `target` now, oldest first.
#[tauri::command]
pub async fn typing_in(state: State<'_, AppState>, target: TargetArg) -> Result<Vec<String>, String> {
    Ok(engine(&state).await?.typing_in(target.target()?))
}

/// Tells the window who is typing in `target` now.
pub async fn emit_typing(app: &AppHandle, target: &TypingTarget) {
    let Some(engine) = app.state::<AppState>().engine.read().await.clone() else {
        return;
    };
    let payload = json!({ "target": target_json(target), "names": engine.typing_in(*target) });
    if let Err(e) = app.emit(TYPING_EVENT, payload) {
        warn!(error = %e, "could not emit typing change");
    }
}
