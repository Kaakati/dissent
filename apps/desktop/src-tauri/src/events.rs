//! Forwards engine notifications to the window as the `dissent://event` Tauri event.

use dissent_core::EngineEvent;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;
use tokio::sync::broadcast;
use tracing::warn;

pub const EVENT: &str = "dissent://event";

/// The payload the UI listens for. Most events serialise as they are. Conversation and contact
/// changes are translated to the UI's `group`, `dm` and `contacts` kinds.
async fn ui_payload(app: &AppHandle, event: &EngineEvent) -> Value {
    match event {
        EngineEvent::ConversationChanged { id } => {
            let engine = app.state::<AppState>().engine.read().await.clone();
            let summary = engine
                .and_then(|e| e.conversations().into_iter().find(|c| &c.id == id));
            match summary {
                Some(c) if c.kind == "dm" => json!({ "kind": "dm", "peer": c.peer }),
                _ => json!({ "kind": "group", "group_id": id }),
            }
        }
        EngineEvent::ContactRequest { .. } | EngineEvent::ContactsChanged => json!({ "kind": "contacts" }),
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    }
}

/// Runs until the engine drops its sender. A lagging receiver asks the UI to refresh the community list.
pub fn forward_events(app: AppHandle, mut rx: broadcast::Receiver<EngineEvent>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let call_changed = matches!(event, EngineEvent::CallUpdated);
                    let payload = ui_payload(&app, &event).await;
                    if let Err(e) = app.emit(EVENT, payload) {
                        warn!(error = %e, "could not emit engine event");
                    }
                    if call_changed {
                        crate::part6::emit_call_state(&app).await;
                        crate::voice::sync(&app).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "engine event receiver lagged");
                    let _ = app.emit(EVENT, EngineEvent::CommunitiesChanged);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
