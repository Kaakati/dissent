//! Network settings: which relays the engine uses. Persisted as `network.json` in the data dir.
//! Read at engine start; changing them restarts the engine.
//!
//! The relay modes are never `Disabled`. Disabling relays also turns off the DHT, which leaves the node
//! offline, so settings that would need it are rejected.

use std::path::Path;

use dissent_core::{NodeConfig, RelayChoice};
use serde::{Deserialize, Serialize};
use tracing::warn;
use url::Url;

pub const FILE: &str = "network.json";
/// Upper bound on custom relays, so one settings file cannot make the node dial a long list.
const MAX_RELAYS: usize = 8;
/// Shown when public relays are off and no custom relay is listed.
pub const EMPTY_OFF_MESSAGE: &str = "Add at least one relay, or turn public relays back on.";

/// number0's public relays, as `iroh::defaults::prod::default_relay_map()` returns them in iroh 1.4.1.
/// Kept here because dissent-core does not re-export iroh's defaults. Update this list when iroh changes.
const N0_RELAYS: &[&str] = &[
    "https://use1-1.relay.n0.iroh.link./",
    "https://usw1-1.relay.n0.iroh.link./",
    "https://euc1-1.relay.n0.iroh.link./",
    "https://aps1-1.relay.n0.iroh.link./",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// True adds number0's relays to any custom ones. False uses only the custom relays.
    pub use_public_relays: bool,
    /// Custom relay URLs. Each must be https.
    pub relays: Vec<String>,
    /// Find people on the same local network: a multicast beacon with rotating tokens, and a
    /// challenge handshake before any device id is shared. Off by default. Ignored in loopback mode.
    pub lan_discovery: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self { use_public_relays: true, relays: Vec::new(), lan_discovery: false }
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

/// Checks a whole settings value: relay URLs, and that the choice leaves at least one relay.
pub fn check(settings: &NetworkSettings) -> Result<NetworkSettings, String> {
    let relays = normalize(&settings.relays)?;
    if !settings.use_public_relays && relays.is_empty() {
        return Err(EMPTY_OFF_MESSAGE.to_string());
    }
    Ok(NetworkSettings { use_public_relays: settings.use_public_relays, relays, lan_discovery: settings.lan_discovery })
}

/// Parses and checks a settings document. Missing fields take their defaults.
pub fn parse_settings(json: &str) -> Result<NetworkSettings, String> {
    let raw: NetworkSettings = serde_json::from_str(json).map_err(|e| e.to_string())?;
    check(&raw)
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

/// Checks and writes the settings file atomically. Returns what was written.
pub fn save(data_dir: &Path, settings: &NetworkSettings) -> Result<NetworkSettings, String> {
    let checked = check(settings)?;
    let text = serde_json::to_string_pretty(&checked).map_err(|e| e.to_string())?;
    let tmp = data_dir.join("network.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, data_dir.join(FILE)).map_err(|e| e.to_string())?;
    Ok(checked)
}

/// The relay mode for these settings. Checked settings always leave at least one relay, so this never disables relays.
pub fn relay_choice(settings: &NetworkSettings) -> RelayChoice {
    let custom = settings.relays.iter().filter_map(|r| validate_relay(r).ok());
    match (settings.use_public_relays, settings.relays.is_empty()) {
        (true, true) => RelayChoice::Default,
        (true, false) => {
            let mut urls: Vec<Url> = N0_RELAYS.iter().filter_map(|r| validate_relay(r).ok()).collect();
            for url in custom {
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
            RelayChoice::Custom(urls)
        }
        (false, false) => RelayChoice::Custom(custom.collect()),
        // Unreachable for checked settings; fall back to the defaults rather than disable relays.
        (false, true) => RelayChoice::Default,
    }
}

/// Node config for these settings.
pub fn node_config(settings: &NetworkSettings) -> NodeConfig {
    NodeConfig { relay: relay_choice(settings), lan_discovery: settings.lan_discovery, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_urls(choice: RelayChoice) -> Vec<Url> {
        match choice {
            RelayChoice::Custom(urls) => urls,
            other => panic!("expected custom relays, got {other:?}"),
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s = parse_settings("{}").unwrap();
        assert_eq!(s, NetworkSettings::default());
        assert!(s.use_public_relays);
    }

    #[test]
    fn accepts_https_relays_and_drops_duplicates() {
        let s = parse_settings(
            r#"{"use_public_relays": true, "relays": ["https://relay.example.org", "https://relay.example.org/"]}"#,
        )
        .unwrap();
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
    fn lan_discovery_is_off_by_default_and_reaches_the_node() {
        assert!(!NetworkSettings::default().lan_discovery);
        assert!(!node_config(&NetworkSettings::default()).lan_discovery);
        let on = NetworkSettings { lan_discovery: true, ..NetworkSettings::default() };
        assert!(node_config(&on).lan_discovery);
    }

    #[test]
    fn public_on_without_custom_relays_uses_the_default_mode() {
        assert!(matches!(relay_choice(&NetworkSettings::default()), RelayChoice::Default));
    }

    #[test]
    fn public_on_with_custom_relays_adds_them_to_the_n0_set() {
        let s = NetworkSettings { use_public_relays: true, relays: vec!["https://relay.example.org".into()], lan_discovery: false };
        let urls = custom_urls(relay_choice(&s));
        assert_eq!(urls.len(), N0_RELAYS.len() + 1);
        assert!(urls.iter().any(|u| u.host_str() == Some("relay.example.org")));
        assert!(urls.iter().any(|u| u.host_str() == Some("use1-1.relay.n0.iroh.link.")));
    }

    #[test]
    fn public_off_uses_only_the_custom_relays() {
        let s = NetworkSettings { use_public_relays: false, relays: vec!["https://relay.example.org".into()], lan_discovery: false };
        let urls = custom_urls(relay_choice(&s));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].host_str(), Some("relay.example.org"));
    }

    #[test]
    fn public_off_with_no_relays_is_rejected_not_disabled() {
        let s = NetworkSettings { use_public_relays: false, relays: vec![], lan_discovery: false };
        assert_eq!(check(&s).unwrap_err(), EMPTY_OFF_MESSAGE);
        assert!(parse_settings(r#"{"use_public_relays": false, "relays": []}"#).is_err());
        assert!(!matches!(relay_choice(&s), RelayChoice::Disabled));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("dissent-network-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = NetworkSettings { use_public_relays: false, relays: vec!["https://relay.example.org/".into()], lan_discovery: false };
        let saved = save(&dir, &s).unwrap();
        assert_eq!(load(&dir), saved);
        assert!(save(&dir, &NetworkSettings { use_public_relays: false, relays: vec![], lan_discovery: false }).is_err());
        std::fs::write(dir.join(FILE), "{ broken").unwrap();
        assert_eq!(load(&dir), NetworkSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
