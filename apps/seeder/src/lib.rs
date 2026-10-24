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

use anyhow::{Context, Result, bail};
use clap::Parser;
use dissent_core::{ChannelId, CommunityId, Engine, NodeConfig, RelayChoice};
use dissent_identity::{IdentityError, Vault, keystore};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Identity file, relative to `--data-dir`. Sealed under the vault.
pub const IDENTITY_FILE: &str = "identity.bin";
/// Engine data directory, relative to `--data-dir`.
pub const PROFILE_DIR: &str = "profile";
/// Random salt for the passphrase vault, relative to `--data-dir`. The salt is not secret.
pub const SALT_FILE: &str = "vault.salt";
/// Environment variable with the passphrase, read when no `--passphrase-file` is given.
pub const PASSPHRASE_ENV: &str = "DISSENT_SEEDER_PASSPHRASE";
/// Keychain service name used by `Vault::open`.
const KEYCHAIN_APP: &str = "dissent-seeder";
/// Shortest passphrase accepted. Argon2 does the hard work, but a short phrase is still guessable.
const MIN_PASSPHRASE_CHARS: usize = 8;

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
    /// File whose first line is the passphrase that seals the identity. Used only when the OS
    /// keychain is unavailable, as on most headless Linux servers.
    #[arg(long, value_name = "PATH")]
    pub passphrase_file: Option<PathBuf>,
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
    let passphrase = read_passphrase(cli.passphrase_file.as_deref())?;
    let vault = open_vault(&cli.data_dir, passphrase.as_deref())?;
    let identity = keystore::load_or_create(&cli.data_dir.join(IDENTITY_FILE), now_ms(), &vault)
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
    match Vault::open(KEYCHAIN_APP, &keychain_profile(data_dir)?) {
        Ok(vault) => Ok(vault),
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
    let abs = std::fs::canonicalize(data_dir)
        .with_context(|| format!("resolve data dir {}", data_dir.display()))?;
    let digest = blake3::hash(abs.to_string_lossy().as_bytes());
    Ok(data_encoding::HEXLOWER.encode(&digest.as_bytes()[..8]))
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
    fn cli_parses_passphrase_file() {
        let cli = Cli::try_parse_from([
            "dissent-seeder",
            "--data-dir",
            "d",
            "--passphrase-file",
            "/etc/dissent/seeder.pass",
        ])
        .expect("valid");
        assert_eq!(
            cli.passphrase_file,
            Some(PathBuf::from("/etc/dissent/seeder.pass"))
        );
        assert!(Cli::try_parse_from(["dissent-seeder", "--data-dir", "d"])
            .unwrap()
            .passphrase_file
            .is_none());
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let nanos = now_ms();
        let dir = std::env::temp_dir().join(format!(
            "dissent-seeder-unit-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
}
