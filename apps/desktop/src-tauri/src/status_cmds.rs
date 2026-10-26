//! Tauri commands for this device's status: online, idle, do not disturb, or invisible. Also the
//! auto-idle timer, which reads window input. Names and argument keys match `ui-next/src/lib/api.ts`.
//!
//! The engine emits `StatusChanged` for every change, including an expiry, and `events.rs`
//! forwards it as `dissent://status-changed`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use dissent_core::Status;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::{engine, err, parse, AppState};

/// Quiet time after which an online status reads as idle.
const AUTO_IDLE_AFTER: Duration = Duration::from_secs(10 * 60);
/// How often the auto-idle check runs.
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(15);

/// When the window last had input. Managed as app state.
pub struct LastInput(Mutex<Instant>);

impl LastInput {
    pub fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    fn touch(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn quiet_for(&self) -> Duration {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }
}

/// Our status as the desktop shows it. `status` is `idle` while auto-idle is on.
#[derive(Serialize)]
pub struct StatusView {
    /// `online`, `idle`, `dnd` or `invisible`.
    pub status: Status,
    /// When a timed status ends, in milliseconds since the Unix epoch. `null` if it does not end.
    pub until_ms: Option<u64>,
}

/// Sets our status. `status` is `online`, `idle`, `dnd` or `invisible`. `until_ms` ends a timed
/// status, after which it reads as online. Setting a status counts as input.
#[tauri::command]
pub async fn set_status(
    state: State<'_, AppState>,
    last_input: State<'_, LastInput>,
    status: String,
    until_ms: Option<u64>,
) -> Result<(), String> {
    let status: Status = parse("status", &status)?;
    last_input.touch();
    engine(&state).await?.set_status(status, until_ms).map_err(err)
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusView, String> {
    let (status, until_ms) = engine(&state).await?.status();
    Ok(StatusView { status, until_ms })
}

/// Reports window input, so auto-idle ends. The UI calls it on input, throttled. A missing engine
/// is not an error here, because input can arrive while it restarts.
#[tauri::command]
pub async fn record_activity(
    state: State<'_, AppState>,
    last_input: State<'_, LastInput>,
) -> Result<(), String> {
    last_input.touch();
    if let Ok(engine) = engine(&state).await {
        engine.set_auto_idle(false);
    }
    Ok(())
}

/// Turns auto-idle on after `AUTO_IDLE_AFTER` without input, and off again while input continues.
/// The engine shows it only while the status is online.
pub fn start_idle_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(IDLE_CHECK_EVERY);
        loop {
            tick.tick().await;
            let idle = app.state::<LastInput>().quiet_for() >= AUTO_IDLE_AFTER;
            let engine = app.state::<AppState>().engine.read().await.clone();
            if let Some(engine) = engine {
                engine.set_auto_idle(idle);
            }
        }
    });
}
