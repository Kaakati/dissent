//! Headless always-on member. It keeps community history available while the people's own
//! devices are offline. It has no special sync logic: the engine already syncs and serves
//! history, so the seeder only needs to stay online and join the communities it should hold.

use anyhow::{Context, Result};
use clap::Parser;
use dissent_core::{ChannelId, CommunityId, Engine, NodeConfig, RelayChoice};
use dissent_identity::keystore;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Identity file, relative to `--data-dir`.
pub const IDENTITY_FILE: &str = "identity.bin";
/// Engine data directory, relative to `--data-dir`.
pub const PROFILE_DIR: &str = "profile";

#[derive(Debug, Parser)]
#[command(
    name = "dissent-seeder",
    version,
    about = "Headless Dissent member that keeps community history available"
)]
pub struct Cli {
    /// Directory for the identity and engine data. Created if missing.
    #[arg(long)]
    pub data_dir: PathBuf,
    /// Display name shown to other members.
    #[arg(long, default_value = "seeder")]
    pub name: String,
    /// Invite link of a community to join. Repeat the flag for more communities.
    #[arg(long, value_name = "INVITE")]
    pub join: Vec<String>,
    /// Bind only to 127.0.0.1, with relays and DHT lookup off. For local tests.
    #[arg(long)]
    pub loopback: bool,
    /// Seconds between status lines.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    pub status_every: u64,
}

/// Network settings for the seeder. `loopback` keeps every socket on this machine.
pub fn node_config(loopback: bool) -> NodeConfig {
    if loopback {
        NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            ..NodeConfig::default()
        }
    } else {
        NodeConfig::default()
    }
}

/// Starts the seeder and runs it until Ctrl-C.
pub async fn run(cli: Cli) -> Result<()> {
    std::fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("create data dir {}", cli.data_dir.display()))?;
    let identity = keystore::load_or_create(&cli.data_dir.join(IDENTITY_FILE), now_ms())
        .context("load or create identity")?;
    let (engine, _events) = Engine::start(
        identity,
        cli.data_dir.join(PROFILE_DIR),
        node_config(cli.loopback),
    )
    .await
    .context("start engine")?;
    engine
        .set_display_name(cli.name.clone())
        .await
        .context("set display name")?;

    for invite in &cli.join {
        match engine.join_community(invite).await {
            Ok(summary) => info!(community = %summary.name, id = %summary.id, "joined community"),
            Err(e) => warn!(error = %e, "could not join community, continuing"),
        }
    }

    let mut ticks = tokio::time::interval(Duration::from_secs(cli.status_every));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("ctrl-c received, shutting down");
                break;
            }
            _ = ticks.tick() => log_status(&engine).await,
        }
    }
    engine.shutdown().await.context("shut down engine")?;
    Ok(())
}

/// Logs one line per community, then one for the network.
pub async fn log_status(engine: &Engine) {
    for summary in engine.communities() {
        let Ok(id) = summary.id.parse::<CommunityId>() else {
            continue;
        };
        let Ok(view) = engine.community_view(id) else {
            continue;
        };
        let messages: usize = view
            .channels
            .iter()
            .filter_map(|c| c.id.parse::<ChannelId>().ok())
            .map(|channel| {
                engine
                    .channel_messages(id, channel, usize::MAX)
                    .iter()
                    .filter(|m| !m.deleted)
                    .count()
            })
            .sum();
        info!(
            community = %summary.name,
            id = %summary.id,
            members = view.members.len(),
            messages,
            "community status"
        );
    }
    let net = engine.net_status().await;
    info!(
        peers = net.peers,
        relay = ?net.relay,
        dht = ?net.dht,
        "network status"
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_flags() {
        let cli = Cli::try_parse_from([
            "dissent-seeder",
            "--data-dir",
            "/var/lib/seeder",
            "--name",
            "relay-seed",
            "--join",
            "dissent://invite/one",
            "--join",
            "dissent://invite/two",
            "--loopback",
            "--status-every",
            "15",
        ])
        .expect("valid arguments");
        assert_eq!(cli.data_dir, PathBuf::from("/var/lib/seeder"));
        assert_eq!(cli.name, "relay-seed");
        assert_eq!(cli.join, ["dissent://invite/one", "dissent://invite/two"]);
        assert!(cli.loopback);
        assert_eq!(cli.status_every, 15);
    }

    #[test]
    fn cli_defaults() {
        let cli = Cli::try_parse_from(["dissent-seeder", "--data-dir", "d"]).expect("valid");
        assert_eq!(cli.name, "seeder");
        assert!(cli.join.is_empty());
        assert!(!cli.loopback);
        assert_eq!(cli.status_every, 60);
    }

    #[test]
    fn cli_requires_data_dir_and_positive_interval() {
        assert!(Cli::try_parse_from(["dissent-seeder"]).is_err());
        assert!(
            Cli::try_parse_from(["dissent-seeder", "--data-dir", "d", "--status-every", "0"])
                .is_err()
        );
    }

    #[test]
    fn loopback_config_disables_relay_and_dht() {
        let cfg = node_config(true);
        assert!(matches!(cfg.relay, RelayChoice::Disabled));
        assert!(cfg.loopback_only);
        assert!(!cfg.dht_lookup);
    }
}
