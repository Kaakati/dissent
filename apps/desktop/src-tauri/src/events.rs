//! Forwards engine notifications to the window as the `dissent://event` Tauri event.

use dissent_core::EngineEvent;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;
use tokio::sync::broadcast;
use tracing::warn;

pub const EVENT: &str = "dissent://event";
/// Our own status changed. Payload: `{ status, until_ms }`, as `get_status` returns it.
pub const STATUS_EVENT: &str = "dissent://status-changed";
/// A call is ringing on this device. Payload: the dialog's shape, see `part6::ringing_payload`.
pub const RINGING_EVENT: &str = "dissent://call-ringing";

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

/// Tells the UI that a call is ringing here, with the dialog's payload.
async fn emit_ringing(app: &AppHandle, call_id: &str, from: &str, conv: Option<&str>) {
    let Some(engine) = app.state::<AppState>().engine.read().await.clone() else {
        return;
    };
    let payload = crate::part6::ringing_payload(&engine, call_id, from, conv);
    if let Err(e) = app.emit(RINGING_EVENT, payload) {
        warn!(error = %e, "could not emit incoming call");
    }
}

/// Runs until the engine drops its sender. A lagging receiver asks the UI to refresh the community list.
pub fn forward_events(app: AppHandle, mut rx: broadcast::Receiver<EngineEvent>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(EngineEvent::StatusChanged { status, until_ms }) => {
                    let payload = json!({ "status": status, "until_ms": until_ms });
                    if let Err(e) = app.emit(STATUS_EVENT, payload) {
                        warn!(error = %e, "could not emit status change");
                    }
                }
                Ok(event) => {
                    if let EngineEvent::IncomingCall { call_id, from, conv, .. } = &event {
                        emit_ringing(&app, call_id, from, conv.as_deref()).await;
                    }
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
