//! Connectivity probe: measures how two devices connect. It times the first ping, watches the path
//! upgrade from relayed to direct, and then measures a 50 datagram-per-second media stream.
//!
//! The listener echoes every media datagram back. The dialer therefore measures round-trip
//! delivery, not one-way delivery, and its RTT samples cover the whole round trip.

use anyhow::{Context, Result, bail};
use dissent_identity::{DeviceId, DeviceKey};
use dissent_net::{MediaHandler, MediaLink, Node, NodeConfig, PathKind, PeerAddr, RelayChoice};
use serde::Serialize;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use url::Url;

/// number0's public relays, as `iroh::defaults::prod::default_relay_map()` returns them in iroh 1.4.1.
/// Copied from the desktop app, which keeps the same list. Update both when iroh changes.
pub const N0_RELAYS: &[&str] = &[
    "https://use1-1.relay.n0.iroh.link./",
    "https://usw1-1.relay.n0.iroh.link./",
    "https://euc1-1.relay.n0.iroh.link./",
    "https://aps1-1.relay.n0.iroh.link./",
];

/// How often the upgrade phase pings.
pub const PING_EVERY: Duration = Duration::from_millis(500);
/// How long the connect phase keeps retrying before it gives up.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// Media packets per second.
pub const MEDIA_RATE: u32 = 50;
/// Bytes per media packet. The first 8 bytes carry the sequence number.
pub const MEDIA_PAYLOAD: usize = 1000;
/// How long the media phase waits for late echoes after the last send.
const MEDIA_GRACE: Duration = Duration::from_secs(1);

/// Network settings shared by `listen` and `dial`.
#[derive(Debug, Clone, Default)]
pub struct NetOptions {
    /// Extra relays. Added to the public set unless `no_public_relays` is set.
    pub relays: Vec<Url>,
    /// Drop number0's relays. Needs at least one entry in `relays`.
    pub no_public_relays: bool,
    /// Bind only to 127.0.0.1, with relays and DHT lookup off.
    pub loopback: bool,
}

/// The node config for these options. The default is iroh's relays and the DHT. Extra relays
/// make the set custom, with number0's relays included unless turned off.
pub fn node_config(opts: &NetOptions, media: Option<Arc<dyn MediaHandler>>) -> Result<NodeConfig> {
    if opts.loopback {
        if !opts.relays.is_empty() || opts.no_public_relays {
            bail!("--loopback cannot be combined with relay options");
        }
        return Ok(NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            media,
            ..NodeConfig::default()
        });
    }
    let relay = if opts.relays.is_empty() && !opts.no_public_relays {
        RelayChoice::Default
    } else {
        let mut list: Vec<Url> = Vec::new();
        if !opts.no_public_relays {
            for raw in N0_RELAYS {
                list.push(raw.parse().context("built-in relay URL")?);
            }
        }
        list.extend(opts.relays.iter().cloned());
        if list.is_empty() {
            bail!("no relays: give --relay, or drop --no-public-relays");
        }
        RelayChoice::Custom(list)
    };
    Ok(NodeConfig {
        relay,
        media,
        ..NodeConfig::default()
    })
}

/// Echoes every datagram back to the dialer, and logs when each media link opens and closes.
struct EchoHandler;

impl MediaHandler for EchoHandler {
    fn incoming(&self, from: DeviceId, link: MediaLink) {
        info!(peer = %from, path = ?link.path(), "media link opened");
        tokio::spawn(async move {
            while let Ok(bytes) = link.recv().await {
                // A failed echo means the link is closing. The next recv reports that.
                let _ = link.send(bytes);
            }
            info!(peer = %from, "media link closed");
        });
    }
}

/// A running listener and its address.
pub struct Listener {
    pub node: Node,
    pub addr: PeerAddr,
}

/// Starts a node that answers pings and echoes media. Waits briefly for a relay address, since
/// the address other devices need is only complete once the relay is connected.
pub async fn start_listener(opts: &NetOptions) -> Result<Listener> {
    let cfg = node_config(opts, Some(Arc::new(EchoHandler)))?;
    let node = Node::spawn(&DeviceKey::generate(), cfg)
        .await
        .context("start node")?;
    let wants_relay = !opts.loopback;
    let deadline = Instant::now() + Duration::from_secs(20);
    let addr = loop {
        let addr = node.addr().await;
        if !wants_relay || addr.relay.is_some() || Instant::now() >= deadline {
            break addr;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if wants_relay && addr.relay.is_none() {
        warn!("no relay connected yet; the address has direct IPs only");
    }
    Ok(Listener { node, addr })
}

/// Parses a dial target: a full peer address string, or a bare device id. A bare id depends on
/// the DHT to find addresses.
pub fn parse_target(s: &str) -> Result<PeerAddr> {
    if let Ok(addr) = PeerAddr::from_str(s) {
        return Ok(addr);
    }
    let id = DeviceId::from_str(s.trim()).context("target is neither a peer address nor a device id")?;
    Ok(PeerAddr {
        id,
        relay: None,
        direct: Vec::new(),
    })
}

/// Dial settings.
#[derive(Debug, Clone)]
pub struct DialOptions {
    /// How long the upgrade phase pings.
    pub duration: Duration,
    /// How long the media phase streams.
    pub media_duration: Duration,
}

impl Default for DialOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(30),
            media_duration: Duration::from_secs(10),
        }
    }
}

/// The JSON report, printed by `dial --json`. Times are milliseconds from the start of the dial.
/// Path values are `direct`, `relayed`, `mixed`, or `unknown`. A `null` means no value.
///
/// ```text
/// {
///   "target": "<device id>",
///   "connect": { "ms": 812, "path": "relayed", "rtt_ms": 96 },
///   "upgrade": {
///     "duration_s": 30,
///     "time_to_direct_ms": 4120,            // null if no direct path was seen
///     "samples": [ { "t_ms": 812, "path": "relayed", "rtt_ms": 95 } ]  // path/rtt null on failure
///   },
///   "media": {
///     "sent": 500, "delivered": 498, "delivered_pct": 99.6,   // round trip via the echo
///     "rtt_ms": { "median": 41.2, "p95": 88.0 },              // null if nothing arrived
///     "jitter_ms": 3.1,                                       // mean |rtt change| between packets
///     "path_start": "direct", "path_end": "direct"
///   },
///   "relays": ["https://..."],              // extra --relay URLs only, not number0's
///   "public_relays": true,
///   "loopback": false
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub target: String,
    pub connect: Connect,
    pub upgrade: Upgrade,
    pub media: Media,
    pub relays: Vec<String>,
    pub public_relays: bool,
    pub loopback: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Connect {
    pub ms: u64,
    pub path: &'static str,
    pub rtt_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Upgrade {
    pub duration_s: f64,
    pub time_to_direct_ms: Option<u64>,
    pub samples: Vec<Sample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub t_ms: u64,
    pub path: Option<&'static str>,
    pub rtt_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Media {
    pub sent: u32,
    pub delivered: u32,
    pub delivered_pct: f64,
    pub rtt_ms: Option<Rtt>,
    pub jitter_ms: Option<f64>,
    pub path_start: &'static str,
    pub path_end: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rtt {
    pub median: f64,
    pub p95: f64,
}

/// The name used in reports for a path kind.
pub fn path_name(path: PathKind) -> &'static str {
    match path {
        PathKind::Direct => "direct",
        PathKind::Relayed => "relayed",
        PathKind::Mixed => "mixed",
        PathKind::Unknown => "unknown",
    }
}

/// True when a direct IP path is open, alone or beside a relay.
fn has_direct(path: PathKind) -> bool {
    matches!(path, PathKind::Direct | PathKind::Mixed)
}

/// Runs the three phases against `target` and returns the report.
pub async fn dial(
    node: &Node,
    target: &PeerAddr,
    opts: &DialOptions,
    net: &NetOptions,
) -> Result<ProbeReport> {
    let start = Instant::now();
    let since = |i: Instant| u64::try_from(i.duration_since(start).as_millis()).unwrap_or(u64::MAX);

    // 1. Connect: retry until the first ping succeeds.
    let (connect_report, connect_ms) = loop {
        match node.ping(target).await {
            Ok(report) => break (report, since(Instant::now())),
            Err(e) => {
                if start.elapsed() >= CONNECT_TIMEOUT {
                    bail!("could not connect within {:?}: {e}", CONNECT_TIMEOUT);
                }
                tokio::time::sleep(PING_EVERY).await;
            }
        }
    };
    info!(ms = connect_ms, path = path_name(connect_report.path), "connected");

    // 2. Upgrade: ping on a fixed clock and record the path each time.
    let mut samples = Vec::new();
    let mut time_to_direct = has_direct(connect_report.path).then_some(connect_ms);
    let phase_start = Instant::now();
    while phase_start.elapsed() < opts.duration {
        let tick = Instant::now();
        let sample = match node.ping(target).await {
            Ok(report) => Sample {
                t_ms: since(tick),
                path: Some(path_name(report.path)),
                rtt_ms: Some(report.rtt_ms),
            },
            Err(_) => Sample {
                t_ms: since(tick),
                path: None,
                rtt_ms: None,
            },
        };
        if time_to_direct.is_none() && sample.path.is_some_and(|p| p == "direct" || p == "mixed") {
            time_to_direct = Some(sample.t_ms);
        }
        samples.push(sample);
        let next = tick + PING_EVERY;
        tokio::time::sleep_until(next.into()).await;
    }
    let upgrade = Upgrade {
        duration_s: opts.duration.as_secs_f64(),
        time_to_direct_ms: time_to_direct,
        samples,
    };

    // 3. Media: stream sequence-numbered datagrams and count the echoes.
    let media = run_media(node, target, opts.media_duration).await?;

    Ok(ProbeReport {
        target: target.id.to_string(),
        connect: Connect {
            ms: connect_ms,
            path: path_name(connect_report.path),
            rtt_ms: connect_report.rtt_ms,
        },
        upgrade,
        media,
        relays: net.relays.iter().map(|u| u.to_string()).collect(),
        public_relays: !net.no_public_relays,
        loopback: net.loopback,
    })
}

async fn run_media(node: &Node, target: &PeerAddr, duration: Duration) -> Result<Media> {
    let link = node
        .open_media(target.id)
        .await
        .context("open media link")?;
    let max = link
        .max_datagram_size()
        .context("media datagrams are unavailable on this link")?;
    if max < MEDIA_PAYLOAD {
        bail!("media datagrams hold {max} bytes, below the probe payload of {MEDIA_PAYLOAD}");
    }
    let path_start = link.path();

    let total = (duration.as_secs_f64() * f64::from(MEDIA_RATE)).round() as usize;
    let sent_at: Arc<Mutex<Vec<Option<Instant>>>> = Arc::new(Mutex::new(vec![None; total]));
    let rtts: Arc<Mutex<Vec<Option<f64>>>> = Arc::new(Mutex::new(vec![None; total]));

    let receiver = {
        let link = link.clone();
        let sent_at = sent_at.clone();
        let rtts = rtts.clone();
        tokio::spawn(async move {
            while let Ok(bytes) = link.recv().await {
                let Some(seq) = bytes
                    .get(..8)
                    .and_then(|b| <[u8; 8]>::try_from(b).ok())
                    .map(u64::from_le_bytes)
                    .and_then(|s| usize::try_from(s).ok())
                else {
                    continue;
                };
                let sent = sent_at.lock().unwrap_or_else(|e| e.into_inner()).get(seq).copied().flatten();
                if let Some(sent) = sent {
                    let mut out = rtts.lock().unwrap_or_else(|e| e.into_inner());
                    if out[seq].is_none() {
                        out[seq] = Some(sent.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
        })
    };

    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(MEDIA_RATE)));
    for seq in 0..total {
        tick.tick().await;
        let mut payload = vec![0u8; MEDIA_PAYLOAD];
        payload[..8].copy_from_slice(&(seq as u64).to_le_bytes());
        sent_at.lock().unwrap_or_else(|e| e.into_inner())[seq] = Some(Instant::now());
        link.send(payload).context("send media datagram")?;
    }
    tokio::time::sleep(MEDIA_GRACE).await;
    let path_end = link.path();
    link.close();
    receiver.abort();

    let rtts = rtts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let delivered_rtts: Vec<f64> = rtts.iter().flatten().copied().collect();
    let delivered = u32::try_from(delivered_rtts.len()).unwrap_or(u32::MAX);
    let sent = u32::try_from(total).unwrap_or(u32::MAX);
    let delivered_pct = if total == 0 {
        0.0
    } else {
        f64::from(delivered) * 100.0 / f64::from(sent)
    };
    let jitter_ms = {
        let consecutive: Vec<f64> = rtts
            .iter()
            .flatten()
            .zip(rtts.iter().flatten().skip(1))
            .map(|(a, b)| (b - a).abs())
            .collect();
        (!consecutive.is_empty())
            .then(|| consecutive.iter().sum::<f64>() / consecutive.len() as f64)
    };
    Ok(Media {
        sent,
        delivered,
        delivered_pct,
        rtt_ms: rtt_summary(&delivered_rtts),
        jitter_ms,
        path_start: path_name(path_start),
        path_end: path_name(path_end),
    })
}

/// Median and 95th percentile, or `None` when there are no samples.
fn rtt_summary(samples: &[f64]) -> Option<Rtt> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    Some(Rtt {
        median: at(0.5),
        p95: at(0.95),
    })
}

/// A plain-text summary table for people reading the terminal.
pub fn summary_table(report: &ProbeReport) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    line(format!("target            {}", report.target));
    line(format!(
        "connect           {} ms, path {}, rtt {} ms",
        report.connect.ms, report.connect.path, report.connect.rtt_ms
    ));
    let direct = match report.upgrade.time_to_direct_ms {
        Some(ms) => format!("{ms} ms"),
        None => "never".to_string(),
    };
    line(format!(
        "time to direct    {direct} (over {:.0} s, {} samples)",
        report.upgrade.duration_s,
        report.upgrade.samples.len()
    ));
    line(format!(
        "media path        {} at start, {} at end",
        report.media.path_start, report.media.path_end
    ));
    line(format!(
        "media delivered   {}/{} ({:.1}%), round trip",
        report.media.delivered, report.media.sent, report.media.delivered_pct
    ));
    match &report.media.rtt_ms {
        Some(rtt) => line(format!(
            "media rtt         median {:.1} ms, p95 {:.1} ms",
            rtt.median, rtt.p95
        )),
        None => line("media rtt         no echoes received".to_string()),
    }
    match report.media.jitter_ms {
        Some(j) => line(format!("media jitter      {j:.1} ms")),
        None => line("media jitter      n/a".to_string()),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_relays_are_used_by_default() {
        let cfg = node_config(&NetOptions::default(), None).unwrap();
        assert!(matches!(cfg.relay, RelayChoice::Default));
    }

    #[test]
    fn extra_relay_adds_to_n0_set() {
        let opts = NetOptions {
            relays: vec!["https://relay.example.org".parse().unwrap()],
            ..NetOptions::default()
        };
        let cfg = node_config(&opts, None).unwrap();
        let RelayChoice::Custom(list) = cfg.relay else {
            panic!("expected a custom relay set");
        };
        assert_eq!(list.len(), N0_RELAYS.len() + 1);
        assert_eq!(list.last().unwrap().host_str(), Some("relay.example.org"));
    }

    #[test]
    fn no_public_relays_keeps_only_custom() {
        let opts = NetOptions {
            relays: vec!["https://relay.example.org".parse().unwrap()],
            no_public_relays: true,
            ..NetOptions::default()
        };
        let RelayChoice::Custom(list) = node_config(&opts, None).unwrap().relay else {
            panic!("expected a custom relay set");
        };
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn no_public_relays_without_custom_is_an_error() {
        let opts = NetOptions {
            no_public_relays: true,
            ..NetOptions::default()
        };
        assert!(node_config(&opts, None).is_err());
    }

    #[test]
    fn loopback_disables_relays_and_dht() {
        let cfg = node_config(
            &NetOptions {
                loopback: true,
                ..NetOptions::default()
            },
            None,
        )
        .unwrap();
        assert!(matches!(cfg.relay, RelayChoice::Disabled));
        assert!(cfg.loopback_only);
        assert!(!cfg.dht_lookup);
    }

    #[test]
    fn loopback_refuses_relay_options() {
        let opts = NetOptions {
            loopback: true,
            no_public_relays: true,
            ..NetOptions::default()
        };
        assert!(node_config(&opts, None).is_err());
    }

    #[test]
    fn rtt_summary_picks_median_and_p95() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let rtt = rtt_summary(&samples).unwrap();
        assert_eq!(rtt.median, 51.0);
        assert_eq!(rtt.p95, 95.0);
        assert!(rtt_summary(&[]).is_none());
    }

    #[test]
    fn bare_device_id_parses_as_target() {
        let id = DeviceKey::generate().id();
        let target = parse_target(&id.to_string()).unwrap();
        assert_eq!(target.id, id);
        assert!(target.relay.is_none() && target.direct.is_empty());
        assert!(parse_target("not a target").is_err());
    }
}
