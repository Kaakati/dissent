mod commands;
mod events;
mod storage;

use std::path::PathBuf;
use std::sync::Mutex;

use commands::AppState;
use dissent_core::{Engine, NodeConfig, RelayChoice};
use dissent_identity::keystore;
use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

/// Production defaults, or loopback-only with no relays and no DHT when `DISSENT_LOOPBACK=1`.
/// Loopback mode avoids the firewall prompt on Windows and only works for instances on one machine.
pub(crate) fn node_config() -> NodeConfig {
    if std::env::var_os("DISSENT_LOOPBACK").is_some_and(|v| v == "1") {
        tracing::warn!("loopback mode is on (DISSENT_LOOPBACK=1): binding 127.0.0.1 only, no relays, no DHT");
        return NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            ..Default::default()
        };
    }
    NodeConfig::default()
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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let app = tauri::Builder::default()
        .setup(|app| {
            let data_dir = data_dir(app)?;
            std::fs::create_dir_all(&data_dir)?;
            tracing::info!(dir = %data_dir.display(), "data dir");

            let identity = keystore::load_or_create(&data_dir.join("identity.bin"), commands::now_ms()?)?;
            let profile = storage::profile_dir(&data_dir, &identity);
            storage::migrate_legacy(&data_dir, &profile)?;
            std::fs::create_dir_all(&profile)?;
            tracing::info!(dir = %profile.display(), "engine data dir");

            let (engine, rx) = tauri::async_runtime::block_on(Engine::start(
                identity.clone(),
                profile,
                node_config(),
            ))?;

            app.manage(AppState {
                data_dir,
                identity: Mutex::new(identity),
                engine: tokio::sync::RwLock::new(Some(engine)),
            });
            events::forward_events(app.handle().clone(), rx);
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
