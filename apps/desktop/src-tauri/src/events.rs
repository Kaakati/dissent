//! Forwards engine notifications to the window as the `dissent://event` Tauri event.

use dissent_core::EngineEvent;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tracing::warn;

pub const EVENT: &str = "dissent://event";

/// Runs until the engine drops its sender. A lagging receiver asks the UI to refresh the community list.
pub fn forward_events(app: AppHandle, mut rx: broadcast::Receiver<EngineEvent>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = app.emit(EVENT, event) {
                        warn!(error = %e, "could not emit engine event");
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
