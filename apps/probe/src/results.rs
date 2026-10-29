//! Result files: every `check` and `dial` run saves one JSON file, and `summary` turns a folder of them
//! into a table. Files are read as plain JSON values, so files from older probe versions still load.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The default folder for result files, relative to the current directory.
pub const DEFAULT_DIR: &str = "probe-results";

/// What every result file starts with. `kind` is `check` or `dial`, and `result` holds that run's report.
///
/// ```text
/// { "kind": "dial", "probe_version": "0.1.x", "unix_ms": 1761700000000, "os": "windows",
///   "network": "home Wi-Fi", "peer_network": "phone hotspot", "note": null,
///   "nat": { ... },            // this side's net report (dial only, `check` has it in `result`)
///   "result": { ... } }        // CheckReport or ProbeReport
/// ```
#[derive(Debug, Serialize)]
pub struct Record<'a, T: Serialize> {
    pub kind: &'static str,
    pub probe_version: &'static str,
    pub unix_ms: u64,
    pub os: &'static str,
    /// The network this device is on, as the person typed it.
    pub network: &'a str,
    /// The other device's network (dial only).
    pub peer_network: Option<&'a str>,
    pub note: Option<&'a str>,
    pub nat: Option<crate::check::Nat>,
    pub result: &'a T,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A file-name-safe form of a network label: lowercase letters and digits, other runs become `-`.
pub fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "unlabelled".to_string() } else { out }
}

/// Writes `record` to `<dir>/<utc time>-<kind>-<network>[-to-<peer>].json` and returns the path.
pub fn save<T: Serialize>(dir: &Path, record: &Record<'_, T>) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mut name = format!(
        "{}-{}-{}",
        utc_stamp(record.unix_ms).replace(' ', "T").replace(':', ""),
        record.kind,
        slug(record.network)
    );
    if let Some(peer) = record.peer_network {
        name.push_str("-to-");
        name.push_str(&slug(peer));
    }
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// `YYYY-MM-DD HH:MM` in UTC.
pub fn utc_stamp(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// Reads every `*.json` in `dir` (not recursive) that looks like a result file, oldest first. Files
/// that fail to parse are reported in the second list and skipped.
pub fn load_dir(dir: &Path) -> Result<(Vec<Value>, Vec<String>)> {
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|s| serde_json::from_str::<Value>(&s).map_err(anyhow::Error::from))
        {
            Ok(v) if v.get("kind").is_some() => records.push(v),
            Ok(_) => skipped.push(format!("{}: not a probe result", path.display())),
            Err(e) => skipped.push(format!("{}: {e}", path.display())),
        }
    }
    records.sort_by_key(|v| v["unix_ms"].as_u64().unwrap_or(0));
    Ok((records, skipped))
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() != 0.0 => format!("{f:.1}"),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// A relay URL without scheme and trailing dot or slash: `euc1-1.relay.n0.iroh.link`.
fn short_relay(v: &Value) -> String {
    match v.as_str() {
        Some(u) => u
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .trim_end_matches('.')
            .to_string(),
        None => "-".to_string(),
    }
}

fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = format!("| {} |\n|", header.join(" | "));
    for _ in header {
        out.push_str("---|");
    }
    out.push('\n');
    for row in rows {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out
}

/// Markdown tables for a set of result files: one for checks, one for dials.
pub fn summarize(records: &[Value]) -> String {
    let mut checks = Vec::new();
    let mut dials = Vec::new();
    for r in records {
        let when = utc_stamp(r["unix_ms"].as_u64().unwrap_or(0));
        let res = &r["result"];
        match r["kind"].as_str() {
            Some("check") => {
                let nat = &res["nat"];
                // A TCP connect is one round trip; the net report's HTTPS figure includes TLS setup.
                let relays = res["relays"].as_array().map(Vec::as_slice).unwrap_or_default();
                let by_loop = relays
                    .iter()
                    .filter_map(|r| r["tcp_rtt_median_ms"].as_f64().map(|ms| (ms, r)))
                    .min_by(|(a, _), (b, _)| a.total_cmp(b))
                    .map(|(ms, r)| format!("{} tcp {ms:.0} ms", short_relay(&r["url"])));
                let best = by_loop
                    .or_else(|| {
                        relays
                            .first()
                            .map(|b| format!("{} {} ms", short_relay(&b["url"]), s(&b["best_ms"])))
                    })
                    .unwrap_or_else(|| "-".to_string());
                let dht = &res["dht"];
                let dht = if dht.is_null() {
                    "-".to_string()
                } else if dht["answered_median_ms"].is_null() {
                    "unreachable".to_string()
                } else {
                    format!("{} ms", s(&dht["answered_median_ms"]))
                };
                checks.push(vec![
                    when,
                    s(&r["network"]),
                    r["os"].as_str().unwrap_or("-").to_string(),
                    format!("{}/{}", s(&nat["udp_v4"]), s(&nat["udp_v6"])),
                    format!("{}/{}", s(&nat["mapping_varies_ipv4"]), s(&nat["mapping_varies_ipv6"])),
                    s(&nat["verdict"]),
                    short_relay(&nat["home_relay"]),
                    best,
                    dht,
                ]);
            }
            Some("dial") => {
                let samples = res["upgrade"]["samples"].as_array();
                let last_path = samples
                    .and_then(|a| a.iter().rev().find_map(|x| x["path"].as_str()))
                    .unwrap_or("-")
                    .to_string();
                let ping = &res["upgrade"]["rtt_ms"];
                let media = &res["media"];
                let mrtt = &media["rtt_ms"];
                let ttd = match res["upgrade"]["time_to_direct_ms"].as_u64() {
                    Some(ms) => format!("{ms} ms"),
                    None => "never".to_string(),
                };
                dials.push(vec![
                    when,
                    format!("{} -> {}", s(&r["network"]), s(&r["peer_network"])),
                    format!("{} ({} ms)", s(&res["connect"]["path"]), s(&res["connect"]["ms"])),
                    ttd,
                    last_path,
                    s(&res["upgrade"]["direct_pct"]),
                    format!("{} / {} / {}", s(&ping["median"]), s(&ping["p95"]), s(&ping["p99"])),
                    format!("{}%", s(&media["delivered_pct"])),
                    format!("{} / {}", s(&mrtt["median"]), s(&mrtt["p95"])),
                    s(&media["jitter_ms"]),
                    short_relay(&res["listener_relay"]),
                ]);
            }
            _ => {}
        }
    }
    let mut out = String::new();
    if !checks.is_empty() {
        out.push_str("## Network checks\n\n");
        out.push_str(&table(
            &[
                "when (UTC)",
                "network",
                "os",
                "udp v4/v6",
                "mapping varies v4/v6",
                "verdict",
                "home relay",
                "fastest relay",
                "dht lookup",
            ],
            &checks,
        ));
        out.push('\n');
    }
    if !dials.is_empty() {
        out.push_str("## Two-device runs\n\n");
        out.push_str(&table(
            &[
                "when (UTC)",
                "dialer -> listener",
                "connect",
                "time to direct",
                "final path",
                "direct %",
                "ping rtt p50/p95/p99 ms",
                "media delivered",
                "media rtt p50/p95 ms",
                "jitter ms",
                "relay",
            ],
            &dials,
        ));
    }
    if out.is_empty() {
        out.push_str("no probe results found\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slug_keeps_letters_and_digits() {
        assert_eq!(slug("Home Wi-Fi (5 GHz)"), "home-wi-fi-5-ghz");
        assert_eq!(slug("  "), "unlabelled");
    }

    #[test]
    fn utc_stamp_formats_known_dates() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00");
        assert_eq!(utc_stamp(1_761_700_000_000), "2025-10-29 01:06");
        assert_eq!(utc_stamp(951_782_400_000), "2000-02-29 00:00");
    }

    #[test]
    fn summary_has_a_row_per_run() {
        let records = vec![
            json!({"kind": "check", "unix_ms": 0, "os": "windows", "network": "home",
                   "result": {"nat": {"udp_v4": true, "udp_v6": false, "mapping_varies_ipv4": false,
                   "mapping_varies_ipv6": null, "verdict": "easy: direct likely",
                   "home_relay": "https://euc1-1.relay.n0.iroh.link./"},
                   "relays": [{"url": "https://euc1-1.relay.n0.iroh.link./", "best_ms": 21.5},
                              {"url": "https://use1-1.relay.n0.iroh.link./", "best_ms": 90.0, "tcp_rtt_median_ms": 90.2},
                              {"url": "https://usw1-1.relay.n0.iroh.link./", "best_ms": 95.0, "tcp_rtt_median_ms": 20.4}],
                   "dht": {"answered_median_ms": 900}}}),
            json!({"kind": "dial", "unix_ms": 1, "network": "home", "peer_network": "hotspot",
                   "result": {"connect": {"path": "relayed", "ms": 800},
                   "upgrade": {"time_to_direct_ms": null, "direct_pct": 0.0,
                   "rtt_ms": {"median": 90.0, "p95": 120.0, "p99": 150.0},
                   "samples": [{"path": "relayed"}, {"path": null}]},
                   "media": {"delivered_pct": 99.6, "rtt_ms": {"median": 91.2, "p95": 130.0}, "jitter_ms": 3.4},
                   "listener_relay": "https://use1-1.relay.n0.iroh.link./"}}),
        ];
        let out = summarize(&records);
        assert!(out.contains("| home | windows | yes/no | no/- | easy: direct likely | euc1-1.relay.n0.iroh.link | usw1-1.relay.n0.iroh.link tcp 20 ms | 900 ms |"), "{out}");
        assert!(out.contains("| home -> hotspot | relayed (800 ms) | never | relayed | 0.0 | 90.0 / 120.0 / 150.0 | 99.6% |"), "{out}");
        assert!(out.contains("use1-1.relay.n0.iroh.link |"), "{out}");
    }

    #[test]
    fn saved_files_load_back() {
        let dir = std::env::temp_dir().join(format!("dissent-probe-results-{}", now_ms()));
        let body = json!({"nat": null, "relays": [], "dht": null});
        let rec = Record {
            kind: "check",
            probe_version: "test",
            unix_ms: 1_761_700_000_000,
            os: "test",
            network: "Phone hotspot",
            peer_network: None,
            note: None,
            nat: None,
            result: &body,
        };
        let path = save(&dir, &rec).unwrap();
        assert!(path.file_name().unwrap().to_str().unwrap().ends_with("-check-phone-hotspot.json"));
        std::fs::write(dir.join("other.json"), "{}").unwrap();
        let (records, skipped) = load_dir(&dir).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(skipped.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
