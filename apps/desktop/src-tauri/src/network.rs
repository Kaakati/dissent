//! Network settings: which relays the engine uses. Persisted as `network.json` in the data dir.
//! Read at engine start; changing them restarts the engine.
//!
//! The relay modes are never `Disabled`. Disabling relays also turns off the DHT, which leaves the node
//! offline, so settings that would need it are rejected.

use std::path::Path;

use dissent_core::{merge_relays, HostedRelay, NodeConfig, RelayChoice, RelayEntry, RelayReport, RelaySource, MAX_RELAY_LIST};
use serde::{Deserialize, Serialize};
use tracing::warn;
use url::Url;

pub const FILE: &str = "network.json";
/// Upper bound on the relays this person adds, so one settings file cannot make the node dial a long
/// list. Community relays take the rest of the node's list, up to `MAX_RELAY_LIST` in all.
const MAX_RELAYS: usize = 16;
/// Shown when no relay is enabled: none is listed, public relays are off, or every one is turned off.
pub const EMPTY_OFF_MESSAGE: &str = "No relay is enabled. Add one, turn public relays back on, or re-enable one.";

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
    /// Relays to keep when the list is over its limit. Any relay can be pinned, public or custom.
    pub pinned: Vec<String>,
    /// Relays turned off. They are not dialled and never become the home relay. Relays a community
    /// names cannot be turned off here yet, since the node adds them on join.
    pub disabled: Vec<String>,
    /// Use the relays that communities name. Off, they are neither measured nor used, and only this
    /// person's relays and the defaults are. A relay sees the IP address of each device that measures it.
    pub use_community_relays: bool,
    /// Find devices through number0's DNS directory. It is faster to connect, but number0 sees this device's
    /// ID and addresses. Used only with number0's relays, so custom relays never reach it. On by default.
    pub use_n0_lookup: bool,
    /// "Hide my IP address": every connection goes through a relay, so peers see the relay's address
    /// and never this device's. No direct paths, no LAN discovery, and no Mainline DHT, whose client
    /// would show this device's IP to strangers. The relay sees the IP and the traffic's metadata, and
    /// calls and transfers are slower. Off by default. Applied when the engine restarts.
    pub hide_ip: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            use_public_relays: true,
            relays: Vec::new(),
            lan_discovery: false,
            pinned: Vec::new(),
            disabled: Vec::new(),
            use_community_relays: true,
            use_n0_lookup: true,
            hide_ip: false,
        }
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

/// Validates every relay and drops duplicates. Does not limit the count.
fn dedupe_valid(relays: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in relays {
        let text = validate_relay(raw)?.to_string();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    Ok(out)
}

/// Validates the relays this person adds, drops duplicates, and enforces their limit.
pub fn normalize(relays: &[String]) -> Result<Vec<String>, String> {
    let out = dedupe_valid(relays)?;
    if out.len() > MAX_RELAYS {
        return Err(format!("at most {MAX_RELAYS} custom relays"));
    }
    Ok(out)
}

/// The relay URLs in a list that has already been checked.
fn urls(relays: &[String]) -> Vec<Url> {
    relays.iter().filter_map(|r| validate_relay(r).ok()).collect()
}

/// The relays this person chose, in priority order: their own first, then number0's when they are on.
fn candidates(settings: &NetworkSettings) -> Vec<(RelaySource, Url)> {
    let mut out: Vec<(RelaySource, Url)> = urls(&settings.relays).into_iter().map(|u| (RelaySource::Custom, u)).collect();
    if settings.use_public_relays {
        out.extend(N0_RELAYS.iter().filter_map(|r| validate_relay(r).ok()).map(|u| (RelaySource::Public, u)));
    }
    out
}

/// The relays these settings leave enabled, merged, ordered and capped at `MAX_RELAY_LIST`. Community
/// relays are not in the settings; the node adds them when it joins their communities.
pub fn enabled(settings: &NetworkSettings) -> Vec<RelayEntry> {
    merge_relays(&candidates(settings), &urls(&settings.pinned), &urls(&settings.disabled), MAX_RELAY_LIST)
}

/// Checks a whole settings value: relay URLs, and that at least one relay stays enabled.
pub fn check(settings: &NetworkSettings) -> Result<NetworkSettings, String> {
    let pinned = dedupe_valid(&settings.pinned)?;
    let disabled = dedupe_valid(&settings.disabled)?;
    if pinned.len() > MAX_RELAY_LIST {
        return Err(format!("at most {MAX_RELAY_LIST} relays can be pinned"));
    }
    if let Some(url) = pinned.iter().find(|url| disabled.contains(url)) {
        return Err(format!("{url} is pinned and turned off: unpin it, or turn it on"));
    }
    let checked = NetworkSettings {
        use_public_relays: settings.use_public_relays,
        relays: normalize(&settings.relays)?,
        lan_discovery: settings.lan_discovery,
        pinned,
        disabled,
        use_community_relays: settings.use_community_relays,
        use_n0_lookup: settings.use_n0_lookup,
        hide_ip: settings.hide_ip,
    };
    if enabled(&checked).is_empty() {
        return Err(EMPTY_OFF_MESSAGE.to_string());
    }
    Ok(checked)
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

/// The relay mode for these settings. Checked settings always leave at least one relay enabled, so this
/// never disables relays.
pub fn relay_choice(settings: &NetworkSettings) -> RelayChoice {
    // The plain public set, with nothing added or turned off, is iroh's own default map.
    if settings.use_public_relays && settings.relays.is_empty() && settings.disabled.is_empty() {
        return RelayChoice::Default;
    }
    let entries = enabled(settings);
    if entries.is_empty() {
        // Unreachable for checked settings; fall back to the defaults rather than disable relays.
        return RelayChoice::Default;
    }
    RelayChoice::Custom(entries.into_iter().map(|e| e.url).collect())
}

/// Node config for these settings.
pub fn node_config(settings: &NetworkSettings) -> NodeConfig {
    NodeConfig {
        relay: relay_choice(settings),
        // Hiding the IP leaves the node no IP transport, so nothing reaches a peer but through a relay.
        relay_only: settings.hide_ip,
        dht_lookup: !settings.hide_ip,
        lan_discovery: settings.lan_discovery && !settings.hide_ip,
        community_relays: settings.use_community_relays,
        n0_address_lookup: settings.use_n0_lookup,
        ..Default::default()
    }
}

/// One relay in the Settings list, with what the node measured for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelayRow {
    pub url: String,
    /// "custom", "community", "hosted" or "public".
    pub source: &'static str,
    /// Best measured latency. None when the node has not reached the relay yet, or is not running.
    pub latency_ms: Option<u64>,
    /// True for the relay the node prefers now.
    pub home: bool,
    pub pinned: bool,
    pub disabled: bool,
    /// True when the node is using this relay now. A disabled relay is never in use.
    pub in_use: bool,
    /// For a hosted relay: the host's name. None for the other sources.
    pub host: Option<String>,
    /// For a hosted relay: true when this app counts the host as verified. Always false for the other sources.
    pub verified: bool,
}

/// The Settings view of the relays: the home relay, and every relay from every source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelayStatus {
    pub home: Option<String>,
    pub relays: Vec<RelayRow>,
}

fn source_label(source: RelaySource) -> &'static str {
    match source {
        RelaySource::Custom => "custom",
        RelaySource::Community => "community",
        RelaySource::Public => "public",
        RelaySource::Hosted => "hosted",
    }
}

/// Builds the Settings view. `community` holds the relays the joined communities name. `report` is what
/// the running node measured. When the node is not running it is empty, and nothing is in use.
pub fn status(settings: &NetworkSettings, community: &[String], hosted: &[HostedRelay], report: &RelayReport) -> RelayStatus {
    let mut sources: Vec<(RelaySource, Url)> = candidates(settings);
    // Each hosted URL keeps its first host, with that host's verified mark.
    let mut hosts: Vec<(Url, String, bool)> = Vec::new();
    if settings.use_community_relays {
        sources.extend(community.iter().filter_map(|r| Url::parse(r).ok()).map(|u| (RelaySource::Community, u)));
        for h in hosted {
            if let Ok(url) = Url::parse(&h.url) {
                if !hosts.iter().any(|(u, _, _)| *u == url) {
                    hosts.push((url.clone(), h.host.clone(), h.verified));
                }
                sources.push((RelaySource::Hosted, url));
            }
        }
    }
    let pinned = urls(&settings.pinned);
    let disabled = urls(&settings.disabled);
    let mut rows: Vec<RelayRow> = Vec::new();
    for (source, url) in sources {
        if rows.iter().any(|r| r.url == url.as_str()) {
            continue;
        }
        let host = hosts.iter().find(|(u, _, _)| *u == url).filter(|_| source == RelaySource::Hosted);
        let measured = report.latencies.iter().filter(|(u, _)| *u == url).filter_map(|(_, l)| *l).min();
        let known = report.latencies.iter().any(|(u, _)| *u == url);
        rows.push(RelayRow {
            url: url.to_string(),
            source: source_label(source),
            latency_ms: measured.map(|d| d.as_millis() as u64),
            home: report.home.as_ref() == Some(&url),
            pinned: pinned.contains(&url),
            disabled: disabled.contains(&url),
            in_use: known && !disabled.contains(&url),
            host: host.map(|(_, name, _)| name.clone()),
            verified: host.is_some_and(|(_, _, verified)| *verified),
        });
    }
    // Relays in use come first, fastest first, unmeasured ones after the measured. Relays not in use follow.
    rows.sort_by_key(|r| (!r.in_use, r.latency_ms.is_none(), r.latency_ms.unwrap_or_default()));
    RelayStatus { home: report.home.as_ref().map(|u| u.to_string()), relays: rows }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn custom_urls(choice: RelayChoice) -> Vec<Url> {
        match choice {
            RelayChoice::Custom(urls) => urls,
            other => panic!("expected custom relays, got {other:?}"),
        }
    }

    fn url(host: &str) -> Url {
        Url::parse(&format!("https://{host}.example.org/")).unwrap()
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
    fn an_old_settings_file_keeps_the_number0_directory_on() {
        // A network.json written before the switch existed has no use_n0_lookup field.
        let old = parse_settings(r#"{"use_public_relays": true, "relays": []}"#).unwrap();
        assert!(old.use_n0_lookup);
        assert!(node_config(&old).n0_address_lookup);
    }

    #[test]
    fn the_number0_directory_switch_reaches_the_node() {
        assert!(node_config(&NetworkSettings::default()).n0_address_lookup);
        let off = NetworkSettings { use_n0_lookup: false, ..NetworkSettings::default() };
        assert!(!node_config(&off).n0_address_lookup);
        let saved = parse_settings(r#"{"use_n0_lookup": false}"#).unwrap();
        assert!(!saved.use_n0_lookup);
        assert!(!node_config(&saved).n0_address_lookup);
    }

    #[test]
    fn hide_ip_is_off_by_default_and_an_old_file_keeps_it_off() {
        assert!(!NetworkSettings::default().hide_ip);
        let config = node_config(&NetworkSettings::default());
        assert!(!config.relay_only && config.dht_lookup);
        let old = parse_settings(r#"{"use_public_relays": true, "relays": [], "lan_discovery": true}"#).unwrap();
        assert!(!old.hide_ip, "a network.json from before the switch reads as off");
    }

    #[test]
    fn hide_ip_routes_everything_through_relays_and_leaves_the_dht_and_lan() {
        let on = parse_settings(r#"{"hide_ip": true, "lan_discovery": true}"#).unwrap();
        assert!(on.hide_ip);
        let config = node_config(&on);
        assert!(config.relay_only, "no IP transport");
        assert!(!config.dht_lookup, "no Mainline DHT socket");
        assert!(!config.lan_discovery, "no LAN beacons");
        assert!(!matches!(config.relay, RelayChoice::Disabled), "a relay to go through");
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
        let s = NetworkSettings { relays: vec!["https://relay.example.org".into()], ..NetworkSettings::default() };
        let urls = custom_urls(relay_choice(&s));
        assert_eq!(urls.len(), N0_RELAYS.len() + 1);
        assert!(urls.iter().any(|u| u.host_str() == Some("relay.example.org")));
        assert!(urls.iter().any(|u| u.host_str() == Some("use1-1.relay.n0.iroh.link.")));
    }

    #[test]
    fn public_off_uses_only_the_custom_relays() {
        let s = NetworkSettings { use_public_relays: false, relays: vec!["https://relay.example.org".into()], ..NetworkSettings::default() };
        let urls = custom_urls(relay_choice(&s));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].host_str(), Some("relay.example.org"));
    }

    #[test]
    fn public_off_with_no_relays_is_rejected_not_disabled() {
        let s = NetworkSettings { use_public_relays: false, ..NetworkSettings::default() };
        assert_eq!(check(&s).unwrap_err(), EMPTY_OFF_MESSAGE);
        assert!(parse_settings(r#"{"use_public_relays": false, "relays": []}"#).is_err());
        assert!(!matches!(relay_choice(&s), RelayChoice::Disabled));
    }

    #[test]
    fn turning_off_every_relay_is_rejected() {
        let disabled: Vec<String> = N0_RELAYS.iter().map(|r| r.to_string()).collect();
        let s = NetworkSettings { disabled, ..NetworkSettings::default() };
        assert_eq!(check(&s).unwrap_err(), EMPTY_OFF_MESSAGE);
    }

    #[test]
    fn a_disabled_public_relay_is_left_out_of_the_node() {
        let s = NetworkSettings { disabled: vec![N0_RELAYS[0].to_string()], ..NetworkSettings::default() };
        let urls = custom_urls(relay_choice(&s));
        assert_eq!(urls.len(), N0_RELAYS.len() - 1);
        assert!(!urls.iter().any(|u| u.host_str() == Some("use1-1.relay.n0.iroh.link.")));
    }

    #[test]
    fn invalid_pins_and_disables_are_rejected() {
        assert!(parse_settings(r#"{"pinned": ["http://relay.example.org"]}"#).is_err());
        assert!(parse_settings(r#"{"disabled": ["nope"]}"#).is_err());
    }

    #[test]
    fn status_lists_each_source_once_and_marks_the_home_relay() {
        let s = NetworkSettings { relays: vec![url("mine").to_string()], ..NetworkSettings::default() };
        let community = vec![url("community").to_string(), url("mine").to_string()];
        let report = RelayReport {
            home: Some(url("community")),
            latencies: vec![
                (url("community"), Some(Duration::from_millis(40))),
                (url("mine"), Some(Duration::from_millis(90))),
            ],
        };
        let view = status(&s, &community, &[], &report);
        assert_eq!(view.home, Some(url("community").to_string()));
        let mine = view.relays.iter().find(|r| r.url == url("mine").to_string()).unwrap();
        assert_eq!(mine.source, "custom", "the person's own label wins over the community's");
        let first = &view.relays[0];
        assert_eq!(first.url, url("community").to_string());
        assert!(first.home && first.in_use);
        assert_eq!(first.latency_ms, Some(40));
        assert_eq!(view.relays.iter().filter(|r| r.url == url("mine").to_string()).count(), 1);
    }

    #[test]
    fn pins_past_the_list_size_are_refused() {
        let pinned: Vec<String> = (0..=MAX_RELAY_LIST).map(|i| url(&format!("pin{i}")).to_string()).collect();
        let err = check(&NetworkSettings { pinned, ..NetworkSettings::default() }).unwrap_err();
        assert!(err.contains("at most 30 relays can be pinned"), "{err}");
    }

    #[test]
    fn a_relay_cannot_be_pinned_and_turned_off_at_once() {
        let both = url("both").to_string();
        let pinned_and_off = NetworkSettings { pinned: vec![both.clone()], disabled: vec![both.clone()], ..NetworkSettings::default() };
        assert!(check(&pinned_and_off).unwrap_err().contains("pinned and turned off"));
        // Pinning one that is off is refused the same way, whichever list it was added to first.
        let off_then_pinned = NetworkSettings { pinned: vec![both.clone()], disabled: vec![both], ..NetworkSettings::default() };
        assert!(check(&off_then_pinned).is_err());
    }

    #[test]
    fn exactly_thirty_pins_are_accepted() {
        let pinned: Vec<String> = (0..MAX_RELAY_LIST).map(|i| url(&format!("pin{i}")).to_string()).collect();
        assert!(check(&NetworkSettings { pinned, ..NetworkSettings::default() }).is_ok());
    }

    #[test]
    fn status_marks_disabled_relays_and_keeps_them_out_of_use() {
        let s = NetworkSettings { disabled: vec![url("off").to_string()], ..NetworkSettings::default() };
        let report = RelayReport {
            home: None,
            latencies: vec![(url("off"), Some(Duration::from_millis(5)))],
        };
        let view = status(&s, &[], &[], &report);
        let off = view.relays.iter().find(|r| r.url == url("off").to_string());
        assert!(off.is_none(), "a disabled custom relay is not listed unless it was added");
        let s = NetworkSettings { relays: vec![url("off").to_string()], disabled: vec![url("off").to_string()], ..NetworkSettings::default() };
        let view = status(&s, &[], &[], &report);
        let off = view.relays.iter().find(|r| r.url == url("off").to_string()).unwrap();
        assert!(off.disabled && !off.in_use);
    }

    #[test]
    fn community_relays_are_off_when_the_switch_is_off() {
        assert!(NetworkSettings::default().use_community_relays);
        assert!(node_config(&NetworkSettings::default()).community_relays);
        let off = NetworkSettings { use_community_relays: false, ..NetworkSettings::default() };
        assert!(!node_config(&off).community_relays);
        let community = vec![url("community").to_string()];
        let view = status(&off, &community, &[], &RelayReport::default());
        assert!(view.relays.iter().all(|r| r.source != "community"));
        let on = status(&NetworkSettings::default(), &community, &[], &RelayReport::default());
        assert!(on.relays.iter().any(|r| r.source == "community"));
    }

    #[test]
    fn hosted_relays_carry_the_host_and_its_verified_mark_and_respect_the_switch() {
        let hosted = vec![
            HostedRelay { url: url("hosted").to_string(), host: "Yusuf".into(), verified: true },
            HostedRelay { url: url("unchecked").to_string(), host: "Nadia".into(), verified: false },
        ];
        let view = status(&NetworkSettings::default(), &[], &hosted, &RelayReport::default());
        let yusuf = view.relays.iter().find(|r| r.url == url("hosted").to_string()).unwrap();
        assert_eq!((yusuf.source, yusuf.host.as_deref(), yusuf.verified), ("hosted", Some("Yusuf"), true));
        let nadia = view.relays.iter().find(|r| r.url == url("unchecked").to_string()).unwrap();
        assert!(!nadia.verified);
        let off = NetworkSettings { use_community_relays: false, ..NetworkSettings::default() };
        let view = status(&off, &[], &hosted, &RelayReport::default());
        assert!(view.relays.iter().all(|r| r.source != "hosted"));
    }

    #[test]
    fn status_is_empty_when_the_node_is_not_running() {
        let view = status(&NetworkSettings::default(), &[], &[], &RelayReport::default());
        assert_eq!(view.home, None);
        assert!(view.relays.iter().all(|r| !r.in_use && r.latency_ms.is_none() && !r.home));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("dissent-network-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = NetworkSettings {
            use_public_relays: false,
            relays: vec!["https://relay.example.org/".into()],
            pinned: vec!["https://relay.example.org/".into()],
            ..NetworkSettings::default()
        };
        let saved = save(&dir, &s).unwrap();
        assert_eq!(load(&dir), saved);
        assert!(save(&dir, &NetworkSettings { use_public_relays: false, ..NetworkSettings::default() }).is_err());
        std::fs::write(dir.join(FILE), "{ broken").unwrap();
        assert_eq!(load(&dir), NetworkSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
