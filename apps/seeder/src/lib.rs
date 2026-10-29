//! Headless always-on member. It keeps community history available while the people's own
//! devices are offline. It has no special sync logic: the engine already syncs and serves
//! history, so the seeder only needs to stay online and join the communities it should hold.
//!
//! # Running a seeder
//!
//! The identity is sealed at rest. The key comes from the OS keychain when one is available.
//! Headless Linux servers usually have none, so pass a passphrase instead:
//!
//! ```text
//! dissent-seeder --data-dir /var/lib/dissent-seeder --passphrase-file /etc/dissent/seeder.pass \
//!     --join <invite> --loopback
//! ```
//!
//! The passphrase is the first line of the file, or the `DISSENT_SEEDER_PASSPHRASE` variable. It
//! must be at least 8 characters. The salt is stored in `vault.salt` in the data directory. The
//! salt is not secret, but losing it makes the identity unreadable. Back up the data directory
//! and remember the passphrase. Losing the passphrase loses the seeder's identity. With neither a
//! keychain nor a passphrase, the seeder exits with an error. It never writes the identity
//! unencrypted.
//!
//! # Running as a background service
//!
//! `dissent-seeder service install --user -- --data-dir <abs dir> --join <invite>` records the
//! run arguments and installs a per-user service. It runs at logon, on Windows as a hidden Task
//! Scheduler task, on Linux as a `systemd --user` unit, and on macOS as a LaunchAgent. The
//! credential store is per user, so a user service can use the keychain. `--system` installs a
//! machine-wide service, which has no user keychain. It then needs `--passphrase-file`.
//!
//! `service start`, `service stop`, `service status` and `service uninstall` take the same
//! `--user` or `--system` flag. `stop` and `status` also take `--data-dir`. `status` reads
//! `status.json` from the data directory, which the running seeder rewrites every status interval.
//! Logs rotate at 10 MB and are kept for 7 days in `<data-dir>/logs`.

pub mod logs;
pub mod service;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use dissent_core::{ChannelId, CommunityId, Engine, NodeConfig, RelayChoice};
use dissent_identity::{IdentityError, Vault, keystore};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use tracing::{info, warn};

/// Identity file, relative to `--data-dir`. Sealed under the vault.
pub const IDENTITY_FILE: &str = "identity.bin";
/// Engine data directory, relative to `--data-dir`.
pub const PROFILE_DIR: &str = "profile";
/// Random salt for the passphrase vault, relative to `--data-dir`. The salt is not secret.
pub const SALT_FILE: &str = "vault.salt";
/// Status snapshot, relative to `--data-dir`. Rewritten at every status interval.
pub const STATUS_FILE: &str = "status.json";
/// Environment variable with the passphrase, read when no `--passphrase-file` is given.
pub const PASSPHRASE_ENV: &str = "DISSENT_SEEDER_PASSPHRASE";
/// Keychain service name used by `Vault::open`.
const KEYCHAIN_APP: &str = "dissent-seeder";
/// Shortest passphrase accepted. Argon2 does the hard work, but a short phrase is still guessable.
const MIN_PASSPHRASE_CHARS: usize = 8;
/// Log file stem, in the log directory.
const LOG_STEM: &str = "dissent-seeder";

/// How the service layer names this program.
pub const SPEC: service::Spec = service::Spec {
    name: "dissent-seeder",
    title: "Dissent Seeder",
    label: "org.dissent.seeder",
    description: "Dissent seeder: keeps community history available",
    path_flags: &["--data-dir", "--passphrase-file", "--log-dir"],
    user_by_default: true,
};

#[derive(Debug, Parser)]
#[command(
    name = "dissent-seeder",
    version,
    about = "Headless Dissent member that keeps community history available"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub run: RunArgs,
}

/// Flags for running the seeder. These are the arguments a service records.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// Directory for the identity and engine data. Created if missing. Required to run.
    #[arg(long, env = "DISSENT_SEEDER_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Display name shown to other members.
    #[arg(long, default_value = "seeder", env = "DISSENT_SEEDER_NAME")]
    pub name: String,
    /// Invite link of a community to join. Repeat the flag for more communities.
    #[arg(long, value_name = "INVITE", env = "DISSENT_SEEDER_JOIN", value_delimiter = ',')]
    pub join: Vec<String>,
    /// Bind only to 127.0.0.1, with relays and DHT lookup off. For local tests.
    #[arg(long, env = "DISSENT_SEEDER_LOOPBACK")]
    pub loopback: bool,
    /// Seconds between status lines.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..), env = "DISSENT_SEEDER_STATUS_EVERY")]
    pub status_every: u64,
    /// File whose first line is the passphrase that seals the identity. Used only when the OS
    /// keychain is unavailable, as on most headless Linux servers.
    #[arg(long, value_name = "PATH", env = "DISSENT_SEEDER_PASSPHRASE_FILE")]
    pub passphrase_file: Option<PathBuf>,
    /// Write rolling log files here, as well as to the console.
    #[arg(long, value_name = "DIR", env = "DISSENT_SEEDER_LOG_DIR")]
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install, start, stop, inspect or remove the seeder as a background service.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCmd {
    /// Record the run arguments and install the service. Pass them after `--`.
    Install {
        #[arg(long, conflicts_with = "system")]
        user: bool,
        #[arg(long)]
        system: bool,
        /// Linux system service only: run as this user instead of root.
        #[arg(long, value_name = "USER")]
        run_as: Option<String>,
        /// Run arguments, for example `-- --data-dir <dir> --join <invite>`.
        #[arg(last = true)]
        run: Vec<String>,
    },
    /// Remove the service. Keeps the data directory.
    Uninstall {
        #[command(flatten)]
        level: LevelArgs,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Start the installed service.
    Start {
        #[command(flatten)]
        level: LevelArgs,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Stop the service. Gives it a few seconds to shut down cleanly.
    Stop {
        #[command(flatten)]
        level: LevelArgs,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Show whether the service is installed and running, and what it reports.
    Status {
        #[command(flatten)]
        level: LevelArgs,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Args, Clone, Copy, PartialEq, Eq)]
pub struct LevelArgs {
    /// Use the per-user service. This is the default for the seeder.
    #[arg(long, conflicts_with = "system")]
    pub user: bool,
    /// Use the machine-wide service.
    #[arg(long)]
    pub system: bool,
}

/// Network settings for the seeder. `loopback` keeps every socket on this machine.
pub fn node_config(loopback: bool) -> NodeConfig {
    if loopback {
        NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            serves_lookups: true,
            ..NodeConfig::default()
        }
    } else {
        NodeConfig {
            serves_lookups: true,
            ..NodeConfig::default()
        }
    }
}

/// What `status.json` holds. Written by the running seeder, read by `service status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    pub pid: u32,
    pub started_ms: u64,
    pub updated_ms: u64,
    pub peers: usize,
    pub communities: Vec<CommunityLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityLine {
    pub name: String,
    pub id: String,
    pub members: usize,
    pub messages: usize,
    /// Whether the owner lets this seeder forward the server's voice calls ("Seeder may forward
    /// calls"). Absent from a status file written before it, which reads as off.
    #[serde(default)]
    pub forwards_calls: bool,
    /// Whether an admin granted this seeder storage in the server (`StorageGrant`). It asks for one
    /// when it joins; a storage join does not need one yet. Absent from an older status file: off.
    #[serde(default)]
    pub storage_granted: bool,
}

/// Runs the seeder until Ctrl-C, SIGTERM, or a stop request. Called by the console and by the
/// Windows service dispatcher, which passes `stop` when the service is asked to stop.
pub fn run_blocking(run: RunArgs, stop: Arc<Notify>) -> Result<()> {
    init_tracing(run.log_dir.as_deref());
    let data_dir = run.data_dir.clone().context("--data-dir is required to run the seeder")?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?
        .block_on(run_seeder(run, data_dir, stop))
}

/// The entry point for the Windows service dispatcher. Parses the process arguments.
pub fn run_from_args(args: Vec<String>, stop: Arc<Notify>) -> Result<()> {
    let cli = Cli::try_parse_from(std::iter::once("dissent-seeder".to_string()).chain(args))
        .context("parse the service's run arguments")?;
    run_blocking(cli.run, stop)
}

/// Writes log lines to the console, and to rolling files when a log directory is given.
pub fn init_tracing(log_dir: Option<&Path>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let file = log_dir
        .and_then(|dir| logs::RollingLog::open(dir, LOG_STEM).ok())
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

async fn run_seeder(run: RunArgs, data_dir: PathBuf, stop: Arc<Notify>) -> Result<()> {
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    // A stop request left over from an earlier run is stale.
    let _ = std::fs::remove_file(data_dir.join(service::STOP_FILE));
    let passphrase = read_passphrase(run.passphrase_file.as_deref())?;
    let vault = open_vault(&data_dir, passphrase.as_deref())?;
    let identity = keystore::load_or_create(&data_dir.join(IDENTITY_FILE), now_ms(), &vault)
        .context("load or create identity")?;
    let started_ms = now_ms();
    let (engine, _events) = Engine::start(
        identity,
        data_dir.join(PROFILE_DIR),
        node_config(run.loopback),
        Arc::new(vault),
    )
        .await
        .context("start engine")?;
    engine
        .set_display_name(run.name.clone())
        .await
        .context("set display name")?;

    // A seeder joins as storage. It keeps and serves sealed history, but it is never added to a
    // community's encryption group, so it cannot read messages or attachments.
    for invite in &run.join {
        match engine.join_as_storage(invite).await {
            Ok(summary) => info!(community = %summary.name, id = %summary.id, "joined as storage"),
            Err(e) => warn!(error = %e, "could not join community, continuing"),
        }
    }

    let mut ticks = tokio::time::interval(Duration::from_secs(run.status_every));
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown_requested(stop.clone()) => {
                info!("shutdown requested, stopping");
                break;
            }
            _ = poll.tick() => {
                if data_dir.join(service::STOP_FILE).exists() {
                    info!("stop request found, stopping");
                    break;
                }
            }
            _ = ticks.tick() => {
                let status = collect_status(&engine, started_ms).await;
                log_status(&status);
                if let Err(e) = write_status(&data_dir, &status) {
                    warn!(error = %e, "could not write status.json");
                }
            }
        }
    }
    engine.shutdown().await.context("shut down engine")?;
    let _ = std::fs::remove_file(data_dir.join(service::STOP_FILE));
    Ok(())
}

/// Resolves when Ctrl-C, SIGTERM, or the service stop notification arrives.
async fn shutdown_requested(stop: Arc<Notify>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = stop.notified() => {}
                }
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = stop.notified() => {}
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

/// The current counts for every community, and the peer count.
pub async fn collect_status(engine: &Engine, started_ms: u64) -> StatusFile {
    let mut communities = Vec::new();
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
        communities.push(CommunityLine {
            name: summary.name,
            id: summary.id,
            members: view.members.len(),
            messages,
            forwards_calls: engine.seeder_forwards_calls(id),
            storage_granted: engine.holds_storage_grant(id),
        });
    }
    let net = engine.net_status().await;
    StatusFile {
        pid: std::process::id(),
        started_ms,
        updated_ms: now_ms(),
        peers: net.peers,
        communities,
    }
}

/// Logs one line per community, then one for the network.
pub fn log_status(status: &StatusFile) {
    for c in &status.communities {
        info!(
            community = %c.name,
            id = %c.id,
            members = c.members,
            messages = c.messages,
            forwards_calls = c.forwards_calls,
            storage_granted = c.storage_granted,
            "community status"
        );
    }
    info!(peers = status.peers, communities = status.communities.len(), "network status");
}

/// Writes `status.json` through a temporary file, so a reader never sees a half-written file.
pub fn write_status(data_dir: &Path, status: &StatusFile) -> Result<()> {
    let text = serde_json::to_string_pretty(status).context("encode status")?;
    let tmp = data_dir.join(format!("{STATUS_FILE}.tmp"));
    std::fs::write(&tmp, text).context("write status")?;
    std::fs::rename(&tmp, data_dir.join(STATUS_FILE)).context("replace status.json")
}

/// Reads `status.json`. `None` when the seeder has not written one yet.
pub fn read_status(data_dir: &Path) -> Result<Option<StatusFile>> {
    let path = data_dir.join(STATUS_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).context("read status.json")?;
    Ok(Some(serde_json::from_str(&text).context("parse status.json")?))
}

/// Runs a service subcommand and prints its result.
pub fn service_command(action: ServiceCmd) -> Result<()> {
    init_tracing(None);
    match action {
        ServiceCmd::Install { user, system, run, run_as } => {
            let level = service::level_for(&SPEC, user, system)?;
            let parsed = parse_recorded(&run)?;
            let data_dir = parsed
                .data_dir
                .clone()
                .context("the recorded arguments must include --data-dir")?;
            if level == service::Level::System && parsed.passphrase_file.is_none() {
                bail!(
                    "a system service has no user keychain. Add --passphrase-file <path> to the run arguments"
                );
            }
            let exe = std::env::current_exe().context("find this program")?;
            let args = service::with_log_dir(&run, &data_dir);
            let plan = service::Plan {
                exe,
                args,
                work_dir: data_dir.clone(),
                writable: vec![data_dir],
                low_port: false,
                run_as,
            };
            let out = service::install(&SPEC, level, &plan)?;
            print!("{out}");
            println!("recorded command: {}", service::command_line(&plan));
            Ok(())
        }
        ServiceCmd::Uninstall { level, data_dir } => {
            let level = level_of(&SPEC, level)?;
            print!("{}", service::uninstall(&SPEC, level, data_dir.as_deref())?);
            Ok(())
        }
        ServiceCmd::Start { level, data_dir } => {
            let level = level_of(&SPEC, level)?;
            print!("{}", service::start(&SPEC, level, data_dir.as_deref())?);
            Ok(())
        }
        ServiceCmd::Stop { level, data_dir } => {
            let level = level_of(&SPEC, level)?;
            print!("{}", service::stop(&SPEC, level, data_dir.as_deref())?);
            Ok(())
        }
        ServiceCmd::Status { level, data_dir } => {
            let level = level_of(&SPEC, level)?;
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
            match data_dir {
                Some(dir) => match read_status(&dir)? {
                    Some(s) => {
                        let age = now_ms().saturating_sub(s.updated_ms) / 1000;
                        println!("communities: {} (status written {age} s ago, pid {})", s.communities.len(), s.pid);
                        for c in &s.communities {
                            println!(
                                "  {}: {} members, {} messages{}{}",
                                c.name,
                                c.members,
                                c.messages,
                                if c.storage_granted { "" } else { ", no storage grant yet" },
                                if c.forwards_calls { ", forwards calls" } else { "" }
                            );
                        }
                    }
                    None => println!("communities: no status.json in {} yet", dir.display()),
                },
                None => println!("communities: pass --data-dir to read status.json"),
            }
            Ok(())
        }
    }
}

fn level_of(spec: &service::Spec, args: LevelArgs) -> Result<service::Level> {
    service::level_for(spec, args.user, args.system)
}

/// Parses the recorded run arguments the same way the service will, so a typo fails at install.
fn parse_recorded(run: &[String]) -> Result<RunArgs> {
    let cli = Cli::try_parse_from(std::iter::once("dissent-seeder".to_string()).chain(run.iter().cloned()))
        .context("the run arguments do not parse")?;
    if cli.command.is_some() {
        bail!("pass run arguments after --, not a subcommand");
    }
    service::check_absolute_paths(run, SPEC.path_flags)?;
    Ok(cli.run)
}

/// The passphrase from `path`, or else from the environment. `None` when neither is set.
pub fn read_passphrase(path: Option<&Path>) -> Result<Option<String>> {
    if let Some(path) = path {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read passphrase file {}", path.display()))?;
        return Ok(Some(text.lines().next().unwrap_or("").to_string()));
    }
    Ok(std::env::var(PASSPHRASE_ENV).ok())
}

/// Opens the vault for `data_dir`. Tries the OS keychain first. When the keychain is unavailable,
/// uses `passphrase`. When neither works, it fails. It never falls back to plaintext.
pub fn open_vault(data_dir: &Path, passphrase: Option<&str>) -> Result<Vault> {
    let name = keychain_profile(data_dir)?;
    let canonical = legacy_keychain_profile(data_dir)?;
    let spelled: Vec<String> = dissent_identity::vault::spellings(data_dir).iter().map(|t| profile_name(t)).collect();
    open_vault_in(KEYCHAIN_APP, data_dir, &name, &canonical, &spelled, passphrase)
}

/// `open_vault` for one credential-store app and the names it should use. Tests pass their own app.
fn open_vault_in(
    app: &str,
    data_dir: &Path,
    name: &str,
    canonical: &str,
    spelled: &[String],
    passphrase: Option<&str>,
) -> Result<Vault> {
    let identity = data_dir.join(IDENTITY_FILE);
    // A new key beside sealed data would leave that data unreadable, so it is refused before any key is made.
    if !identity.exists() && dissent_core::sealed_data_present(data_dir) {
        bail!(dissent_core::SEALED_WITHOUT_IDENTITY);
    }
    let keychain = if identity.exists() {
        let legacy: Vec<&str> = std::iter::once(canonical).chain(spelled.iter().map(String::as_str)).collect();
        choose_keychain_key(app, &identity, name, &legacy)
    } else {
        if let Err(e) = Vault::adopt_key(app, canonical, name) {
            warn!(error = %e, "could not move the seeder key to its new name");
        }
        Vault::open(app, name).map(Some)
    };
    // A sealed identity already on disk must be opened with its key or the passphrase. A new key cannot.
    match keychain {
        Ok(Some(vault)) => Ok(vault),
        Ok(None) => match passphrase {
            Some(passphrase) => passphrase_vault(data_dir, passphrase),
            None => bail!(
                "this profile's key isn't in this computer's credential store, and no passphrase was given. \
                 Pass --passphrase-file <path>, or set {PASSPHRASE_ENV}."
            ),
        },
        Err(IdentityError::Keychain(reason)) => {
            info!(reason = %reason, "OS keychain unavailable, using a passphrase");
            let Some(passphrase) = passphrase else {
                bail!(
                    "no OS keychain is available ({reason}), and no passphrase was given. \
                     Pass --passphrase-file <path>, or set {PASSPHRASE_ENV}. \
                     The identity is never stored unencrypted."
                );
            };
            passphrase_vault(data_dir, passphrase)
        }
        Err(e) => Err(e).context("open keychain vault"),
    }
}

/// The first keychain entry that opens the identity, tried in order: the normalised name, then each legacy name.
/// The normalised entry stays as it is when it opens the identity. A legacy key that opens it is copied to the
/// normalised name, which did not open it. The old entry is kept.
fn choose_keychain_key(app: &str, identity: &Path, name: &str, legacy: &[&str]) -> Result<Option<Vault>, IdentityError> {
    for candidate in std::iter::once(name).chain(legacy.iter().copied().filter(|c| *c != name)) {
        let Some(vault) = keychain_opens(app, identity, candidate)? else {
            continue;
        };
        if candidate != name {
            if let Err(e) = Vault::replace_key(app, candidate, name) {
                warn!(error = %e, "could not copy the seeder key to its new name");
            }
        }
        return Ok(Some(vault));
    }
    Ok(None)
}

/// The key in the keychain entry `profile`, when it opens the identity file. A keychain error is returned.
fn keychain_opens(app: &str, identity: &Path, profile: &str) -> Result<Option<Vault>, IdentityError> {
    let Some(vault) = Vault::open_existing(app, profile)? else {
        return Ok(None);
    };
    Ok(keystore::load(identity, &vault).is_ok().then_some(vault))
}

/// A vault derived from `passphrase` and the salt in `data_dir`. The salt is created on first use
/// and reused after that, so the same passphrase always gives the same vault.
pub fn passphrase_vault(data_dir: &Path, passphrase: &str) -> Result<Vault> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        bail!("the passphrase must be at least {MIN_PASSPHRASE_CHARS} characters");
    }
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    let salt = load_or_create_salt(data_dir)?;
    Vault::from_passphrase(passphrase, &salt).context("derive vault from passphrase")
}

fn load_or_create_salt(data_dir: &Path) -> Result<[u8; 16]> {
    let path = data_dir.join(SALT_FILE);
    if path.exists() {
        let raw = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        return <[u8; 16]>::try_from(raw.as_slice())
            .map_err(|_| anyhow::anyhow!("{} is not 16 bytes", path.display()));
    }
    let mut salt = [0u8; 16];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| anyhow::anyhow!("random number generator failed"))?;
    std::fs::write(&path, salt).with_context(|| format!("write {}", path.display()))?;
    Ok(salt)
}

/// A stable keychain profile for `data_dir`: the first 8 bytes of the hash of its absolute path,
/// in hex. Two seeders on one machine then get separate keys.
fn keychain_profile(data_dir: &Path) -> Result<String> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    Ok(profile_name(&dissent_identity::vault::normal_dir(data_dir)))
}

/// The name before `normal_dir`, from the canonical path. Kept so an existing key is found and moved.
fn legacy_keychain_profile(data_dir: &Path) -> Result<String> {
    let abs = std::fs::canonicalize(data_dir)
        .with_context(|| format!("resolve data dir {}", data_dir.display()))?;
    Ok(profile_name(&abs.to_string_lossy()))
}

fn profile_name(text: &str) -> String {
    let digest = blake3::hash(text.as_bytes());
    data_encoding::HEXLOWER.encode(&digest.as_bytes()[..8])
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

    /// Touches the credential store under a test app name, so it is ignored by default. A real identity sealed under
    /// one spelling's key, a stray empty key under another spelling, and the normalised name missing: the real key is
    /// chosen whichever order the legacy names come in, and it is copied to the normalised name.
    #[test]
    #[ignore]
    fn the_seeder_key_that_opens_the_identity_wins_over_a_stray_one() {
        use dissent_identity::Identity;
        const APP: &str = "dissent-seeder-test";
        let dir = std::env::temp_dir().join(format!("dissent-seeder-choose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (norm, back, fwd) = ("seed-norm-test", "seed-back-test", "seed-fwd-test");
        let forget = |names: &[&str]| {
            for name in names {
                let _ = Vault::delete_key(APP, name);
            }
        };
        forget(&[norm, back, fwd]);
        let real = Vault::open(APP, fwd).unwrap();
        keystore::save(&Identity::create(1), &dir.join(IDENTITY_FILE), &real).unwrap();
        let _stray = Vault::open(APP, back).unwrap();
        for order in [vec![back.to_string(), fwd.to_string()], vec![fwd.to_string(), back.to_string()]] {
            forget(&[norm]);
            let vault = open_vault_in(APP, &dir, norm, "unused-canonical", &order, None).unwrap();
            assert!(keystore::load(&dir.join(IDENTITY_FILE), &vault).is_ok(), "the chosen key opens the identity");
            let copied = Vault::open_existing(APP, norm).unwrap().expect("copied to the normalised name");
            assert!(keystore::load(&dir.join(IDENTITY_FILE), &copied).is_ok());
        }
        forget(&[norm, back, fwd]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Touches the credential store under a test app name, so it is ignored by default. When no entry opens the
    /// identity and no passphrase is given, the open refuses and creates no entry under the normalised name.
    #[test]
    #[ignore]
    fn sealed_identity_that_no_key_opens_refuses_and_creates_nothing() {
        const APP: &str = "dissent-seeder-test";
        let dir = std::env::temp_dir().join(format!("dissent-seeder-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (norm, stray) = ("seed-refuse-norm-test", "seed-refuse-stray-test");
        let _ = Vault::delete_key(APP, norm);
        let _ = Vault::delete_key(APP, stray);
        std::fs::write(dir.join(IDENTITY_FILE), b"sealed").unwrap();
        let _ = Vault::open(APP, stray).unwrap();
        let err = open_vault_in(APP, &dir, norm, "unused-canonical", &[stray.to_string()], None).expect_err("refused");
        assert!(err.to_string().contains("isn't in this computer's credential store"), "{err}");
        assert!(Vault::open_existing(APP, norm).unwrap().is_none(), "no key was created");
        let _ = Vault::delete_key(APP, stray);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("dissent-seeder").chain(args.iter().copied())).expect("valid")
    }

    #[test]
    #[ignore] // uses the OS credential store, which CI's Linux runner lacks
    fn a_missing_identity_beside_sealed_data_is_refused_and_no_key_is_made() {
        const APP: &str = "dissent-seeder-test";
        let dir = std::env::temp_dir().join(format!("dissent-seeder-sealed-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("profiles").join("root")).unwrap();
        std::fs::write(dir.join("profiles").join("root").join("store.bin"), b"sealed").unwrap();
        let name = "dissent-seeder-test-sealed";
        let err = open_vault_in(APP, &dir, name, "unused-canonical", &[], None).expect_err("refused");
        assert!(err.to_string().contains("identity.bin is missing"), "{err}");
        assert!(dissent_identity::Vault::open_existing(APP, name).unwrap().is_none(), "no key was made under the test name");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cli_parses_flags() {
        let cli = parse(&[
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
        ]);
        assert!(cli.command.is_none());
        assert_eq!(cli.run.data_dir, Some(PathBuf::from("/var/lib/seeder")));
        assert_eq!(cli.run.name, "relay-seed");
        assert_eq!(cli.run.join, ["dissent://invite/one", "dissent://invite/two"]);
        assert!(cli.run.loopback);
        assert_eq!(cli.run.status_every, 15);
    }

    #[test]
    fn cli_defaults() {
        let cli = parse(&["--data-dir", "d"]);
        assert_eq!(cli.run.name, "seeder");
        assert!(cli.run.join.is_empty());
        assert!(!cli.run.loopback);
        assert_eq!(cli.run.status_every, 60);
    }

    #[test]
    fn data_dir_is_checked_when_running_not_when_parsing() {
        // `service status` needs no data dir, so parsing alone must not require one.
        assert!(Cli::try_parse_from(["dissent-seeder"]).is_ok());
        assert!(
            Cli::try_parse_from(["dissent-seeder", "--data-dir", "d", "--status-every", "0"]).is_err()
        );
    }

    #[test]
    fn cli_parses_passphrase_file() {
        let cli = parse(&["--data-dir", "d", "--passphrase-file", "/etc/dissent/seeder.pass"]);
        assert_eq!(cli.run.passphrase_file, Some(PathBuf::from("/etc/dissent/seeder.pass")));
        assert!(parse(&["--data-dir", "d"]).run.passphrase_file.is_none());
    }

    #[test]
    fn cli_parses_service_install_with_recorded_arguments() {
        let cli = parse(&[
            "service",
            "install",
            "--user",
            "--",
            "--data-dir",
            "/var/lib/seeder",
            "--join",
            "dissent://invite/one",
            "--loopback",
        ]);
        let Some(Command::Service {
            action: ServiceCmd::Install { user, system, run, .. },
        }) = cli.command
        else {
            panic!("expected service install");
        };
        assert!(user && !system);
        assert_eq!(
            run,
            ["--data-dir", "/var/lib/seeder", "--join", "dissent://invite/one", "--loopback"]
        );
    }

    #[test]
    fn cli_parses_service_status_with_level_and_data_dir() {
        let cli = parse(&["service", "status", "--system", "--data-dir", "/var/lib/seeder"]);
        let Some(Command::Service {
            action: ServiceCmd::Status { level, data_dir },
        }) = cli.command
        else {
            panic!("expected service status");
        };
        assert!(level.system && !level.user);
        assert_eq!(data_dir, Some(PathBuf::from("/var/lib/seeder")));
    }

    #[test]
    fn recorded_arguments_are_validated_at_install() {
        let absolute = std::env::temp_dir().to_string_lossy().into_owned();
        let ok = ["--data-dir".to_string(), absolute.clone()];
        assert!(parse_recorded(&ok).is_ok());
        let typo = ["--data-dri".to_string(), absolute];
        assert!(parse_recorded(&typo).is_err());
        let relative = ["--data-dir", "seeder"].map(String::from);
        assert!(parse_recorded(&relative).is_err());
    }

    #[test]
    fn status_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("dissent-seeder-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let status = StatusFile {
            pid: 42,
            started_ms: 1,
            updated_ms: 2,
            peers: 3,
            communities: vec![CommunityLine {
                name: "Rustaceans".into(),
                id: "abc".into(),
                members: 4,
                messages: 5,
                forwards_calls: true,
                storage_granted: true,
            }],
        };
        write_status(&dir, &status).unwrap();
        assert_eq!(read_status(&dir).unwrap(), Some(status));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn passphrase_identity_reloads_and_wrong_passphrase_fails() {
        let dir = scratch_dir("reload");
        let right = "correct horse battery";
        let vault = passphrase_vault(&dir, right).expect("vault");
        let id = dissent_identity::Identity::create(now_ms());
        keystore::save(&id, &dir.join(IDENTITY_FILE), &vault).expect("save");

        let again = passphrase_vault(&dir, right).expect("same passphrase, same salt");
        let loaded = keystore::load(&dir.join(IDENTITY_FILE), &again).expect("reloads");
        assert_eq!(loaded.device_id(), id.device_id());

        let wrong = passphrase_vault(&dir, "wrong horse battery").expect("derives");
        assert!(
            keystore::load(&dir.join(IDENTITY_FILE), &wrong).is_err(),
            "a wrong passphrase must not open the identity"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identity_file_holds_no_plaintext_seed() {
        let dir = scratch_dir("plaintext");
        let vault = passphrase_vault(&dir, "a long enough phrase").expect("vault");
        let id = dissent_identity::Identity::create(now_ms());
        let path = dir.join(IDENTITY_FILE);
        keystore::save(&id, &path, &vault).expect("save");
        let bytes = std::fs::read(&path).unwrap();
        let seed = id.device.0.seed();
        assert!(
            !bytes.windows(seed.len()).any(|w| w == seed),
            "device seed found in clear on disk"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn salt_is_created_once_and_reused() {
        let dir = scratch_dir("salt");
        let first = load_or_create_salt(&dir).unwrap();
        let second = load_or_create_salt(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(dir.join(SALT_FILE)).unwrap().len(), 16);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn short_passphrase_is_refused() {
        let dir = scratch_dir("short");
        assert!(passphrase_vault(&dir, "short").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn passphrase_file_uses_first_line() {
        let dir = scratch_dir("pass-file");
        let path = dir.join("pass");
        std::fs::write(&path, "first line phrase\nsecond\n").unwrap();
        assert_eq!(
            read_passphrase(Some(&path)).unwrap().as_deref(),
            Some("first line phrase")
        );
        assert!(read_passphrase(Some(&dir.join("missing"))).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loopback_config_disables_relay_and_dht() {
        let cfg = node_config(true);
        assert!(matches!(cfg.relay, RelayChoice::Disabled));
        assert!(cfg.loopback_only);
        assert!(!cfg.dht_lookup);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dissent-seeder-unit-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
