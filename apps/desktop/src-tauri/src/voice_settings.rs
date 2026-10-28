//! Voice settings: the input profile, volumes, processing, sensitivity and push-to-talk. They are
//! saved with the device choice in `audio.json`. The voice thread and the mic test read them on
//! every tick, so a change applies at once, without a restart.

use dissent_audio::gate::clamp_manual;
use dissent_audio::{ChainSettings, EchoEngine, Threshold, STANDARD_STRENGTH, STRONG_STRENGTH};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tracing::warn;

use crate::voice::{lock, Voice};

/// Largest volume either slider can set, in percent.
pub const MAX_VOLUME: u32 = 200;

/// The input profile. Voice Isolation and Studio are fixed presets. Custom exposes the processing
/// choices below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Strong noise suppression, echo cancellation and AGC.
    #[default]
    VoiceIsolation,
    /// No processing: no noise suppression, echo cancellation or AGC.
    Studio,
    /// The processing choices as the user set them.
    Custom,
}

/// The echo cancellation engine. Standard is ours: the NLMS canceller, RNNoise and our AGC. WebRtc is sonora's port of
/// WebRTC's processing (AEC3, noise suppression, AGC2), experimental, and it runs only in a build with that feature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoEngineChoice {
    #[default]
    Standard,
    WebRtc,
}

/// Noise suppression for the Custom profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseMode {
    /// RNNoise at the standard strength.
    #[default]
    Standard,
    /// No suppression.
    Off,
}

/// Everything the Voice settings page changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub profile: Profile,
    /// Microphone volume in percent, 0 to 200.
    pub input_volume: u32,
    /// Speaker volume in percent, 0 to 200.
    pub output_volume: u32,
    /// Noise suppression, for the Custom profile.
    pub noise_suppression: NoiseMode,
    /// Echo cancellation, for the Custom profile.
    pub echo_cancellation: bool,
    /// Automatic gain control, for the Custom profile.
    pub agc: bool,
    /// Adapt the sensitivity to the noise floor, instead of using `threshold_db`.
    pub auto_sensitivity: bool,
    /// Manual sensitivity in dBFS. Frames below it are not sent.
    pub threshold_db: f32,
    /// Transmit only while `ptt_key` is held.
    pub push_to_talk: bool,
    /// The push-to-talk key as an accelerator, such as `Control+KeyV`.
    pub ptt_key: Option<String>,
    /// The echo cancellation engine. Switching it mid-call rebuilds the chain.
    pub echo_engine: EchoEngineChoice,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            profile: Profile::VoiceIsolation,
            input_volume: 100,
            output_volume: 100,
            noise_suppression: NoiseMode::Standard,
            echo_cancellation: true,
            agc: true,
            auto_sensitivity: true,
            threshold_db: -50.0,
            push_to_talk: false,
            ptt_key: None,
            echo_engine: EchoEngineChoice::Standard,
        }
    }
}

impl VoiceSettings {
    /// The push-to-talk key that is listening right now: the bound key, when push-to-talk is on.
    pub fn ptt_binding(&self) -> Option<&str> {
        if self.push_to_talk {
            self.ptt_key.as_deref()
        } else {
            None
        }
    }

    /// Checks the ranges and the key, so a bad value is refused before it is saved.
    pub fn validate(&self) -> Result<(), String> {
        if self.input_volume > MAX_VOLUME || self.output_volume > MAX_VOLUME {
            return Err(format!("volumes go from 0 to {MAX_VOLUME}%"));
        }
        if let Some(key) = &self.ptt_key {
            parse_key(key)?;
        }
        Ok(())
    }
}

/// The microphone chain's settings for these choices. The threshold is clamped to the slider range.
pub fn chain_settings(s: &VoiceSettings) -> ChainSettings {
    let (noise_suppression, echo_cancellation, agc) = match s.profile {
        Profile::VoiceIsolation => (Some(STRONG_STRENGTH), true, true),
        Profile::Studio => (None, false, false),
        Profile::Custom => {
            let ns = match s.noise_suppression {
                NoiseMode::Standard => Some(STANDARD_STRENGTH),
                NoiseMode::Off => None,
            };
            (ns, s.echo_cancellation, s.agc)
        }
    };
    let threshold = if s.auto_sensitivity {
        Threshold::Auto
    } else {
        Threshold::Manual(clamp_manual(s.threshold_db))
    };
    let engine = match s.echo_engine {
        EchoEngineChoice::Standard => EchoEngine::Standard,
        EchoEngineChoice::WebRtc => EchoEngine::WebRtc,
    };
    ChainSettings {
        input_gain: percent(s.input_volume),
        noise_suppression,
        echo_cancellation,
        agc,
        threshold,
        engine,
    }
}

/// The speaker gain for these choices, applied to the audio before it plays.
pub fn output_gain(s: &VoiceSettings) -> f32 {
    percent(s.output_volume)
}

fn percent(p: u32) -> f32 {
    p as f32 / 100.0
}

/// Reads a push-to-talk accelerator, such as `Control+KeyV`, into a shortcut.
pub fn parse_key(accel: &str) -> Result<Shortcut, String> {
    accel
        .parse::<Shortcut>()
        .map_err(|_| format!("{accel} is not a key Dissent can use for push to talk"))
}

/// Moves the global push-to-talk registration from `old` to `new`. The new key is registered
/// first, so a key another app holds leaves the old binding in place.
fn move_ptt(app: &AppHandle, old: Option<&str>, new: Option<&str>) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    let shortcuts = app.global_shortcut();
    let next = new.map(parse_key).transpose()?;
    if let Some(next) = next {
        shortcuts
            .register(next)
            .map_err(|e| format!("That key is in use by another app ({e})"))?;
    }
    if let Some(old) = old.map(parse_key).transpose()? {
        if shortcuts.is_registered(old) {
            if let Err(e) = shortcuts.unregister(old) {
                warn!(error = %e, "could not release the old push-to-talk key");
            }
        }
    }
    Ok(())
}

/// Registers the saved push-to-talk key at startup. A key another app took meanwhile is logged,
/// and push to talk waits until the user binds a key again.
pub fn restore_ptt(app: &AppHandle) {
    let binding = {
        let voice = app.state::<Voice>();
        let prefs = lock(&voice.prefs);
        prefs.voice.ptt_binding().map(str::to_string)
    };
    if let Some(key) = binding {
        if let Err(e) = move_ptt(app, None, Some(key.as_str())) {
            warn!(error = %e, "push-to-talk key not registered");
        }
    }
}

#[tauri::command]
pub fn voice_settings(voice: State<'_, Voice>) -> VoiceSettings {
    lock(&voice.prefs).voice.clone()
}

/// Saves the voice settings, moves the push-to-talk key, and applies everything to the running
/// call or mic test on its next tick.
#[tauri::command]
pub async fn set_voice_settings(app: AppHandle, settings: VoiceSettings) -> Result<(), String> {
    settings.validate()?;
    let mut settings = settings;
    settings.threshold_db = clamp_manual(settings.threshold_db);
    let voice = app.state::<Voice>();
    let old = lock(&voice.prefs).voice.clone();
    move_ptt(&app, old.ptt_binding(), settings.ptt_binding())?;
    if !settings.push_to_talk {
        voice.ptt_held.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    voice.update_prefs(|prefs| prefs.voice = settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::AudioPrefs;

    #[test]
    fn voice_isolation_is_strong_suppression_with_echo_and_agc() {
        let c = chain_settings(&VoiceSettings::default());
        assert_eq!(c.noise_suppression, Some(STRONG_STRENGTH));
        assert!(c.echo_cancellation);
        assert!(c.agc);
        assert_eq!(c.threshold, Threshold::Auto);
    }

    #[test]
    fn studio_turns_all_processing_off() {
        let s = VoiceSettings { profile: Profile::Studio, ..VoiceSettings::default() };
        let c = chain_settings(&s);
        assert_eq!(c.noise_suppression, None);
        assert!(!c.echo_cancellation);
        assert!(!c.agc);
    }

    #[test]
    fn custom_uses_the_choices_it_was_given() {
        let s = VoiceSettings {
            profile: Profile::Custom,
            noise_suppression: NoiseMode::Off,
            echo_cancellation: false,
            agc: true,
            ..VoiceSettings::default()
        };
        let c = chain_settings(&s);
        assert_eq!(c.noise_suppression, None);
        assert!(!c.echo_cancellation);
        assert!(c.agc);

        let standard = VoiceSettings { profile: Profile::Custom, ..VoiceSettings::default() };
        assert_eq!(chain_settings(&standard).noise_suppression, Some(STANDARD_STRENGTH));
    }

    #[test]
    fn manual_sensitivity_is_clamped_and_auto_ignores_it() {
        let manual = VoiceSettings {
            auto_sensitivity: false,
            threshold_db: -200.0,
            ..VoiceSettings::default()
        };
        assert_eq!(chain_settings(&manual).threshold, Threshold::Manual(-80.0));
        let auto = VoiceSettings { threshold_db: -20.0, ..VoiceSettings::default() };
        assert_eq!(chain_settings(&auto).threshold, Threshold::Auto);
    }

    #[test]
    fn volumes_map_to_linear_gain() {
        let s = VoiceSettings { input_volume: 200, output_volume: 50, ..VoiceSettings::default() };
        assert_eq!(chain_settings(&s).input_gain, 2.0);
        assert_eq!(output_gain(&s), 0.5);
    }

    #[test]
    fn validation_refuses_out_of_range_volume_and_bad_keys() {
        let loud = VoiceSettings { input_volume: 201, ..VoiceSettings::default() };
        assert!(loud.validate().is_err());
        let bad_key = VoiceSettings { ptt_key: Some("NotAKey".into()), ..VoiceSettings::default() };
        assert!(bad_key.validate().is_err());
        let good_key = VoiceSettings {
            ptt_key: Some("Control+Shift+KeyV".into()),
            ..VoiceSettings::default()
        };
        assert!(good_key.validate().is_ok());
    }

    #[test]
    fn push_to_talk_binding_needs_the_toggle() {
        let key = Some("F8".to_string());
        let off = VoiceSettings { ptt_key: key.clone(), push_to_talk: false, ..VoiceSettings::default() };
        assert_eq!(off.ptt_binding(), None);
        let on = VoiceSettings { ptt_key: key, push_to_talk: true, ..VoiceSettings::default() };
        assert_eq!(on.ptt_binding(), Some("F8"));
    }

    #[test]
    fn older_prefs_without_voice_settings_load_with_defaults() {
        let prefs: AudioPrefs = serde_json::from_str(r#"{"input":"Mic"}"#).unwrap();
        assert_eq!(prefs.input.as_deref(), Some("Mic"));
        assert_eq!(prefs.voice, VoiceSettings::default());
    }
}

#[cfg(test)]
mod engine_choice_tests {
    use super::*;

    #[test]
    fn the_engine_choice_maps_to_the_chain_and_defaults_to_standard() {
        let mut s = VoiceSettings::default();
        assert_eq!(chain_settings(&s).engine, EchoEngine::Standard, "the standard engine is the default");
        s.echo_engine = EchoEngineChoice::WebRtc;
        assert_eq!(chain_settings(&s).engine, EchoEngine::WebRtc);
    }

    #[test]
    fn older_prefs_without_the_engine_load_as_standard() {
        let old: VoiceSettings = serde_json::from_str(r#"{"profile": "custom"}"#).unwrap();
        assert_eq!(old.echo_engine, EchoEngineChoice::Standard);
        let new: VoiceSettings = serde_json::from_str(r#"{"echo_engine": "web_rtc"}"#).unwrap();
        assert_eq!(new.echo_engine, EchoEngineChoice::WebRtc);
    }
}
