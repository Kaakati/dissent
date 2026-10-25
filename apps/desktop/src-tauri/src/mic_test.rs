//! Mic test: the microphone through the same chain as a call, with the level sent to the settings
//! page. With playback on, the processed audio also plays through the speaker. It runs only
//! outside a call, because a call owns the devices.
//!
//! Like the voice thread, the device callbacks and this loop never run on the async runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dissent_audio::{apply_gain, CaptureChain, CaptureStream, ChainSettings, PlayoutStream};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use crate::voice::{lock, take_frames, AudioPrefs, LevelReporter, Voice};
use crate::voice_settings::{chain_settings, output_gain};

const TICK: Duration = Duration::from_millis(20);

/// A running mic test. Its thread owns the devices and stops when `stop` is set.
pub struct MicTest {
    stop: Arc<AtomicBool>,
    /// Whether the processed audio is played through the speaker.
    playback: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

/// Starts the mic test, or changes whether it plays back when it already runs. Waits until the
/// microphone is open, so a device error comes back to the caller.
#[tauri::command]
pub async fn mic_test_start(app: AppHandle, playback: bool) -> Result<(), String> {
    let voice = app.state::<Voice>();
    if voice.in_call() {
        return Err("Leave the call to test your microphone.".into());
    }
    let opened = {
        let mut slot = lock(&voice.test);
        if let Some(test) = slot.as_ref() {
            test.playback.store(playback, Ordering::Relaxed);
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let playback_flag = Arc::new(AtomicBool::new(playback));
        let (tx, rx) = mpsc::channel();
        let (s, p) = (stop.clone(), playback_flag.clone());
        let (app_for_thread, prefs) = (app.clone(), voice.prefs.clone());
        let handle = thread::Builder::new()
            .name("dissent-mic-test".into())
            .spawn(move || run(app_for_thread, prefs, s, p, tx))
            .map_err(|e| e.to_string())?;
        *slot = Some(MicTest { stop, playback: playback_flag, handle });
        rx
    };
    let result = tauri::async_runtime::spawn_blocking(move || opened.recv())
        .await
        .map_err(|e| e.to_string())?;
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            // The thread has already returned. Forget its slot, so the next start tries again.
            stop_test(&app).await;
            Err(e)
        }
        Err(_) => {
            stop_test(&app).await;
            Err("the microphone test stopped before it started".into())
        }
    }
}

/// Stops the mic test and releases its devices.
#[tauri::command]
pub async fn mic_test_stop(app: AppHandle) -> Result<(), String> {
    stop_test(&app).await;
    Ok(())
}

/// Stops the mic test, if one runs, and releases its devices. A call calls this before it opens them.
pub async fn stop_test(app: &AppHandle) {
    let Some(test) = lock(&app.state::<Voice>().test).take() else { return };
    test.stop.store(true, Ordering::Relaxed);
    let _ = tauri::async_runtime::spawn_blocking(move || test.handle.join()).await;
    info!("mic test stopped");
}

/// The mic test thread. It reports whether the microphone opened, then runs until stopped.
fn run(
    app: AppHandle,
    prefs: Arc<Mutex<AudioPrefs>>,
    stop: Arc<AtomicBool>,
    playback: Arc<AtomicBool>,
    opened: mpsc::Sender<Result<(), String>>,
) {
    let input = lock(&prefs).input.clone();
    let mut mic = match CaptureStream::open(input.as_deref()) {
        Ok(mic) => {
            let _ = opened.send(Ok(()));
            mic
        }
        Err(e) => {
            let _ = opened.send(Err(format!("could not open the microphone: {e}")));
            return;
        }
    };
    info!("mic test microphone open");

    let mut chain = CaptureChain::new(ChainSettings::default());
    let mut level = LevelReporter::new(app);
    let mut speaker: Option<PlayoutStream> = None;
    let mut read = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let mut next = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let (settings, out_gain) = {
            let p = lock(&prefs);
            (chain_settings(&p.voice), output_gain(&p.voice))
        };
        chain.set_settings(settings);

        read.clear();
        mic.read_48k(&mut read);
        pending.extend_from_slice(&read);
        out.clear();
        for mut frame in take_frames(&mut pending) {
            // The test ignores push-to-talk and mute: it is the place to check the mic.
            let report = chain.process(&mut frame, true);
            level.report(report);
            out.extend_from_slice(&frame);
        }

        if playback.load(Ordering::Relaxed) {
            if speaker.is_none() {
                let output = lock(&prefs).output.clone();
                match PlayoutStream::open(output.as_deref()) {
                    Ok(s) => speaker = Some(s),
                    Err(e) => warn!(error = %e, "no speaker for the mic test"),
                }
            }
            if let Some(s) = speaker.as_mut() {
                apply_gain(&mut out, out_gain);
                chain.push_far(&out);
                s.write_48k(&out);
            }
        } else if speaker.take().is_some() {
            info!("mic test speaker released");
        }

        next += TICK;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    if let Some(s) = speaker {
        if let Err(e) = s.stop() {
            warn!(error = %e, "mic test speaker did not stop cleanly");
        }
    }
}
