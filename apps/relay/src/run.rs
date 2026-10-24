//! The relay's command line, run loop, and service commands.

use crate::service::{self, Level, Plan, Spec};
use crate::{Server, Tls, client_url, dev_config, tls_config};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::info;

/// How the service layer names this program.
pub const SPEC: Spec = Spec {
    name: "dissent-relay",
    title: "Dissent Relay",
    label: "org.dissent.relay",
    description: "Dissent relay: carries encrypted traffic between devices that cannot connect directly",
    path_flags: &["--data-dir", "--cert", "--key", "--acme-cache", "--log-dir"],
    user_by_default: false,
};

/// Log file stem, in the log directory.
const LOG_STEM: &str = "dissent-relay";
/// Longest the shutdown may take before the process gives up on open connections.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(name = "dissent-relay", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub run: RunArgs,
}

/// Flags for running the relay. Pick one mode: `--dev`, `--acme-email`, or `--cert` with `--key`.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// HTTP address to bind. With TLS, it serves only the captive-portal probe.
    #[arg(long, default_value = "0.0.0.0:3340")]
    pub bind: SocketAddr,

    /// HTTPS address to bind. TLS modes only.
    #[arg(long, default_value = "0.0.0.0:443")]
    pub https_bind: SocketAddr,

    /// Serve plain HTTP with no TLS. For local testing only.
    #[arg(long, conflicts_with_all = ["acme_email", "cert", "key"])]
    pub dev: bool,

    /// Public DNS name of this relay. Required with --acme-email. Also used in the printed URL.
    #[arg(long)]
    pub hostname: Option<String>,

    /// Email for the Let's Encrypt account. Gets a certificate for --hostname.
    #[arg(long, requires = "hostname", conflicts_with_all = ["cert", "key"])]
    pub acme_email: Option<String>,

    /// Use the Let's Encrypt staging directory. Its certificates are not trusted by browsers.
    #[arg(long, requires = "acme_email")]
    pub acme_staging: bool,

    /// Directory caching the ACME account and issued certificates. Defaults to `<data-dir>/acme`.
    #[arg(long)]
    pub acme_cache: Option<PathBuf>,

    /// PEM certificate chain. Use with --key.
    #[arg(long, requires = "key", conflicts_with = "acme_email")]
    pub cert: Option<PathBuf>,

    /// PEM private key. Use with --cert.
    #[arg(long, requires = "cert")]
    pub key: Option<PathBuf>,

    /// Directory for the relay's state: ACME cache and logs. Required to install a service.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Write rolling log files here, as well as to the console.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install, start, stop, inspect or remove the relay as a background service.
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
        /// Run arguments, for example `-- --data-dir <dir> --hostname relay.example.org --acme-email <email>`.
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
    /// Install for the whole machine. This is the default for the relay.
    #[arg(long)]
    pub system: bool,
}

/// The TLS mode the flags select.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Dev,
    Tls(Tls),
}

impl RunArgs {
    pub fn mode(&self) -> Result<Mode> {
        if self.dev {
            return Ok(Mode::Dev);
        }
        if let Some(email) = &self.acme_email {
            let Some(hostname) = &self.hostname else {
                bail!("--acme-email needs --hostname");
            };
            return Ok(Mode::Tls(Tls::Acme {
                hostname: hostname.clone(),
                email: email.clone(),
                staging: self.acme_staging,
                cache_dir: self.acme_cache_dir(),
            }));
        }
        if let (Some(cert), Some(key)) = (&self.cert, &self.key) {
            return Ok(Mode::Tls(Tls::Files {
                cert: cert.clone(),
                key: key.clone(),
            }));
        }
        bail!("choose a mode: --dev, --acme-email with --hostname, or --cert with --key");
    }

    /// The ACME cache: `--acme-cache`, else `<data-dir>/acme`, else `acme-cache` in the working
    /// directory.
    pub fn acme_cache_dir(&self) -> PathBuf {
        self.acme_cache
            .clone()
            .or_else(|| self.data_dir.as_ref().map(|d| d.join("acme")))
            .unwrap_or_else(|| PathBuf::from("acme-cache"))
    }

    /// True when a bind address uses a port below 1024, which needs a capability on Linux.
    fn needs_low_port(&self) -> bool {
        let low = |p: u16| (1..1024).contains(&p);
        low(self.bind.port()) || (!self.dev && low(self.https_bind.port()))
    }
}

/// Runs the relay until Ctrl-C, SIGTERM, or a stop request. Used by the console and by the
/// Windows service dispatcher.
pub fn run_blocking(run: RunArgs, stop: Arc<Notify>) -> Result<()> {
    init_tracing(run.log_dir.as_deref());
    // Both TLS providers may be linked, so pick ring explicitly, as the iroh relay binary does.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mode = run.mode()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?
        .block_on(serve(run, mode, stop))
}

async fn serve(run: RunArgs, mode: Mode, stop: Arc<Notify>) -> Result<()> {
    let (config, tls) = match mode {
        Mode::Dev => (dev_config(run.bind), false),
        Mode::Tls(tls) => (
            tls_config(run.bind, run.https_bind, tls)
                .await
                .context("configure TLS")?,
            true,
        ),
    };
    let mut server = Server::spawn(config).await.context("start relay")?;
    let url = client_url(&server, run.hostname.as_deref(), run.https_bind, tls);
    println!("Relay listening. Put this URL in a Dissent client: {url}");
    info!(url = %url, "relay listening");

    tokio::select! {
        _ = shutdown_requested(stop) => info!("shutdown requested, stopping"),
        _ = server.join() => {}
    }
    tokio::time::timeout(SHUTDOWN_GRACE, server.shutdown())
        .await
        .context("relay did not shut down in time")?
        .context("shut down relay")?;
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
    let cli = Cli::try_parse_from(std::iter::once("dissent-relay".to_string()).chain(args))
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
                writable: vec![data_dir.clone(), parsed.acme_cache_dir()],
                low_port: parsed.needs_low_port(),
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
            let level = level_of(level)?;
            let st = service::status(&SPEC, level)?;
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
    let cli = Cli::try_parse_from(std::iter::once("dissent-relay".to_string()).chain(run.iter().cloned()))
        .context("the run arguments do not parse")?;
    if cli.command.is_some() {
        bail!("pass run arguments after --, not a subcommand");
    }
    service::check_absolute_paths(run, SPEC.path_flags)?;
    Ok(cli.run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> RunArgs {
        Cli::try_parse_from(std::iter::once("dissent-relay").chain(args.iter().copied()))
            .expect("valid arguments")
            .run
    }

    #[test]
    fn defaults_bind_3340() {
        let run = parse(&["--dev"]);
        assert_eq!(run.bind, "0.0.0.0:3340".parse().unwrap());
        assert_eq!(run.mode().unwrap(), Mode::Dev);
    }

    #[test]
    fn acme_needs_hostname() {
        assert!(Cli::try_parse_from(["dissent-relay", "--acme-email", "a@b.c"]).is_err());
        let run = parse(&["--acme-email", "a@b.c", "--hostname", "relay.example.org"]);
        assert_eq!(
            run.mode().unwrap(),
            Mode::Tls(Tls::Acme {
                hostname: "relay.example.org".into(),
                email: "a@b.c".into(),
                staging: false,
                cache_dir: PathBuf::from("acme-cache"),
            })
        );
    }

    #[test]
    fn acme_cache_defaults_under_data_dir() {
        let run = parse(&["--acme-email", "a@b.c", "--hostname", "h.example", "--data-dir", "/var/lib/r"]);
        assert_eq!(run.acme_cache_dir(), PathBuf::from("/var/lib/r").join("acme"));
        let given = parse(&[
            "--acme-email", "a@b.c", "--hostname", "h.example", "--data-dir", "/var/lib/r", "--acme-cache", "/c",
        ]);
        assert_eq!(given.acme_cache_dir(), PathBuf::from("/c"));
    }

    #[test]
    fn cert_needs_key_and_pairs_with_files_mode() {
        assert!(Cli::try_parse_from(["dissent-relay", "--cert", "c.pem"]).is_err());
        let run = parse(&["--cert", "c.pem", "--key", "k.pem"]);
        assert_eq!(
            run.mode().unwrap(),
            Mode::Tls(Tls::Files {
                cert: "c.pem".into(),
                key: "k.pem".into(),
            })
        );
    }

    #[test]
    fn dev_conflicts_with_tls_flags() {
        assert!(
            Cli::try_parse_from(["dissent-relay", "--dev", "--cert", "c.pem", "--key", "k.pem"]).is_err()
        );
    }

    #[test]
    fn no_mode_is_an_error() {
        assert!(parse(&[]).mode().is_err());
    }

    #[test]
    fn low_ports_need_the_capability() {
        assert!(parse(&["--cert", "c", "--key", "k", "--https-bind", "0.0.0.0:443"]).needs_low_port());
        assert!(!parse(&["--dev", "--bind", "127.0.0.1:3340"]).needs_low_port());
    }

    #[test]
    fn service_install_parses_recorded_arguments() {
        let cli = Cli::try_parse_from([
            "dissent-relay", "service", "install", "--system", "--", "--data-dir", "/var/lib/r", "--dev",
        ])
        .unwrap();
        let Some(Command::Service {
            action: ServiceCmd::Install { level, run, .. },
        }) = cli.command
        else {
            panic!("expected install");
        };
        assert!(level.system);
        assert_eq!(run, ["--data-dir", "/var/lib/r", "--dev"]);
    }

    #[test]
    fn recorded_arguments_are_validated() {
        let ok = ["--data-dir".to_string(), std::env::temp_dir().to_string_lossy().into_owned(), "--dev".into()];
        assert!(parse_recorded(&ok).is_ok());
        let relative = ["--data-dir".to_string(), "relay".into(), "--dev".into()];
        assert!(parse_recorded(&relative).is_err());
    }
}
