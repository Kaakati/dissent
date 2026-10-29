//! One-device network check: what iroh's net report says about this network (UDP, NAT mapping, IPv4 and
//! IPv6, the preferred relay), the latency to each relay, and whether the Mainline DHT answers. It needs
//! no second device, so it is the first thing to run on a new network.

use crate::{NetOptions, node_config};
use anyhow::{Context, Result};
use dissent_discovery::Discovery;
use dissent_identity::DeviceKey;
use dissent_net::Node;
use iroh::Watcher;
use iroh::unstable_net_report::NetReport;
use serde::Serialize;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// How often the check reads the latest net report. iroh reruns its report every 20 to 26 s.
const REPORT_POLL: Duration = Duration::from_millis(500);
/// How long the DHT node warms up before the first timed lookup. A fresh routing table is empty.
const DHT_WARMUP: Duration = Duration::from_secs(5);
/// Timed DHT lookups per check.
const DHT_LOOKUPS: usize = 5;
/// TCP connects per relay, and the pause between them.
const TCP_SAMPLES: usize = 7;
const TCP_GAP: Duration = Duration::from_millis(200);
const TCP_TIMEOUT: Duration = Duration::from_secs(5);

/// What the net report says about this network. Public addresses are left out on purpose, so a result
/// file can be shared: only whether one was found is kept.
#[derive(Debug, Clone, Serialize)]
pub struct Nat {
    /// A QUIC address discovery round trip completed over IPv4.
    pub udp_v4: bool,
    /// The same over IPv6.
    pub udp_v6: bool,
    /// The public IPv4 address differs per destination (a "hard" NAT). `null` when not measured.
    pub mapping_varies_ipv4: Option<bool>,
    pub mapping_varies_ipv6: Option<bool>,
    /// A public IPv4 address was discovered.
    pub public_v4: bool,
    /// A public IPv6 address was discovered.
    pub public_v6: bool,
    pub captive_portal: Option<bool>,
    /// The relay iroh picked as home: the fastest it measured.
    pub home_relay: Option<String>,
    /// A plain-language reading of the fields above. See `verdict`.
    pub verdict: &'static str,
}

impl Nat {
    pub fn from_report(r: &NetReport) -> Self {
        Self {
            udp_v4: r.udp_v4,
            udp_v6: r.udp_v6,
            mapping_varies_ipv4: r.mapping_varies_by_dest_ipv4,
            mapping_varies_ipv6: r.mapping_varies_by_dest_ipv6,
            public_v4: r.global_v4.is_some(),
            public_v6: r.global_v6.is_some(),
            captive_portal: r.captive_portal,
            home_relay: r.preferred_relay.as_ref().map(|u| u.to_string()),
            verdict: verdict(r.udp_v4, r.udp_v6, r.mapping_varies_by_dest_ipv4, r.mapping_varies_by_dest_ipv6),
        }
    }
}

/// How likely a direct path is from this network, judged from one side only. The other side's NAT
/// matters as much, so two "easy" networks usually connect directly and two "hard" ones usually relay.
pub fn verdict(udp_v4: bool, udp_v6: bool, varies_v4: Option<bool>, varies_v6: Option<bool>) -> &'static str {
    if !udp_v4 && !udp_v6 {
        return "udp blocked: relay only";
    }
    // IPv6 usually has no NAT, so a stable IPv6 mapping is as good as an easy IPv4 NAT.
    if udp_v6 && varies_v6 == Some(false) {
        return "easy: direct likely (ipv6)";
    }
    match (udp_v4, varies_v4) {
        (true, Some(false)) => "easy: direct likely",
        (true, Some(true)) => "hard nat: direct only if the other side is easy",
        _ => "unknown: mapping not measured",
    }
}

/// Latency to one relay, from every net report seen during the check.
#[derive(Debug, Clone, Serialize)]
pub struct RelayLatency {
    pub url: String,
    /// Best latency per net report, in ms.
    pub samples_ms: Vec<f64>,
    pub best_ms: Option<f64>,
    pub median_ms: Option<f64>,
    /// The probe kinds that reached it: `Https`, `QadIpv4`, `QadIpv6`. Only `Https` means UDP is blocked.
    pub probes: Vec<String>,
    /// This relay was home in the last report.
    pub home: bool,
    /// TCP connect times to the relay's HTTPS port: one SYN and SYN-ACK, so one network round trip. The
    /// net report figure above is a fresh HTTPS request, which adds TCP and TLS setup, so it reads about
    /// three to four times higher. Empty when every connect failed or the check was off.
    pub tcp_rtt_ms: Vec<f64>,
    pub tcp_rtt_median_ms: Option<f64>,
}

/// Mainline DHT reachability, from a DHT node of its own.
#[derive(Debug, Clone, Serialize)]
pub struct Dht {
    pub size_estimate: usize,
    pub confidence: f64,
    /// The DHT thinks this node is behind a NAT or firewall.
    pub firewalled: bool,
    /// Wall time of each timed lookup, answered or not.
    pub lookups_ms: Vec<u64>,
    /// Median of the lookups some node answered. `null` means no lookup was answered: the DHT is unreachable.
    pub answered_median_ms: Option<u64>,
}

/// The result of `check`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub duration_s: f64,
    /// Time until the node first connected to a relay. `null` if it never did.
    pub online_ms: Option<u64>,
    /// Net reports seen. iroh reruns its report every 20 to 26 s.
    pub reports_seen: usize,
    /// From the last net report. `null` if none finished.
    pub nat: Option<Nat>,
    pub relays: Vec<RelayLatency>,
    /// `null` when the DHT check was turned off.
    pub dht: Option<Dht>,
}

/// Times TCP connects to `relay`'s host on its port (443 for https). The name is resolved once first,
/// so the samples hold no DNS time. Outbound TCP only: nothing listens, so no firewall prompt.
async fn tcp_rtt(relay: &url::Url) -> Vec<f64> {
    let Some(host) = relay.host_str() else {
        return Vec::new();
    };
    let port = relay.port_or_known_default().unwrap_or(443);
    let addr = match tokio::net::lookup_host((host, port)).await.map(|mut a| a.next()) {
        Ok(Some(addr)) => addr,
        Ok(None) | Err(_) => {
            warn!(%relay, "could not resolve the relay");
            return Vec::new();
        }
    };
    let mut samples = Vec::new();
    for _ in 0..TCP_SAMPLES {
        let started = Instant::now();
        match tokio::time::timeout(TCP_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
                drop(stream);
            }
            Ok(Err(e)) => warn!(%relay, "tcp connect failed: {e}"),
            Err(_) => warn!(%relay, "tcp connect timed out"),
        }
        tokio::time::sleep(TCP_GAP).await;
    }
    samples
}

/// Runs the check for `duration`. The DHT check runs alongside unless `dht` is false. With `tcp`,
/// each relay's round trip is then timed by TCP connects (`RelayLatency::tcp_rtt_ms`).
pub async fn run_check(net: &NetOptions, duration: Duration, dht: bool, tcp: bool) -> Result<CheckReport> {
    let node = Node::spawn(&DeviceKey::generate(), node_config(net, None)?)
        .await
        .context("start node")?;
    let dht_task = dht.then(|| tokio::spawn(check_dht()));
    let result = watch_reports(&node, duration).await;
    node.shutdown().await?;
    let mut report = result?;
    if tcp {
        for relay in &mut report.relays {
            let Ok(url) = url::Url::parse(&relay.url) else {
                continue;
            };
            let samples = tcp_rtt(&url).await;
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            relay.tcp_rtt_median_ms = sorted.get(sorted.len() / 2).copied();
            relay.tcp_rtt_ms = samples;
        }
    }
    if let Some(task) = dht_task {
        report.dht = match task.await.context("dht check")? {
            Ok(d) => Some(d),
            Err(e) => {
                warn!("dht check failed: {e:#}");
                None
            }
        };
    }
    Ok(report)
}

async fn watch_reports(node: &Node, duration: Duration) -> Result<CheckReport> {
    let start = Instant::now();
    let mut online_ms = None;
    {
        let online = node.online();
        tokio::pin!(online);
        if tokio::time::timeout(duration, &mut online).await.is_ok() {
            online_ms = Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));
            info!(ms = online_ms, "connected to a relay");
        } else {
            warn!("no relay connection within {duration:?}");
        }
    }
    let mut seen: Vec<NetReport> = Vec::new();
    let mut watcher = node.endpoint().net_report();
    while start.elapsed() < duration {
        if let Some(r) = watcher.get().filter(|r| seen.last() != Some(r)) {
            info!(udp_v4 = r.udp_v4, udp_v6 = r.udp_v6, home = ?r.preferred_relay, "net report");
            seen.push(r);
        }
        tokio::time::sleep(REPORT_POLL).await;
    }
    let mut relays: Vec<RelayLatency> = node
        .relays()
        .into_iter()
        .map(|u| RelayLatency {
            url: u.to_string(),
            samples_ms: Vec::new(),
            best_ms: None,
            median_ms: None,
            probes: Vec::new(),
            home: false,
            tcp_rtt_ms: Vec::new(),
            tcp_rtt_median_ms: None,
        })
        .collect();
    for r in &seen {
        for relay in &mut relays {
            let mut best: Option<Duration> = None;
            for (probe, url, latency) in r.relay_latency.iter() {
                if url.to_string() != relay.url {
                    continue;
                }
                best = Some(best.map_or(latency, |b| b.min(latency)));
                let probe = probe.to_string();
                if !relay.probes.contains(&probe) {
                    relay.probes.push(probe);
                }
            }
            if let Some(b) = best {
                relay.samples_ms.push(b.as_secs_f64() * 1000.0);
            }
        }
    }
    let last = seen.last();
    for relay in &mut relays {
        let mut sorted = relay.samples_ms.clone();
        sorted.sort_by(f64::total_cmp);
        relay.best_ms = sorted.first().copied();
        relay.median_ms = (!sorted.is_empty()).then(|| sorted[sorted.len() / 2]);
        relay.home = last
            .and_then(|r| r.preferred_relay.as_ref())
            .is_some_and(|h| h.to_string() == relay.url);
    }
    relays.sort_by(|a, b| match (a.best_ms, b.best_ms) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.url.cmp(&b.url),
    });
    Ok(CheckReport {
        duration_s: duration.as_secs_f64(),
        online_ms,
        reports_seen: seen.len(),
        nat: last.map(Nat::from_report),
        relays,
        dht: None,
    })
}

async fn check_dht() -> Result<Dht> {
    let discovery = Discovery::new().context("start dht node")?;
    tokio::time::sleep(DHT_WARMUP).await;
    let mut lookups_ms = Vec::new();
    for _ in 0..DHT_LOOKUPS {
        let started = Instant::now();
        discovery.probe_latency().await;
        lookups_ms.push(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    }
    let stats = discovery.stats().await.context("dht stats")?;
    Ok(Dht {
        size_estimate: stats.size_estimate,
        confidence: stats.confidence,
        firewalled: stats.firewalled,
        lookups_ms,
        answered_median_ms: stats.latency_ms,
    })
}

/// A plain-text summary of a check, for the terminal.
pub fn check_table(r: &CheckReport) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    let yn = |b: bool| if b { "yes" } else { "no" };
    let opt = |b: Option<bool>| b.map_or("not measured", |b| if b { "yes" } else { "no" });
    match r.online_ms {
        Some(ms) => line(format!("relay connected   {ms} ms")),
        None => line("relay connected   never".to_string()),
    }
    line(format!("net reports       {} over {:.0} s", r.reports_seen, r.duration_s));
    match &r.nat {
        Some(n) => {
            line(format!("udp ipv4 / ipv6   {} / {}", yn(n.udp_v4), yn(n.udp_v6)));
            line(format!(
                "mapping varies    ipv4 {}, ipv6 {}",
                opt(n.mapping_varies_ipv4),
                opt(n.mapping_varies_ipv6)
            ));
            line(format!("public ipv4/ipv6  {} / {}", yn(n.public_v4), yn(n.public_v6)));
            line(format!("captive portal    {}", opt(n.captive_portal)));
            line(format!("home relay        {}", n.home_relay.as_deref().unwrap_or("none")));
            line(format!("verdict           {}", n.verdict));
        }
        None => line("net report        none finished".to_string()),
    }
    for relay in &r.relays {
        let lat = match (relay.best_ms, relay.median_ms) {
            (Some(b), Some(m)) => format!("best {b:.1} ms, median {m:.1} ms, {} samples", relay.samples_ms.len()),
            _ => "unreachable".to_string(),
        };
        let home = if relay.home { " (home)" } else { "" };
        line(format!("relay             {}{home}", relay.url));
        line(format!("  net report      {lat} [{}]", relay.probes.join(",")));
        match relay.tcp_rtt_median_ms {
            Some(m) => line(format!(
                "  tcp rtt         median {m:.1} ms, {} of {TCP_SAMPLES} connects",
                relay.tcp_rtt_ms.len()
            )),
            None => line("  tcp rtt         not measured or unreachable".to_string()),
        }
    }
    match &r.dht {
        Some(d) => {
            let median = d
                .answered_median_ms
                .map_or("no lookup answered".to_string(), |m| format!("median {m} ms"));
            line(format!(
                "dht               {median}, size ~{} (confidence {:.2}), firewalled {}",
                d.size_estimate,
                d.confidence,
                yn(d.firewalled)
            ));
            line(format!("dht lookups ms    {:?}", d.lookups_ms));
        }
        None => line("dht               not checked".to_string()),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_reads_the_nat_fields() {
        assert_eq!(verdict(false, false, None, None), "udp blocked: relay only");
        assert_eq!(verdict(true, false, Some(false), None), "easy: direct likely");
        assert_eq!(
            verdict(true, false, Some(true), None),
            "hard nat: direct only if the other side is easy"
        );
        assert_eq!(verdict(true, true, Some(true), Some(false)), "easy: direct likely (ipv6)");
        assert_eq!(verdict(true, false, None, None), "unknown: mapping not measured");
    }
}
