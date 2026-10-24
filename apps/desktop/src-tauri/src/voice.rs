//! Real voice I/O. While this device is in a call, a plain OS thread opens the microphone and
//! speaker, sends 20 ms frames to the engine and plays back what the engine mixes. Device
//! callbacks and the frame loop never run on the async runtime.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dissent_audio::{list_inputs, list_outputs, CaptureStream, DeviceInfo, PlayoutStream};
use dissent_core::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tracing::{info, warn};

use crate::commands::AppState;

/// 20 ms of mono audio at 48 kHz, the frame size the engine takes.
pub const FRAME: usize = 960;
const TICK: Duration = Duration::from_millis(20);
const RETRY: Duration = Duration::from_secs(2);
const FILE: &str = "audio.json";
const REPORT_EVERY: u64 = 250;
/// The engine's own default for when a forwarded call grows into a cascade.
pub const DEFAULT_CASCADE_AT: usize = 50;
const CASCADE_MIN: usize = 3;
const CASCADE_MAX: usize = 500;

/// The devices the user picked. `None` means the system default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPrefs {
    pub input: Option<String>,
    pub output: Option<String>,
    /// Participant count at which a forwarded call grows into a cascade. `None` means the engine default.
    pub cascade_at: Option<usize>,
}

/// What the voice thread should do. Decided from the call state, never from the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No call: no devices are open.
    Idle,
    /// In a call: the speaker plays, and the microphone is open only when `capture` is true.
    Run { capture: bool },
}

/// The call state decides the plan. Capture needs a joined, unmuted call.
pub fn plan(in_call: bool, muted: bool) -> Plan {
    if in_call {
        Plan::Run { capture: !muted }
    } else {
        Plan::Idle
    }
}

/// Splits whole 20 ms frames off the front of `pending`. The remainder stays for the next tick.
pub fn take_frames(pending: &mut Vec<f32>) -> Vec<Vec<f32>> {
    let whole = pending.len() / FRAME * FRAME;
    let rest = pending.split_off(whole);
    let frames = pending.as_chunks::<FRAME>().0.iter().map(|f| f.to_vec()).collect();
    *pending = rest;
    frames
}

/// Managed app state: where prefs live, the saved device choice, and the running voice thread.
pub struct Voice {
    data_dir: PathBuf,
    prefs: Mutex<AudioPrefs>,
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    stop: Arc<AtomicBool>,
    capture_on: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Voice {
    pub fn new(data_dir: PathBuf) -> Self {
        let prefs = load(&data_dir);
        Voice { data_dir, prefs: Mutex::new(prefs), worker: Mutex::new(None) }
    }

    /// Starts the worker, or updates its mute state when it already runs.
    fn apply(&self, engine: Engine, plan: Plan) {
        let Plan::Run { capture } = plan else { return };
        let mut slot = lock(&self.worker);
        if let Some(worker) = slot.as_ref() {
            worker.capture_on.store(capture, Ordering::Relaxed);
            return;
        }
        let prefs = lock(&self.prefs).clone();
        let stop = Arc::new(AtomicBool::new(false));
        let capture_on = Arc::new(AtomicBool::new(capture));
        let (s, c) = (stop.clone(), capture_on.clone());
        match thread::Builder::new()
            .name("dissent-voice".into())
            .spawn(move || run(engine, s, c, prefs))
        {
            Ok(handle) => *slot = Some(Worker { stop, capture_on, handle }),
            Err(e) => warn!(error = %e, "could not start the voice thread; this call is silent"),
        }
    }

    fn take(&self) -> Option<Worker> {
        lock(&self.worker).take()
    }
}

/// Makes new calls use the voice pipeline and the saved cascade setting. Call once for each engine.
pub fn prepare(engine: &Engine, voice: &Voice) {
    let prefs = lock(&voice.prefs).clone();
    engine.set_voice_audio(true);
    engine.set_cascade_at(prefs.cascade_at.unwrap_or(DEFAULT_CASCADE_AT));
}

/// Reads the call state and starts, updates or stops the voice thread to match.
pub async fn sync(app: &AppHandle) {
    let engine = {
        let state = app.state::<AppState>();
        let guard = state.engine.read().await;
        guard.clone()
    };
    let call_muted = engine.as_ref().and_then(|e| e.call_state().map(|c| c.muted));
    match (engine, call_muted) {
        (Some(engine), Some(muted)) => app.state::<Voice>().apply(engine, plan(true, muted)),
        _ => stop(app).await,
    }
}

/// Stops the voice thread and waits until it has released both devices.
pub async fn stop(app: &AppHandle) {
    let Some(worker) = app.state::<Voice>().take() else { return };
    worker.stop.store(true, Ordering::Relaxed);
    let _ = tauri::async_runtime::spawn_blocking(move || worker.handle.join()).await;
    info!("voice devices released");
}

fn due(retry_at: Option<Instant>) -> bool {
    match retry_at {
        None => true,
        Some(t) => Instant::now() >= t,
    }
}

/// The voice thread. It owns the devices, so it opens and drops them on this thread.
fn run(engine: Engine, stop: Arc<AtomicBool>, capture_on: Arc<AtomicBool>, prefs: AudioPrefs) {
    let mut playout = match PlayoutStream::open(prefs.output.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "no speaker; this call is silent");
            return;
        }
    };
    info!("speaker open");
    let mut mic: Option<CaptureStream> = None;
    let mut retry_at: Option<Instant> = None;
    let mut read = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut play = vec![0f32; FRAME];
    let (mut sent, mut played) = (0u64, 0u64);
    let mut next = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        if capture_on.load(Ordering::Relaxed) {
            if mic.is_none() && due(retry_at) {
                match CaptureStream::open(prefs.input.as_deref()) {
                    Ok(c) => {
                        info!("microphone open");
                        mic = Some(c);
                        retry_at = None;
                    }
                    Err(e) => {
                        warn!(error = %e, "microphone unavailable; retrying");
                        retry_at = Some(Instant::now() + RETRY);
                    }
                }
            }
        } else if mic.take().is_some() {
            pending.clear();
            info!("microphone released");
        }
        if let Some(cap) = mic.as_mut() {
            read.clear();
            cap.read_48k(&mut read);
            pending.extend_from_slice(&read);
            for frame in take_frames(&mut pending) {
                if engine.push_capture(&frame).is_ok() {
                    sent += 1;
                }
            }
        }
        engine.pull_playout(&mut play);
        playout.write_48k(&play);
        played += 1;
        if played % REPORT_EVERY == 0 {
            info!(sent, played, "voice frames");
        }

        next += TICK;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    if let Err(e) = playout.stop() {
        warn!(error = %e, "speaker did not stop cleanly");
    }
}

fn load(data_dir: &Path) -> AudioPrefs {
    match fs::read_to_string(data_dir.join(FILE)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(error = %e, "ignoring invalid audio.json, using system defaults");
            AudioPrefs::default()
        }),
        Err(_) => AudioPrefs::default(),
    }
}

fn save(data_dir: &Path, prefs: &AudioPrefs) -> Result<(), String> {
    let text = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    let tmp = data_dir.join("audio.json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, data_dir.join(FILE)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct DeviceEntry {
    name: String,
    is_default: bool,
}

#[derive(Serialize)]
pub struct AudioDevices {
    inputs: Vec<DeviceEntry>,
    outputs: Vec<DeviceEntry>,
    input: Option<String>,
    output: Option<String>,
}

fn entries(list: Vec<DeviceInfo>) -> Vec<DeviceEntry> {
    list.into_iter().map(|d| DeviceEntry { name: d.name, is_default: d.is_default }).collect()
}

#[tauri::command]
pub async fn audio_devices(voice: State<'_, Voice>) -> Result<AudioDevices, String> {
    let prefs = lock(&voice.prefs).clone();
    let (inputs, outputs) =
        tauri::async_runtime::spawn_blocking(|| (list_inputs(), list_outputs()))
            .await
            .map_err(|e| e.to_string())?;
    Ok(AudioDevices {
        inputs: entries(inputs.map_err(|e| e.to_string())?),
        outputs: entries(outputs.map_err(|e| e.to_string())?),
        input: prefs.input,
        output: prefs.output,
    })
}

/// The cascade threshold the next calls use. Reads the saved value, or the engine default.
#[tauri::command]
pub fn get_cascade_at(voice: State<'_, Voice>) -> usize {
    lock(&voice.prefs).cascade_at.unwrap_or(DEFAULT_CASCADE_AT)
}

/// Saves the cascade threshold and applies it to the running engine. Calls take it as they migrate.
#[tauri::command]
pub async fn set_cascade_at(app: AppHandle, participants: usize) -> Result<(), String> {
    if !(CASCADE_MIN..=CASCADE_MAX).contains(&participants) {
        return Err(format!("choose between {CASCADE_MIN} and {CASCADE_MAX} participants"));
    }
    {
        let voice = app.state::<Voice>();
        let mut prefs = lock(&voice.prefs).clone();
        prefs.cascade_at = Some(participants);
        save(&voice.data_dir, &prefs)?;
        *lock(&voice.prefs) = prefs;
    }
    let engine = app.state::<AppState>().engine.read().await.clone();
    if let Some(engine) = engine {
        engine.set_cascade_at(participants);
    }
    Ok(())
}

/// Saves the chosen devices. A call in progress moves to them at once.
#[tauri::command]
pub async fn set_audio_devices(
    app: AppHandle,
    input: Option<String>,
    output: Option<String>,
) -> Result<(), String> {
    {
        let voice = app.state::<Voice>();
        let mut prefs = lock(&voice.prefs).clone();
        prefs.input = input;
        prefs.output = output;
        save(&voice.data_dir, &prefs)?;
        *lock(&voice.prefs) = prefs;
    }
    stop(&app).await;
    sync(&app).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_call_means_no_devices() {
        assert_eq!(plan(false, false), Plan::Idle);
        assert_eq!(plan(false, true), Plan::Idle);
    }

    #[test]
    fn call_plays_and_captures_only_when_unmuted() {
        assert_eq!(plan(true, false), Plan::Run { capture: true });
        assert_eq!(plan(true, true), Plan::Run { capture: false });
    }

    #[test]
    fn frames_are_whole_and_the_rest_waits() {
        let mut pending = vec![0.5f32; FRAME * 2 + 100];
        let frames = take_frames(&mut pending);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.len() == FRAME));
        assert_eq!(pending.len(), 100);
    }

    #[test]
    fn short_input_yields_no_frame() {
        let mut pending = vec![0.0f32; FRAME - 1];
        assert!(take_frames(&mut pending).is_empty());
        assert_eq!(pending.len(), FRAME - 1);
    }

    #[test]
    fn prefs_round_trip_and_missing_file_means_defaults() {
        let dir = std::env::temp_dir().join(format!("dissent-audio-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), AudioPrefs::default());
        let prefs = AudioPrefs { input: Some("Mic".into()), output: None, cascade_at: Some(12) };
        save(&dir, &prefs).unwrap();
        assert_eq!(load(&dir), prefs);
        let _ = fs::remove_dir_all(&dir);
    }
}
