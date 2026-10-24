mod commands;
mod events;
mod network;
mod storage;
mod tray;
mod updates;
mod attachments;
mod voice;
mod transfers_cmds;
mod part6;
mod contacts_cmds;
mod status_cmds;

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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mainline=error,mainline::rpc=off")))
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
            let vault = std::sync::Arc::new(vault);
            let profile = storage::profile_dir(&data_dir, &identity);
            storage::migrate_legacy(&data_dir, &profile)?;
            std::fs::create_dir_all(&profile)?;
            tracing::info!(dir = %profile.display(), "engine data dir");

            let settings = network::load(&data_dir);
            let voice = voice::Voice::new(data_dir.clone());
            let (engine, rx) = tauri::async_runtime::block_on(Engine::start(
                identity.clone(),
                profile,
                node_config(&settings),
                vault.clone(),
            ))?;
            voice::prepare(&engine, &voice);
            app.manage(voice);

            app.manage(part6::PendingLink::default());
            app.manage(AppState {
                data_dir,
                vault,
                identity: Mutex::new(identity),
                engine: tokio::sync::RwLock::new(Some(engine)),
            });
            events::forward_events(app.handle().clone(), rx);
            app.manage(status_cmds::LastInput::new());
            status_cmds::start_idle_watch(app.handle().clone());
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
            commands::send_reply,
            voice::audio_devices,
            voice::set_audio_devices,
            voice::get_cascade_at,
            voice::set_cascade_at,
            transfers_cmds::transfer_settings,
            transfers_cmds::set_transfer_settings,
            transfers_cmds::store_stats,
            transfers_cmds::pick_download_dir,
            transfers_cmds::send_files,
            transfers_cmds::send_dm_files,
            transfers_cmds::list_transfers,
            transfers_cmds::download_attachment,
            transfers_cmds::pause_transfer,
            transfers_cmds::resume_transfer,
            transfers_cmds::retry_transfer,
            transfers_cmds::cancel_transfer,
            transfers_cmds::remove_transfer,
            transfers_cmds::create_group_invite,
            transfers_cmds::group_invite_link,
            transfers_cmds::revoke_group_invites,
            transfers_cmds::join_group_by_invite,
            transfers_cmds::group_join_requests,
            transfers_cmds::approve_group_join,
            transfers_cmds::decline_group_join,
            transfers_cmds::pending_group_joins,
            transfers_cmds::link_previews,
            transfers_cmds::set_link_previews,
            contacts_cmds::pinned_conversations,
            contacts_cmds::list_contacts,
            contacts_cmds::send_contact_request,
            contacts_cmds::accept_contact,
            contacts_cmds::decline_contact,
            contacts_cmds::cancel_contact_request,
            contacts_cmds::remove_contact,
            contacts_cmds::block_user,
            contacts_cmds::unblock_user,
            contacts_cmds::peer_contact_link,
            contacts_cmds::dm_policy,
            contacts_cmds::set_dm_policy,
            contacts_cmds::create_group,
            contacts_cmds::group_messages,
            contacts_cmds::send_group_message,
            contacts_cmds::add_group_members,
            contacts_cmds::leave_group,
            commands::react,
            commands::unreact,
            commands::list_conversations,
            commands::dm_messages,
            commands::send_dm,
            commands::net_status,
            status_cmds::set_status,
            status_cmds::get_status,
            status_cmds::record_activity,
            commands::get_network_settings,
            commands::safety_number,
            updates::check_update,
            updates::install_update,
            attachments::pick_files,
            attachments::reveal_attachment,
            attachments::open_attachment,
            commands::set_network_settings,
            part6::capabilities,
            part6::set_note,
            part6::pin_conversation,
            part6::mark_read,
            part6::pinned_messages,
            part6::pin_message,
            part6::mark_safety_verified,
            part6::start_call,
            part6::join_call,
            part6::leave_call,
            part6::call_state,
            part6::set_muted,
            part6::set_deafened,
            part6::set_camera,
            part6::send_call_message,
            part6::accept_call,
            part6::decline_call,
            part6::join_group_call,
            part6::join_dm_call,
            part6::complete_first_run,
            part6::read_markers,
            part6::close_conversation,
            part6::reopen_conversation,
            part6::closed_conversations,
            part6::ignore_user,
            part6::unignore_user,
            part6::ignored_users,
            part6::list_guardians,
            part6::sign_guardians,
            part6::list_devices,
            part6::revoke_device,
            part6::link_device_code,
            part6::approve_device,
            part6::finish_device_link,
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
