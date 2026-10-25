//! Real voice I/O. While this device is in a call, a plain OS thread opens the microphone and
//! speaker, runs the microphone chain, sends 20 ms frames to the engine and plays back what the
//! engine mixes. Device callbacks and the frame loop never run on the async runtime.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dissent_audio::{
    apply_gain, default_input_name, default_output_name, list_inputs, list_outputs, CaptureChain,
    CaptureStream, ChainSettings, DeviceInfo, DeviceKind, FrameReport, PlayoutStream,
};
use dissent_core::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::Shortcut;
use tracing::{info, warn};

use crate::commands::AppState;
use crate::mic_test::MicTest;
use crate::voice_settings::{chain_settings, output_gain, parse_key, VoiceSettings};

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
/// Mic levels reach the window at most this often, about 20 Hz.
const LEVEL_EVERY: Duration = Duration::from_millis(50);
/// Event carrying the mic level while a call or the settings page is listening.
pub const LEVEL_EVENT: &str = "dissent://mic-level";

/// The devices the user picked and the voice settings. `None` devices mean the system default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPrefs {
    pub input: Option<String>,
    pub output: Option<String>,
    /// Participant count at which a forwarded call grows into a cascade. `None` means the engine default.
    pub cascade_at: Option<usize>,
    pub voice: VoiceSettings,
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

/// Managed app state: where prefs live, the saved device and voice choices, the running voice
/// thread, the running mic test and the push-to-talk state.
pub struct Voice {
    pub(crate) data_dir: PathBuf,
    pub(crate) prefs: Arc<Mutex<AudioPrefs>>,
    worker: Mutex<Option<Worker>>,
    pub(crate) test: Mutex<Option<MicTest>>,
    /// True while the push-to-talk key is held down.
    pub(crate) ptt_held: Arc<AtomicBool>,
}

struct Worker {
    stop: Arc<AtomicBool>,
    capture_on: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Voice {
    pub fn new(data_dir: PathBuf) -> Self {
        let prefs = load(&data_dir);
        Voice {
            data_dir,
            prefs: Arc::new(Mutex::new(prefs)),
            worker: Mutex::new(None),
            test: Mutex::new(None),
            ptt_held: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts the worker, or updates its mute state when it already runs.
    fn apply(&self, app: &AppHandle, engine: Engine, plan: Plan) {
        let Plan::Run { capture } = plan else { return };
        let mut slot = lock(&self.worker);
        if let Some(worker) = slot.as_ref() {
            worker.capture_on.store(capture, Ordering::Relaxed);
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let capture_on = Arc::new(AtomicBool::new(capture));
        let (s, c) = (stop.clone(), capture_on.clone());
        let (app, prefs, held) = (app.clone(), self.prefs.clone(), self.ptt_held.clone());
        match thread::Builder::new()
            .name("dissent-voice".into())
            .spawn(move || run(app, engine, s, c, prefs, held))
        {
            Ok(handle) => *slot = Some(Worker { stop, capture_on, handle }),
            Err(e) => warn!(error = %e, "could not start the voice thread; this call is silent"),
        }
    }

    fn take(&self) -> Option<Worker> {
        lock(&self.worker).take()
    }

    /// True while a call's voice thread runs. The mic test yields to it, since a call owns the devices.
    pub fn in_call(&self) -> bool {
        lock(&self.worker).is_some()
    }

    /// Changes the saved preferences and writes them to disk. The memory copy changes only when
    /// the write succeeds.
    pub(crate) fn update_prefs(&self, change: impl FnOnce(&mut AudioPrefs)) -> Result<(), String> {
        let mut prefs = lock(&self.prefs).clone();
        change(&mut prefs);
        save(&self.data_dir, &prefs)?;
        *lock(&self.prefs) = prefs;
        Ok(())
    }
}

/// Makes new calls use the voice pipeline and the saved cascade setting. Call once for each engine.
/// The desktop chain does the noise suppression and AGC, so the engine's own copies are turned off.
pub fn prepare(engine: &Engine, voice: &Voice) {
    let prefs = lock(&voice.prefs).clone();
    engine.set_voice_audio(true);
    engine.set_voice_sender_config(dissent_audio::SenderConfig {
        noise_suppression: false,
        agc: false,
        ..dissent_audio::SenderConfig::default()
    });
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
        (Some(engine), Some(muted)) => {
            // A call owns the devices, so a running mic test gives them up first.
            crate::mic_test::stop_test(app).await;
            app.state::<Voice>().apply(app, engine, plan(true, muted));
        }
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

/// Sets the push-to-talk state from a global shortcut event. Other shortcuts are ignored.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, pressed: bool) {
    let Some(voice) = app.try_state::<Voice>() else { return };
    let binding = lock(&voice.prefs).voice.ptt_binding().map(str::to_string);
    if let Some(key) = binding.and_then(|k| parse_key(&k).ok()) {
        if key.id() == shortcut.id() {
            voice.ptt_held.store(pressed, Ordering::Relaxed);
        }
    }
}

fn due(retry_at: Option<Instant>) -> bool {
    match retry_at {
        None => true,
        Some(t) => Instant::now() >= t,
    }
}

/// One mic level, as the window receives it on [`LEVEL_EVENT`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct MicLevel {
    /// The processed level before the gate, in dBFS. -100 is silence.
    pub db: f32,
    /// True when the frame passed the gate, so it is being sent.
    pub open: bool,
    /// The threshold the level is compared with, in dBFS.
    pub threshold_db: f32,
}

/// Sends mic levels to the window at about 20 Hz. Each frame is reported only if the last report
/// is old enough.
pub struct LevelReporter {
    app: AppHandle,
    last: Option<Instant>,
}

impl LevelReporter {
    pub fn new(app: AppHandle) -> Self {
        LevelReporter { app, last: None }
    }

    pub fn report(&mut self, frame: FrameReport) {
        if self.last.is_some_and(|t| t.elapsed() < LEVEL_EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        let level = MicLevel {
            db: frame.level_db,
            open: frame.open,
            threshold_db: frame.threshold_db,
        };
        if let Err(e) = self.app.emit(LEVEL_EVENT, level) {
            warn!(error = %e, "could not send the mic level");
        }
    }
}

/// The voice thread. It owns the devices, so it opens and drops them on this thread.
fn run(
    app: AppHandle,
    engine: Engine,
    stop: Arc<AtomicBool>,
    capture_on: Arc<AtomicBool>,
    prefs: Arc<Mutex<AudioPrefs>>,
    held: Arc<AtomicBool>,
) {
    let (input, output) = {
        let p = lock(&prefs);
        (p.input.clone(), p.output.clone())
    };
    let mut playout = match PlayoutStream::open(output.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "no speaker; this call is silent");
            return;
        }
    };
    info!("speaker open");
    let mut chain = CaptureChain::new(ChainSettings::default());
    let mut level = LevelReporter::new(app);
    let mut mic: Option<CaptureStream> = None;
    let mut retry_at: Option<Instant> = None;
    let mut read = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut play = vec![0f32; FRAME];
    let (mut sent, mut played) = (0u64, 0u64);
    let mut next = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        // Read the live settings each tick, so a change applies at once.
        let (settings, out_gain, ptt_on) = {
            let p = lock(&prefs);
            (chain_settings(&p.voice), output_gain(&p.voice), p.voice.ptt_binding().is_some())
        };
        chain.set_settings(settings);

        if capture_on.load(Ordering::Relaxed) {
            if mic.is_none() && due(retry_at) {
                match CaptureStream::open(input.as_deref()) {
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
            for mut frame in take_frames(&mut pending) {
                // With push-to-talk on, audio goes out only while the key is held.
                let allowed = !ptt_on || held.load(Ordering::Relaxed);
                let report = chain.process(&mut frame, allowed);
                level.report(report);
                if engine.push_capture(&frame).is_ok() {
                    sent += 1;
                }
            }
        }

        engine.pull_playout(&mut play);
        apply_gain(&mut play, out_gain);
        // The echo canceller needs the audio as it leaves the speaker. It is fed only while the
        // mic runs, so its reference stays in step with the mic frames.
        if mic.is_some() {
            chain.push_far(&play);
        }
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

/// One device in a device list. `kind` is `headset`, `display` or `other`, for the icon.
#[derive(Serialize)]
pub struct DeviceEntry {
    name: String,
    is_default: bool,
    subtitle: Option<String>,
    kind: &'static str,
}

#[derive(Serialize)]
pub struct AudioDevices {
    inputs: Vec<DeviceEntry>,
    outputs: Vec<DeviceEntry>,
    /// The system default microphone's name, or `None` when there is none.
    default_input: Option<String>,
    /// The system default speaker's name, or `None` when there is none.
    default_output: Option<String>,
    /// The microphone the user picked, or `None` for the default.
    input: Option<String>,
    /// The speaker the user picked, or `None` for the default.
    output: Option<String>,
}

fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Headset => "headset",
        DeviceKind::Display => "display",
        DeviceKind::Other => "other",
    }
}

fn entries(list: Vec<DeviceInfo>) -> Vec<DeviceEntry> {
    list.into_iter()
        .map(|d| DeviceEntry {
            name: d.name,
            is_default: d.is_default,
            subtitle: d.subtitle,
            kind: kind_name(d.kind),
        })
        .collect()
}

#[tauri::command]
pub async fn audio_devices(voice: State<'_, Voice>) -> Result<AudioDevices, String> {
    let prefs = lock(&voice.prefs).clone();
    let (inputs, outputs, default_input, default_output) =
        tauri::async_runtime::spawn_blocking(|| {
            (list_inputs(), list_outputs(), default_input_name(), default_output_name())
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(AudioDevices {
        inputs: entries(inputs.map_err(|e| e.to_string())?),
        outputs: entries(outputs.map_err(|e| e.to_string())?),
        default_input,
        default_output,
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
    app.state::<Voice>().update_prefs(|prefs| prefs.cascade_at = Some(participants))?;
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
    app.state::<Voice>().update_prefs(|prefs| {
        prefs.input = input;
        prefs.output = output;
    })?;
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
        let prefs = AudioPrefs {
            input: Some("Mic".into()),
            output: None,
            cascade_at: Some(12),
            voice: VoiceSettings { input_volume: 150, ..VoiceSettings::default() },
        };
        save(&dir, &prefs).unwrap();
        assert_eq!(load(&dir), prefs);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_kinds_have_stable_wire_names() {
        assert_eq!(kind_name(DeviceKind::Headset), "headset");
        assert_eq!(kind_name(DeviceKind::Display), "display");
        assert_eq!(kind_name(DeviceKind::Other), "other");
    }
}
