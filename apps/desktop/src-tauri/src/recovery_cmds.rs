//! Tauri commands for guardian recovery: the recovering device's side (start, add a pasted link, finish, cancel)
//! and the guardian's side (look at a request, approve or decline it). See `dissent_core::recovery_flow` and
//! `ui-next/src/lib/api.ts`.

use dissent_core::{RecoveryAnswerView, RecoveryRequestView, RecoveryView};
use tauri::State;

use crate::commands::{engine, err, AppState};

/// Starts recovering a lost account on this device: `old` is its account id or its `dissent://guardians/...` link.
#[tauri::command]
pub async fn begin_recovery(state: State<'_, AppState>, old: String) -> Result<RecoveryView, String> {
    engine(&state).await?.begin_recovery(&old).map_err(err)
}

/// The recovery in progress on this device, or null.
#[tauri::command]
pub async fn recovery_status(state: State<'_, AppState>) -> Result<Option<RecoveryView>, String> {
    engine(&state).await?.recovery().map_err(err)
}

/// Adds an approval, guardians or invite link a guardian sent by hand.
#[tauri::command]
pub async fn add_recovery_link(state: State<'_, AppState>, link: String) -> Result<RecoveryView, String> {
    engine(&state).await?.add_recovery_link(&link).map_err(err)
}

/// Rotates the lost account to this device once enough guardians approved. Returns how many servers took it.
#[tauri::command]
pub async fn finish_recovery(state: State<'_, AppState>) -> Result<usize, String> {
    engine(&state).await?.finish_recovery().await.map_err(err)
}

#[tauri::command]
pub async fn cancel_recovery(state: State<'_, AppState>) -> Result<(), String> {
    engine(&state).await?.cancel_recovery().map_err(err)
}

/// What a `dissent://recover/...` link asks, for the guardian to look at.
#[tauri::command]
pub async fn inspect_recovery_request(state: State<'_, AppState>, link: String) -> Result<RecoveryRequestView, String> {
    engine(&state).await?.inspect_recovery_request(&link).map_err(err)
}

/// Approves (with the code the person read out) or declines a recovery request as a guardian.
#[tauri::command]
pub async fn answer_recovery_request(
    state: State<'_, AppState>,
    link: String,
    approve: bool,
    code: Option<String>,
    send_invites: Option<bool>,
) -> Result<RecoveryAnswerView, String> {
    engine(&state)
        .await?
        .answer_recovery_request(&link, approve, code.as_deref(), send_invites.unwrap_or(false))
        .await
        .map_err(err)
}
