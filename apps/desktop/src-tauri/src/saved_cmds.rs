//! Tauri commands for saved messages, reminders and mark-unread. All of it is local to this device.
//! Names and argument keys match `ui-next/src/lib/api.ts`.

use dissent_core::{Bookmark, MessageRef, Reminder};
use tauri::State;

use crate::commands::{engine, err, AppState};

#[tauri::command]
pub async fn add_bookmark(state: State<'_, AppState>, message: MessageRef) -> Result<(), String> {
    engine(&state).await?.add_bookmark(message).map_err(err)
}

#[tauri::command]
pub async fn remove_bookmark(state: State<'_, AppState>, key: String) -> Result<(), String> {
    engine(&state).await?.remove_bookmark(&key).map_err(err)
}

#[tauri::command]
pub async fn list_bookmarks(state: State<'_, AppState>) -> Result<Vec<Bookmark>, String> {
    Ok(engine(&state).await?.list_bookmarks())
}

#[tauri::command]
pub async fn add_reminder(state: State<'_, AppState>, message: MessageRef, at_ms: u64) -> Result<Reminder, String> {
    engine(&state).await?.add_reminder(message, at_ms).map_err(err)
}

#[tauri::command]
pub async fn list_reminders(state: State<'_, AppState>) -> Result<Vec<Reminder>, String> {
    Ok(engine(&state).await?.list_reminders())
}

#[tauri::command]
pub async fn remove_reminder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    engine(&state).await?.remove_reminder(&id).map_err(err)
}

/// Marks a conversation unread from `before_ms` on. The key is the UI's conversation key.
#[tauri::command]
pub async fn mark_unread(state: State<'_, AppState>, key: String, before_ms: u64) -> Result<(), String> {
    engine(&state).await?.mark_unread(key, before_ms).map_err(err)
}
