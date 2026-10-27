//! The relay for a host's own servers, run as a service. It joins each server as storage, as the seeder
//! does, so it learns the devices of the members. It admits only those devices, and it keeps the list
//! current as members join, leave and are banned. See `hosted` and `docs/design/06-threat-model.md`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use dissent_core::Engine;
use dissent_identity::keystore;
use dissent_seeder::{IDENTITY_FILE, PROFILE_DIR, node_config, open_vault, read_passphrase};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::hosted::{self, Caps, ConfigBuilder, HostedAccess, NodeKey, RelayHost};
use crate::run::{Mode, RunArgs, shutdown_requested};
use crate::{dev_config, tls_config};

/// How often the allow-list is compared with the community state. A join, leave or ban takes effect
/// within this time, and a ban that cuts an open connection restarts the relay at the same time.
const MEMBER_SYNC: Duration = Duration::from_secs(5);

/// Runs the members-only relay until `stop` is set. The data directory holds the relay's own identity
/// and its community state, and the passphrase, if any, seals the identity.
pub async fn serve(run: &RunArgs, mode: &Mode, stop: Arc<Notify>) -> Result<()> {
    let data_dir = run
        .data_dir
        .clone()
        .context("--join needs --data-dir, for the relay's identity and community state")?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    let passphrase = read_passphrase(run.passphrase_file.as_deref())?;
    let vault = open_vault(&data_dir, passphrase.as_deref())?;
    let identity = keystore::load_or_create(&data_dir.join(IDENTITY_FILE), now_ms(), &vault)
        .context("load or create the relay's identity")?;
    let (engine, _events) = Engine::start(
        identity,
        data_dir.join(PROFILE_DIR),
        node_config(false),
        Arc::new(vault),
    )
    .await
    .context("start the engine")?;
    engine
        .set_display_name("relay".into())
        .await
        .context("set display name")?;
    // Joins as storage: it keeps the community's sealed history and is never in its encryption group.
    for invite in &run.join {
        match engine.join_as_storage(invite).await {
            Ok(summary) => info!(community = %summary.name, "joined as storage, for the relay's members"),
            Err(e) => warn!(error = %e, "could not join community, continuing"),
        }
    }

    let caps = Caps {
        total_bytes_per_second: run.bandwidth,
        max_connections: run.max_connections,
    };
    caps.validate().map_err(anyhow::Error::msg)?;
    let access = HostedAccess::new(&caps, member_keys(&engine));
    let host = RelayHost::start(config_builder(run, mode, caps), access)
        .await
        .map_err(anyhow::Error::msg)?;
    if let Some(addr) = host.addr() {
        println!("Relay for your servers listening on {addr}.");
    }
    info!(data_dir = %data_dir.display(), "relay for your servers running");

    let mut host = host;
    let mut sync = tokio::time::interval(MEMBER_SYNC);
    loop {
        tokio::select! {
            _ = shutdown_requested(stop.clone()) => {
                info!("shutdown requested, stopping");
                break;
            }
            _ = sync.tick() => {
                let closed = host.set_members(member_keys(&engine));
                if closed > 0 {
                    info!(closed, "closed the connections of removed members");
                }
            }
        }
    }
    host.stop().await;
    engine.shutdown().await.context("shut down the engine")?;
    Ok(())
}

/// The endpoint keys of the devices that may use the relay right now.
fn member_keys(engine: &Engine) -> Vec<NodeKey> {
    engine.relay_member_devices().into_iter().map(|d| d.0).collect()
}

/// Builds the server config for the run's TLS mode, with the access hook and the caps.
fn config_builder(run: &RunArgs, mode: &Mode, caps: Caps) -> ConfigBuilder {
    let bind = run.bind;
    let https_bind = run.https_bind;
    let mode = mode.clone();
    Box::new(move |access| {
        let mode = mode.clone();
        Box::pin(async move {
            let mut config = match mode {
                Mode::Dev => dev_config(bind),
                Mode::Tls(tls) => tls_config(bind, https_bind, tls)
                    .await
                    .map_err(|e| format!("configure TLS: {e:#}"))?,
            };
            hosted::apply(&caps, access, &mut config);
            Ok(config)
        })
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

