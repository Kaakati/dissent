//! The push relay's command line, run loop, and service commands.

use crate::service::{self, Level, Plan, Spec};
use crate::{ApnsSettings, Config, app, build_state};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::info;

/// How the service layer names this program.
pub const SPEC: Spec = Spec {
    name: "dissent-push-relay",
    title: "Dissent Push Relay",
    label: "org.dissent.push-relay",
    description: "Dissent push relay: sends empty wake-up pushes to registered devices",
    path_flags: &["--data-dir", "--snapshot", "--apns-key", "--log-dir"],
    user_by_default: false,
};

/// Log file stem, in the log directory.
const LOG_STEM: &str = "dissent-push-relay";
/// Longest open requests may delay the shutdown before they are dropped.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Snapshot file, inside `--data-dir`, when `--snapshot` is not given.
pub const SNAPSHOT_FILE: &str = "registrations.json";

#[derive(Debug, Parser)]
#[command(name = "dissent-push-relay", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub run: RunArgs,
}

/// Flags for running the push relay. These are the arguments a service records.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// Address to serve on. Put a TLS proxy in front of it for public use.
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,

    /// Allow plain HTTP and private addresses in UnifiedPush endpoints. Tests and local runs only.
    #[arg(long)]
    pub dev: bool,

    /// Host allowed as a UnifiedPush endpoint. Repeat for more. Without any, any public host works.
    #[arg(long = "ups-allow-host", value_name = "HOST")]
    pub ups_allow_hosts: Vec<String>,

    /// JSON file for the registrations. Read at start, written after each change. Defaults to
    /// `<data-dir>/registrations.json`.
    #[arg(long)]
    pub snapshot: Option<PathBuf>,

    /// Directory for the relay's state: the registrations and logs. Required to install a service.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Write rolling log files here, as well as to the console.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,

    /// APNs `.p8` key file. Turns on APNs. Needs the key id, team id and topic.
    #[arg(long, requires_all = ["apns_key_id", "apns_team_id", "apns_topic"])]
    pub apns_key: Option<PathBuf>,

    /// APNs key id, 10 characters.
    #[arg(long, requires = "apns_key")]
    pub apns_key_id: Option<String>,

    /// Apple team id, 10 characters.
    #[arg(long, requires = "apns_key")]
    pub apns_team_id: Option<String>,

    /// App bundle id, sent as the APNs topic.
    #[arg(long, requires = "apns_key")]
    pub apns_topic: Option<String>,

    /// Use APNs sandbox instead of production.
    #[arg(long, requires = "apns_key")]
    pub apns_sandbox: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install, start, stop, inspect or remove the push relay as a background service.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCmd {
    /// Record the run arguments and install the service. Pass them after `--`. Needs --data-dir.
    Install {
        #[command(flatten)]
        level: LevelArgs,
        /// Linux system service only: run as this user instead of root.
        #[arg(long, value_name = "USER")]
        run_as: Option<String>,
        /// Run arguments, for example `-- --data-dir <dir> --bind 127.0.0.1:8787`.
        #[arg(last = true)]
        run: Vec<String>,
    },
    /// Remove the service. Keeps the data directory.
    Uninstall {
        #[command(flatten)]
        level: LevelArgs,
    },
    /// Start the installed service.
    Start {
        #[command(flatten)]
        level: LevelArgs,
    },
    /// Stop the service. Gives it a few seconds to shut down cleanly.
    Stop {
        #[command(flatten)]
        level: LevelArgs,
    },
    /// Show whether the service is installed and running.
    Status {
        #[command(flatten)]
        level: LevelArgs,
    },
}

#[derive(Debug, Args, Clone, Copy, PartialEq, Eq)]
pub struct LevelArgs {
    /// Install for the logged-in user, not the whole machine.
    #[arg(long, conflicts_with = "system")]
    pub user: bool,
    /// Install for the whole machine. This is the default for the push relay.
    #[arg(long)]
    pub system: bool,
}

impl RunArgs {
    /// The snapshot path: `--snapshot`, else `<data-dir>/registrations.json`, else none.
    pub fn snapshot_path(&self) -> Option<PathBuf> {
        self.snapshot
            .clone()
            .or_else(|| self.data_dir.as_ref().map(|d| d.join(SNAPSHOT_FILE)))
    }

    fn apns_settings(&self) -> Result<Option<ApnsSettings>> {
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

/// Runs the push relay until Ctrl-C, SIGTERM, or a stop request. Used by the console and by the
/// Windows service dispatcher.
pub fn run_blocking(run: RunArgs, stop: Arc<Notify>) -> Result<()> {
    init_tracing(run.log_dir.as_deref());
    if run.dev && !run.bind.ip().is_loopback() {
        tracing::warn!("--dev on a non-loopback address allows private endpoints from anyone");
    }
    let cfg = Config {
        dev: run.dev,
        ups_allow_hosts: run.ups_allow_hosts.clone(),
        snapshot: run.snapshot_path(),
    };
    let apns = run.apns_settings()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?
        .block_on(serve(run.bind, cfg, apns, stop))
}

async fn serve(
    bind: SocketAddr,
    cfg: Config,
    apns: Option<ApnsSettings>,
    stop: Arc<Notify>,
) -> Result<()> {
    let state = build_state(cfg, apns).context("start relay state")?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    println!("Push relay listening on http://{bind}");
    info!(address = %bind, "push relay listening");

    let (close, closed) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = closed.await;
    });
    tokio::select! {
        result = server.into_future() => result.context("serve")?,
        _ = async {
            shutdown_requested(stop).await;
            info!("shutdown requested, stopping");
            let _ = close.send(());
            // Let open requests finish for a moment, then drop whatever is left.
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {}
    }
    Ok(())
}

/// Resolves when Ctrl-C, SIGTERM, or the service stop notification arrives.
async fn shutdown_requested(stop: Arc<Notify>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                    _ = stop.notified() => {}
                }
            }
            Err(_) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = stop.notified() => {}
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = stop.notified() => {}
        }
    }
}

/// The entry point for the Windows service dispatcher. Parses the process arguments.
pub fn run_from_args(args: Vec<String>, stop: Arc<Notify>) -> Result<()> {
    let cli = Cli::try_parse_from(std::iter::once("dissent-push-relay".to_string()).chain(args))
        .context("parse the service's run arguments")?;
    run_blocking(cli.run, stop)
}

/// Console output, and rolling files when a log directory is given.
pub fn init_tracing(log_dir: Option<&std::path::Path>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let file = log_dir
        .and_then(|dir| crate::logs::RollingLog::open(dir, LOG_STEM).ok())
        .map(|log| {
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || log.clone())
        });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file.map(|l| l.boxed()))
        .try_init();
}

/// Runs a service subcommand and prints its result.
pub fn service_command(action: ServiceCmd) -> Result<()> {
    init_tracing(None);
    match action {
        ServiceCmd::Install { level, run, run_as } => {
            let level = level_of(level)?;
            let parsed = parse_recorded(&run)?;
            let data_dir = parsed
                .data_dir
                .clone()
                .context("the recorded arguments must include --data-dir")?;
            let exe = std::env::current_exe().context("find this program")?;
            let args = service::with_log_dir(&run, &data_dir);
            let plan = Plan {
                exe,
                args,
                work_dir: data_dir.clone(),
                writable: vec![data_dir],
                low_port: parsed.bind.port() > 0 && parsed.bind.port() < 1024,
                run_as,
            };
            let out = service::install(&SPEC, level, &plan)?;
            print!("{out}");
            println!("recorded command: {}", service::command_line(&plan));
            Ok(())
        }
        ServiceCmd::Uninstall { level } => {
            print!("{}", service::uninstall(&SPEC, level_of(level)?, None)?);
            Ok(())
        }
        ServiceCmd::Start { level } => {
            print!("{}", service::start(&SPEC, level_of(level)?, None)?);
            Ok(())
        }
        ServiceCmd::Stop { level } => {
            print!("{}", service::stop(&SPEC, level_of(level)?, None)?);
            Ok(())
        }
        ServiceCmd::Status { level } => {
            let st = service::status(&SPEC, level_of(level)?)?;
            println!("installed: {}", if st.installed { "yes" } else { "no" });
            match st.running {
                Some(r) => println!("running: {}", if r { "yes" } else { "no" }),
                None => println!("running: unknown"),
            }
            if let Some(last) = &st.last_start {
                println!("last start: {last}");
            }
            println!("service: {}", st.detail);
            Ok(())
        }
    }
}

fn level_of(args: LevelArgs) -> Result<Level> {
    service::level_for(&SPEC, args.user, args.system)
}

/// Parses the recorded run arguments the same way the service will, so a typo fails at install.
fn parse_recorded(run: &[String]) -> Result<RunArgs> {
    let cli = Cli::try_parse_from(std::iter::once("dissent-push-relay".to_string()).chain(run.iter().cloned()))
        .context("the run arguments do not parse")?;
    if cli.command.is_some() {
        anyhow::bail!("pass run arguments after --, not a subcommand");
    }
    service::check_absolute_paths(run, SPEC.path_flags)?;
    Ok(cli.run)
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
        assert_eq!(cli.run.bind, "127.0.0.1:8787".parse().unwrap());
        assert!(!cli.run.dev);
        assert!(cli.run.apns_settings().unwrap().is_none());
        assert!(cli.run.snapshot_path().is_none());
    }

    #[test]
    fn snapshot_defaults_under_data_dir() {
        let cli = Cli::try_parse_from(["dissent-push-relay", "--data-dir", "/var/lib/push"]).unwrap();
        assert_eq!(
            cli.run.snapshot_path(),
            Some(PathBuf::from("/var/lib/push").join(SNAPSHOT_FILE))
        );
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

    #[test]
    fn service_install_records_run_arguments() {
        let cli = Cli::try_parse_from([
            "dissent-push-relay",
            "service",
            "install",
            "--",
            "--data-dir",
            "/var/lib/push",
            "--bind",
            "127.0.0.1:8787",
        ])
        .unwrap();
        let Some(Command::Service {
            action: ServiceCmd::Install { run, .. },
        }) = cli.command
        else {
            panic!("expected install");
        };
        assert_eq!(run, ["--data-dir", "/var/lib/push", "--bind", "127.0.0.1:8787"]);
    }

    #[test]
    fn recorded_arguments_must_be_absolute_and_parse() {
        let ok = ["--data-dir".to_string(), std::env::temp_dir().to_string_lossy().into_owned()];
        assert!(parse_recorded(&ok).is_ok());
        let relative = ["--data-dir".to_string(), "push".to_string()];
        assert!(parse_recorded(&relative).is_err());
        let typo = ["--data-dri".to_string(), "x".to_string()];
        assert!(parse_recorded(&typo).is_err());
    }
}
