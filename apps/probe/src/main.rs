use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use dissent_probe::check::{Nat, check_table, run_check};
use dissent_probe::results::{self, Record};
use dissent_probe::{DialOptions, NetOptions, dial, parse_target, start_listener, summary_table};
use iroh::Watcher;
use std::path::PathBuf;
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
    /// Check this network alone: NAT behaviour, latency to each relay, and DHT reachability.
    Check(CheckArgs),
    /// Start a node that answers probes, and print the command to run on the other device.
    Listen(ListenArgs),
    /// Probe a listener: connect, watch the path upgrade, then stream media.
    Dial(DialArgs),
    /// Print a table of every result file in a folder.
    Summary(SummaryArgs),
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

/// Where and how a run is saved.
#[derive(Debug, Args)]
struct SaveArgs {
    /// The network this device is on, in your words: "home Wi-Fi", "phone hotspot", "office guest".
    #[arg(long, short = 'n', value_name = "LABEL")]
    network: Option<String>,
    /// A free-text note saved with the result: ISP, place, anything odd.
    #[arg(long)]
    note: Option<String>,
    /// Folder for the result file.
    #[arg(long, value_name = "DIR", default_value = results::DEFAULT_DIR)]
    out: PathBuf,
    /// Do not write a result file.
    #[arg(long)]
    no_save: bool,
    /// Print one JSON object on stdout instead of the summary table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[command(flatten)]
    net: NetArgs,
    #[command(flatten)]
    save: SaveArgs,
    /// Seconds to watch the net report. iroh reruns it every 20 to 26 s, so 60 s gives about three.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(5..))]
    seconds: u64,
    /// Skip the Mainline DHT check.
    #[arg(long)]
    no_dht: bool,
    /// Skip timing TCP connects to each relay.
    #[arg(long)]
    no_tcp_rtt: bool,
}

#[derive(Debug, Args)]
struct ListenArgs {
    #[command(flatten)]
    net: NetArgs,
    /// The network this device is on. Printed into the dial command for the other device.
    #[arg(long, short = 'n', value_name = "LABEL")]
    network: Option<String>,
}

#[derive(Debug, Args)]
struct DialArgs {
    /// Peer address string from `listen`, or a bare device id (found through the DHT).
    target: String,
    #[command(flatten)]
    net: NetArgs,
    #[command(flatten)]
    save: SaveArgs,
    /// The listener's network. `listen --network` puts it into the command it prints.
    #[arg(long, value_name = "LABEL")]
    peer_network: Option<String>,
    /// Seconds to watch the path upgrade.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    duration: u64,
}

#[derive(Debug, Args)]
struct SummaryArgs {
    /// Folder of result files.
    #[arg(default_value = results::DEFAULT_DIR)]
    dir: PathBuf,
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
        Command::Check(args) => check_cmd(args).await,
        Command::Listen(args) => listen(args).await,
        Command::Dial(args) => dial_cmd(args).await,
        Command::Summary(args) => summary_cmd(args),
    }
}

/// Quotes `s` for a shell command line when it holds anything beyond letters, digits and `-_.`.
/// Double quotes work the same in PowerShell, cmd, bash and zsh for plain labels.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace(['"', '$', '`'], ""))
    }
}

/// Saves a result file unless turned off, and says where on stderr.
fn save<T: serde::Serialize>(
    args: &SaveArgs,
    kind: &'static str,
    peer_network: Option<&str>,
    nat: Option<Nat>,
    result: &T,
) -> Result<()> {
    if args.no_save {
        return Ok(());
    }
    let record = Record {
        kind,
        probe_version: env!("CARGO_PKG_VERSION"),
        unix_ms: results::now_ms(),
        os: std::env::consts::OS,
        network: args.network.as_deref().unwrap_or(""),
        peer_network,
        note: args.note.as_deref(),
        nat,
        result,
    };
    let path = results::save(&args.out, &record)?;
    eprintln!("saved {}", path.display());
    Ok(())
}

async fn check_cmd(args: CheckArgs) -> Result<()> {
    if args.net.loopback {
        bail!("check measures the real network; --loopback has no relays to measure");
    }
    let report = run_check(
        &args.net.options(),
        Duration::from_secs(args.seconds),
        !args.no_dht,
        !args.no_tcp_rtt,
    )
    .await?;
    if args.save.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", check_table(&report));
    }
    save(&args.save, "check", None, None, &report)
}

async fn listen(args: ListenArgs) -> Result<()> {
    let listener = start_listener(&args.net.options()).await?;
    let mut cmd = format!("dissent-probe dial {}", listener.addr);
    if let Some(label) = &args.network {
        cmd.push_str(&format!(" --peer-network {}", quote(label)));
    }
    for relay in &args.net.relays {
        cmd.push_str(&format!(" --relay {relay}"));
    }
    if args.net.no_public_relays {
        cmd.push_str(" --no-public-relays");
    }
    if args.net.loopback {
        cmd.push_str(" --loopback");
    }
    cmd.push_str(" --network \"<the other device's network>\"");
    println!("Listening. On the other device, run this one line:");
    println!();
    println!("{cmd}");
    println!();
    println!("Device id: {}", listener.addr.id);
    println!("Keep this running until the dial finishes, then press Ctrl-C.");
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
    let nat = node.endpoint().net_report().get().as_ref().map(Nat::from_report);
    node.shutdown().await?;
    let report = result?;
    if args.save.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", summary_table(&report));
        if let Some(n) = &nat {
            println!("this side         {}", n.verdict);
        }
    }
    save(&args.save, "dial", Some(args.peer_network.as_deref().unwrap_or("")), nat, &report)
}

fn summary_cmd(args: SummaryArgs) -> Result<()> {
    let (records, skipped) = results::load_dir(&args.dir)?;
    for line in skipped {
        eprintln!("skipped {line}");
    }
    print!("{}", results::summarize(&records));
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
            "--network",
            "home Wi-Fi",
        ])
        .unwrap();
        let Command::Listen(args) = cli.command else {
            panic!("expected listen");
        };
        assert_eq!(args.net.relays.len(), 1);
        assert!(args.net.no_public_relays);
        assert!(!args.net.loopback);
        assert_eq!(args.network.as_deref(), Some("home Wi-Fi"));
    }

    #[test]
    fn dial_defaults() {
        let cli = Cli::try_parse_from(["dissent-probe", "dial", "abc"]).unwrap();
        let Command::Dial(args) = cli.command else {
            panic!("expected dial");
        };
        assert_eq!(args.target, "abc");
        assert_eq!(args.duration, 30);
        assert!(!args.save.json);
        assert!(!args.save.no_save);
        assert_eq!(args.save.out, PathBuf::from(results::DEFAULT_DIR));
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
            "-n",
            "phone hotspot",
            "--peer-network",
            "home Wi-Fi",
            "--out",
            "runs",
        ])
        .unwrap();
        let Command::Dial(args) = cli.command else {
            panic!("expected dial");
        };
        assert_eq!(args.net.relays.len(), 2);
        assert_eq!(args.duration, 5);
        assert!(args.save.json);
        assert_eq!(args.save.network.as_deref(), Some("phone hotspot"));
        assert_eq!(args.peer_network.as_deref(), Some("home Wi-Fi"));
        assert_eq!(args.save.out, PathBuf::from("runs"));
    }

    #[test]
    fn check_defaults_and_limits() {
        let cli = Cli::try_parse_from(["dissent-probe", "check"]).unwrap();
        let Command::Check(args) = cli.command else {
            panic!("expected check");
        };
        assert_eq!(args.seconds, 60);
        assert!(!args.no_dht);
        assert!(Cli::try_parse_from(["dissent-probe", "check", "--seconds", "1"]).is_err());
    }

    #[test]
    fn loopback_conflicts_with_relays() {
        assert!(
            Cli::try_parse_from(["dissent-probe", "listen", "--loopback", "--relay", "https://a.example.org"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["dissent-probe", "dial", "t", "--duration", "0"]).is_err());
    }

    #[test]
    fn printed_dial_command_quotes_labels() {
        assert_eq!(quote("hotspot"), "hotspot");
        assert_eq!(quote("home Wi-Fi"), "\"home Wi-Fi\"");
        assert_eq!(quote(""), "\"\"");
    }
}
