//! Network settings: which relays the engine uses. Persisted as `network.json` in the data dir.
//! Read at engine start; changing them restarts the engine.

use std::path::Path;

use dissent_core::{NodeConfig, RelayChoice};
use serde::{Deserialize, Serialize};
use tracing::warn;
use url::Url;

pub const FILE: &str = "network.json";
/// Upper bound on custom relays, so one settings file cannot make the node dial a long list.
const MAX_RELAYS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// True uses iroh's public n0 relays. False uses only `relays`, and no relay when that list is empty.
    pub use_public_relays: bool,
    /// Custom relay URLs. Each must be https.
    pub relays: Vec<String>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self { use_public_relays: true, relays: Vec::new() }
    }
}

/// Checks one relay URL: it must parse, use https, and name a host.
pub fn validate_relay(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|e| format!("not a valid URL ({e}): {raw}"))?;
    if url.scheme() != "https" {
        return Err(format!("a relay must use https: {raw}"));
    }
    if url.host_str().is_none() {
        return Err(format!("a relay needs a host: {raw}"));
    }
    Ok(url)
}

/// Validates every relay, drops duplicates, and enforces the list limit.
pub fn normalize(relays: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in relays {
        let text = validate_relay(raw)?.to_string();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    if out.len() > MAX_RELAYS {
        return Err(format!("at most {MAX_RELAYS} custom relays"));
    }
    Ok(out)
}

/// Parses and validates a settings document. Missing fields take their defaults.
pub fn parse_settings(json: &str) -> Result<NetworkSettings, String> {
    let raw: NetworkSettings = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(NetworkSettings { use_public_relays: raw.use_public_relays, relays: normalize(&raw.relays)? })
}

/// Reads the settings file. A missing or invalid file gives the defaults, and an invalid file is logged.
pub fn load(data_dir: &Path) -> NetworkSettings {
    match std::fs::read_to_string(data_dir.join(FILE)) {
        Ok(text) => parse_settings(&text).unwrap_or_else(|e| {
            warn!(error = %e, "ignoring invalid network.json, using public relays");
            NetworkSettings::default()
        }),
        Err(_) => NetworkSettings::default(),
    }
}

/// Validates and writes the settings file atomically.
pub fn save(data_dir: &Path, settings: &NetworkSettings) -> Result<NetworkSettings, String> {
    let checked = NetworkSettings {
        use_public_relays: settings.use_public_relays,
        relays: normalize(&settings.relays)?,
    };
    let text = serde_json::to_string_pretty(&checked).map_err(|e| e.to_string())?;
    let tmp = data_dir.join("network.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, data_dir.join(FILE)).map_err(|e| e.to_string())?;
    Ok(checked)
}

/// The relay mode for these settings.
pub fn relay_choice(settings: &NetworkSettings) -> RelayChoice {
    if settings.use_public_relays {
        RelayChoice::Default
    } else if settings.relays.is_empty() {
        RelayChoice::Disabled
    } else {
        RelayChoice::Custom(settings.relays.iter().filter_map(|r| validate_relay(r).ok()).collect())
    }
}

/// Node config for these settings.
pub fn node_config(settings: &NetworkSettings) -> NodeConfig {
    NodeConfig { relay: relay_choice(settings), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s = parse_settings("{}").unwrap();
        assert_eq!(s, NetworkSettings::default());
        assert!(s.use_public_relays);
    }

    #[test]
    fn accepts_https_relays_and_drops_duplicates() {
        let s = parse_settings(
            r#"{"use_public_relays": false, "relays": ["https://relay.example.org", "https://relay.example.org/"]}"#,
        )
        .unwrap();
        assert!(!s.use_public_relays);
        assert_eq!(s.relays, vec!["https://relay.example.org/".to_string()]);
    }

    #[test]
    fn rejects_non_https_and_garbage() {
        assert!(parse_settings(r#"{"relays": ["http://relay.example.org"]}"#).is_err());
        assert!(parse_settings(r#"{"relays": ["not a url"]}"#).is_err());
        assert!(parse_settings(r#"{"relays": ["ws://relay.example.org"]}"#).is_err());
    }

    #[test]
    fn enforces_the_relay_limit() {
        let many: Vec<String> = (0..MAX_RELAYS + 1).map(|i| format!("https://r{i}.example.org")).collect();
        assert!(normalize(&many).is_err());
        assert!(normalize(&many[..MAX_RELAYS]).is_ok());
    }

    #[test]
    fn maps_settings_to_relay_modes() {
        assert!(matches!(relay_choice(&NetworkSettings::default()), RelayChoice::Default));
        let none = NetworkSettings { use_public_relays: false, relays: vec![] };
        assert!(matches!(relay_choice(&none), RelayChoice::Disabled));
        let custom = NetworkSettings {
            use_public_relays: false,
            relays: vec!["https://relay.example.org".into()],
        };
        match relay_choice(&custom) {
            RelayChoice::Custom(urls) => assert_eq!(urls.len(), 1),
            other => panic!("expected custom relays, got {other:?}"),
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("dissent-network-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = NetworkSettings { use_public_relays: false, relays: vec!["https://relay.example.org/".into()] };
        let saved = save(&dir, &s).unwrap();
        assert_eq!(load(&dir), saved);
        std::fs::write(dir.join(FILE), "{ broken").unwrap();
        assert_eq!(load(&dir), NetworkSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
