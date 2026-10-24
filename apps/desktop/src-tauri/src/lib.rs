mod commands;
mod events;
mod network;
mod storage;
mod tray;
mod updates;
mod attachments;

use std::path::PathBuf;
use std::sync::Mutex;

use commands::AppState;
use dissent_core::{Engine, NodeConfig};
use dissent_identity::{keystore, Vault};
use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

/// The node config: the saved network settings, or loopback-only when `DISSENT_LOOPBACK=1`.
/// Loopback mode avoids the firewall prompt on Windows and only works for instances on one machine.
pub(crate) fn node_config(settings: &network::NetworkSettings) -> NodeConfig {
    if std::env::var_os("DISSENT_LOOPBACK").is_some_and(|v| v == "1") {
        tracing::warn!("loopback mode is on (DISSENT_LOOPBACK=1): binding 127.0.0.1 only, no relays, no DHT");
        return NodeConfig {
            relay: dissent_core::RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            ..Default::default()
        };
    }
    network::node_config(settings)
}

/// Credential-store entry name for this data dir. The default dir uses "identity". A `DISSENT_DATA_DIR`
/// gets its own entry, so two test instances on one machine don't share a key.
fn vault_profile(data_dir: &std::path::Path) -> String {
    if std::env::var_os("DISSENT_DATA_DIR").is_some_and(|d| !d.is_empty()) {
        let hash = blake3::hash(data_dir.to_string_lossy().as_bytes()).to_hex();
        format!("identity-{}", &hash.as_str()[..16])
    } else {
        "identity".to_string()
    }
}

/// `DISSENT_DATA_DIR` overrides the app data directory, so two instances can run on one machine.
fn data_dir(app: &tauri::App) -> tauri::Result<PathBuf> {
    match std::env::var_os("DISSENT_DATA_DIR") {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => app.path().app_data_dir(),
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mainline=error")))
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = data_dir(app)?;
            std::fs::create_dir_all(&data_dir)?;
            attachments::allow_downloads(app, &data_dir)?;
            tracing::info!(dir = %data_dir.display(), "data dir");

            // chisle: the data key comes from the OS credential store. If none is available we refuse to start
            // rather than write plaintext. Passphrase entry is the planned fallback; it is not built yet.
            let vault = match Vault::open("dissent", &vault_profile(&data_dir)) {
                Ok(vault) => vault,
                Err(e) => {
                    tracing::error!(error = %e, "no credential store for the identity key; refusing to start");
                    return Err(Box::new(e));
                }
            };
            let identity = keystore::load_or_create(&data_dir.join("identity.bin"), commands::now_ms()?, &vault)?;
            let profile = storage::profile_dir(&data_dir, &identity);
            storage::migrate_legacy(&data_dir, &profile)?;
            std::fs::create_dir_all(&profile)?;
            tracing::info!(dir = %profile.display(), "engine data dir");

            let settings = network::load(&data_dir);
            let (engine, rx) = tauri::async_runtime::block_on(Engine::start(
                identity.clone(),
                profile,
                node_config(&settings),
            ))?;

            app.manage(AppState {
                data_dir,
                vault,
                identity: Mutex::new(identity),
                engine: tokio::sync::RwLock::new(Some(engine)),
            });
            events::forward_events(app.handle().clone(), rx);
            tray::install(app)?;
            updates::start_background(app.handle().clone());
            attachments::install(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::identity_status,
            commands::identity_export_phrase,
            commands::identity_import_phrase,
            commands::app_version,
            commands::me,
            commands::set_display_name,
            commands::contact_link,
            commands::add_contact,
            commands::user_profile,
            commands::list_communities,
            commands::create_community,
            commands::create_invite,
            commands::join_community,
            commands::leave_community,
            commands::community_view,
            commands::create_channel,
            commands::moderate,
            commands::claim_handle,
            commands::channel_messages,
            commands::send_message,
            commands::edit_message,
            commands::delete_message,
            commands::list_conversations,
            commands::dm_messages,
            commands::send_dm,
            commands::net_status,
            commands::get_network_settings,
            commands::safety_number,
            updates::check_update,
            updates::install_update,
            attachments::pick_files,
            attachments::reveal_attachment,
            attachments::open_attachment,
            commands::set_network_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Dissent");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            let state = handle.state::<AppState>();
            let engine = tauri::async_runtime::block_on(state.engine.write()).take();
            if let Some(engine) = engine {
                if let Err(e) = tauri::async_runtime::block_on(engine.shutdown()) {
                    tracing::warn!(error = %e, "engine shutdown failed");
                }
            }
        }
    });
}
