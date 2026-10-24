use anyhow::{Context, Result};
use clap::Parser;
use dissent_push_relay::{ApnsSettings, Config, app, build_state};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

/// Content-blind wake-up relay. It sends empty pushes to registered devices, and sees no content.
#[derive(Debug, Parser)]
#[command(name = "dissent-push-relay", version, about)]
struct Cli {
    /// Address to serve on. Put a TLS proxy in front of it for public use.
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: SocketAddr,

    /// Allow plain HTTP and private addresses in UnifiedPush endpoints. Tests and local runs only.
    #[arg(long)]
    dev: bool,

    /// Host allowed as a UnifiedPush endpoint. Repeat for more. Without any, any public host works.
    #[arg(long = "ups-allow-host", value_name = "HOST")]
    ups_allow_hosts: Vec<String>,

    /// JSON file for the registrations. Read at start, written after each change. Holds hashes of
    /// tokens and secrets, plus the endpoints themselves.
    #[arg(long)]
    snapshot: Option<PathBuf>,

    /// APNs `.p8` key file. Turns on APNs. Needs the key id, team id and topic.
    #[arg(long, requires_all = ["apns_key_id", "apns_team_id", "apns_topic"])]
    apns_key: Option<PathBuf>,

    /// APNs key id, 10 characters.
    #[arg(long, requires = "apns_key")]
    apns_key_id: Option<String>,

    /// Apple team id, 10 characters.
    #[arg(long, requires = "apns_key")]
    apns_team_id: Option<String>,

    /// App bundle id, sent as the APNs topic.
    #[arg(long, requires = "apns_key")]
    apns_topic: Option<String>,

    /// Use APNs sandbox instead of production.
    #[arg(long, requires = "apns_key")]
    apns_sandbox: bool,
}

impl Cli {
    fn apns(&self) -> Result<Option<ApnsSettings>> {
        let Some(path) = &self.apns_key else {
            return Ok(None);
        };
        let key_pem = std::fs::read_to_string(path)
            .with_context(|| format!("read APNs key {}", path.display()))?;
        Ok(Some(ApnsSettings {
            key_pem,
            key_id: self.apns_key_id.clone().unwrap_or_default(),
            team_id: self.apns_team_id.clone().unwrap_or_default(),
            topic: self.apns_topic.clone().unwrap_or_default(),
            sandbox: self.apns_sandbox,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if cli.dev && !cli.bind.ip().is_loopback() {
        tracing::warn!("--dev on a non-loopback address allows private endpoints from anyone");
    }
    let cfg = Config {
        dev: cli.dev,
        ups_allow_hosts: cli.ups_allow_hosts.clone(),
        snapshot: cli.snapshot.clone(),
    };
    let state = build_state(cfg, cli.apns()?).context("start relay state")?;
    let listener = tokio::net::TcpListener::bind(cli.bind)
        .await
        .with_context(|| format!("bind {}", cli.bind))?;
    println!("Push relay listening on http://{}", cli.bind);
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .context("serve")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_bind_loopback() {
        let cli = Cli::try_parse_from(["dissent-push-relay"]).unwrap();
        assert_eq!(cli.bind, "127.0.0.1:8787".parse().unwrap());
        assert!(!cli.dev);
        assert!(cli.apns().unwrap().is_none());
    }

    #[test]
    fn apns_flags_come_as_a_group() {
        assert!(Cli::try_parse_from(["dissent-push-relay", "--apns-key", "k.p8"]).is_err());
        assert!(Cli::try_parse_from([
            "dissent-push-relay",
            "--apns-key",
            "k.p8",
            "--apns-key-id",
            "ABC1234567",
            "--apns-team-id",
            "TEAM123456",
            "--apns-topic",
            "org.example.dissent",
        ])
        .is_ok());
    }
}
