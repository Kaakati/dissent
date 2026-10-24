use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use dissent_probe::{DialOptions, NetOptions, dial, parse_target, start_listener, summary_table};
use std::time::Duration;
use tracing_subscriber::EnvFilter;
use url::Url;

/// Measures how two devices connect: time to first ping, path upgrade, and media delivery.
#[derive(Debug, Parser)]
#[command(name = "dissent-probe", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a node that answers probes, and print the address to dial.
    Listen(NetArgs),
    /// Probe a listener: connect, watch the path upgrade, then stream media.
    Dial(DialArgs),
}

#[derive(Debug, Args)]
struct NetArgs {
    /// Extra relay URL. Repeat for more. Added to number0's relays unless --no-public-relays.
    #[arg(long = "relay", value_name = "URL", conflicts_with = "loopback")]
    relays: Vec<Url>,
    /// Drop number0's public relays. Needs at least one --relay.
    #[arg(long, conflicts_with = "loopback")]
    no_public_relays: bool,
    /// Bind only to 127.0.0.1, with relays and DHT off. Same-machine tests only.
    #[arg(long)]
    loopback: bool,
}

impl NetArgs {
    fn options(&self) -> NetOptions {
        NetOptions {
            relays: self.relays.clone(),
            no_public_relays: self.no_public_relays,
            loopback: self.loopback,
        }
    }
}

#[derive(Debug, Args)]
struct DialArgs {
    /// Peer address string from `listen`, or a bare device id (found through the DHT).
    target: String,
    #[command(flatten)]
    net: NetArgs,
    /// Seconds to watch the path upgrade.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    duration: u64,
    /// Print one JSON object on stdout instead of the summary table.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr, so stdout stays clean for --json.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mainline=error,mainline::rpc=off")),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Listen(net) => listen(net).await,
        Command::Dial(args) => dial_cmd(args).await,
    }
}

async fn listen(net: NetArgs) -> Result<()> {
    let listener = start_listener(&net.options()).await?;
    println!("Listening. Dial this address from the other device:");
    println!("  {}", listener.addr);
    println!("Device id: {}", listener.addr.id);
    println!("Press Ctrl-C to stop.");
    tokio::signal::ctrl_c().await?;
    listener.node.shutdown().await?;
    Ok(())
}

async fn dial_cmd(args: DialArgs) -> Result<()> {
    let net = args.net.options();
    let target = parse_target(&args.target)?;
    let node = dissent_net::Node::spawn(
        &dissent_identity::DeviceKey::generate(),
        dissent_probe::node_config(&net, None)?,
    )
    .await?;
    let opts = DialOptions {
        duration: Duration::from_secs(args.duration),
        ..DialOptions::default()
    };
    let result = dial(&node, &target, &opts, &net).await;
    node.shutdown().await?;
    let report = result?;
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", summary_table(&report));
    }
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
    fn listen_takes_relay_flags() {
        let cli = Cli::try_parse_from([
            "dissent-probe",
            "listen",
            "--relay",
            "https://relay.example.org",
            "--no-public-relays",
        ])
        .unwrap();
        let Command::Listen(net) = cli.command else {
            panic!("expected listen");
        };
        assert_eq!(net.relays.len(), 1);
        assert!(net.no_public_relays);
        assert!(!net.loopback);
    }

    #[test]
    fn dial_defaults() {
        let cli = Cli::try_parse_from(["dissent-probe", "dial", "abc"]).unwrap();
        let Command::Dial(args) = cli.command else {
            panic!("expected dial");
        };
        assert_eq!(args.target, "abc");
        assert_eq!(args.duration, 30);
        assert!(!args.json);
        assert!(!args.net.loopback);
    }

    #[test]
    fn dial_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "dissent-probe",
            "dial",
            "target",
            "--relay",
            "https://a.example.org",
            "--relay",
            "https://b.example.org",
            "--duration",
            "5",
            "--json",
        ])
        .unwrap();
        let Command::Dial(args) = cli.command else {
            panic!("expected dial");
        };
        assert_eq!(args.net.relays.len(), 2);
        assert_eq!(args.duration, 5);
        assert!(args.json);
    }

    #[test]
    fn loopback_conflicts_with_relays() {
        assert!(
            Cli::try_parse_from(["dissent-probe", "listen", "--loopback", "--relay", "https://a.example.org"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["dissent-probe", "dial", "t", "--duration", "0"]).is_err());
    }
}
