mod commands;
mod events;
mod network;
mod storage;
mod tray;
mod shell_cmds;
mod shell_settings;
mod updates;
mod attachments;
mod avatar_cmds;
mod server_prefs_cmds;
mod paging_cmds;
mod server_settings_cmds;
mod voice;
mod voice_settings;
mod mic_test;
mod transfers_cmds;
mod part6;
mod contacts_cmds;
mod saved_cmds;
mod conv_cmds;
mod status_cmds;
mod typing_cmds;
mod profile;
mod deep_link;
mod website_handles;
mod website_handle_cmds;

use commands::AppState;
use dissent_core::NodeConfig;
use dissent_identity::keystore;
use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

/// Debug builds, or any build with the `fake-capture` feature: `DISSENT_FAKE_CAPTURE=1` makes this engine's camera and screen a
/// test picture, so a live check can run two instances without a camera or a display. Other release builds ignore it.
pub(crate) fn fake_capture(engine: &dissent_core::Engine) {
    if (cfg!(debug_assertions) || cfg!(feature = "fake-capture")) && std::env::var_os("DISSENT_FAKE_CAPTURE").is_some_and(|v| v == "1") {
        tracing::warn!("fake capture is on (DISSENT_FAKE_CAPTURE=1): the camera and the screen are a test picture");
        engine.set_test_capture(dissent_core::TestCapture::Pattern);
    }
}

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

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mainline=error,mainline::rpc=off")))
        .init();

    // Custom and portable profiles are locked before Tauri starts (see profile::claim_early). The guard lives
    // until run() returns, which is when the app exits.
    let _profile_lock = profile::claim_early();
    let mut builder = tauri::Builder::default();
    // Only the default profile is single instance, and only it takes dissent:// links. Dev and portable profiles can run
    // beside it (see profile.rs) and never claim the scheme.
    let default_profile = profile::mode() == profile::Mode::Default;
    if default_profile {
        builder = builder.plugin(profile::single_instance()).plugin(tauri_plugin_deep_link::init());
    }
    let app = builder
        .manage(deep_link::Queue::default())
        .manage(website_handle_cmds::HandleChecks::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![shell_settings::BACKGROUND_ARG]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    voice::on_shortcut(app, shortcut, event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed)
                })
                .build(),
        )
        .setup(|app| {
            // Any startup failure ends in a native error box, never a hidden window or a silent exit.
            if let Err(e) = setup(app) {
                profile::fatal(&e.to_string());
            }
            if profile::mode() == profile::Mode::Default {
                deep_link::start(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            deep_link::take_deep_links,
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
            commands::set_server_order,
            commands::create_community,
            commands::create_invite,
            commands::join_community,
            commands::invite_preview,
            commands::create_full_invite,
            commands::leave_community,
            commands::community_view,
            commands::create_channel,
            part6::create_voice_channel,
            part6::update_channel,
            part6::delete_channel,
            part6::list_invites,
            part6::revoke_invite,
            commands::moderate,
            commands::member_only_status,
            commands::set_member_only,
            commands::set_history_visibility,
            commands::set_channel_access,
            commands::create_private_channel,
            commands::channel_access,
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
            voice_settings::voice_settings,
            voice_settings::set_voice_settings,
            mic_test::mic_test_start,
            mic_test::mic_test_stop,
            transfers_cmds::transfer_settings,
            transfers_cmds::set_transfer_settings,
            transfers_cmds::store_stats,
            transfers_cmds::pick_download_dir,
            transfers_cmds::reset_download_dir,
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
            transfers_cmds::link_preview_prompted,
            transfers_cmds::mark_link_preview_prompted,
            contacts_cmds::pinned_conversations,
            saved_cmds::add_bookmark,
            saved_cmds::remove_bookmark,
            saved_cmds::list_bookmarks,
            saved_cmds::add_reminder,
            saved_cmds::list_reminders,
            saved_cmds::remove_reminder,
            saved_cmds::mark_unread,
            contacts_cmds::list_contacts,
            contacts_cmds::send_contact_request,
            contacts_cmds::address_discovery,
            contacts_cmds::set_address_discovery,
            contacts_cmds::friends_shared,
            contacts_cmds::set_friends_shared,
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
            conv_cmds::dm_reply,
            conv_cmds::dm_react,
            conv_cmds::dm_unreact,
            conv_cmds::dm_edit,
            conv_cmds::dm_delete,
            conv_cmds::group_reply,
            conv_cmds::group_react,
            conv_cmds::group_unreact,
            conv_cmds::group_edit,
            conv_cmds::group_delete,
            contacts_cmds::add_group_members,
            contacts_cmds::leave_group,
            commands::react,
            commands::unreact,
            commands::list_conversations,
            commands::dm_messages,
            commands::send_dm,
            commands::retry_message,
            commands::net_status,
            status_cmds::set_status,
            status_cmds::get_status,
            typing_cmds::typing,
            typing_cmds::stop_typing,
            typing_cmds::typing_in,
            status_cmds::record_activity,
            avatar_cmds::set_avatar,
            avatar_cmds::clear_avatar,
            avatar_cmds::set_profile_banner,
            avatar_cmds::set_profile_banner_preset,
            avatar_cmds::clear_profile_banner,
            avatar_cmds::set_community_icon,
            avatar_cmds::clear_community_icon,
            avatar_cmds::prepare_animated_icon,
            avatar_cmds::set_community_icon_animated,
            server_prefs_cmds::get_server_prefs,
            server_prefs_cmds::set_server_prefs,
            server_prefs_cmds::server_alerts,
            paging_cmds::channel_messages_before,
            paging_cmds::dm_messages_before,
            paging_cmds::group_messages_before,
            server_settings_cmds::rename_community,
            server_settings_cmds::set_community_banner,
            server_settings_cmds::clear_community_banner,
            server_settings_cmds::set_community_banner_preset,
            server_settings_cmds::set_community_profile,
            server_settings_cmds::prepare_animated_banner,
            server_settings_cmds::set_community_banner_animated,
            server_settings_cmds::set_profile_banner_animated,
            server_settings_cmds::set_hosted_relays,
            server_settings_cmds::set_members_host_relays,
            server_settings_cmds::check_relay,
            commands::get_network_settings,
            shell_cmds::shell_settings,
            shell_cmds::set_shell_settings,
            commands::safety_number,
            updates::check_update,
            updates::install_update,
            updates::open_release_page,
            profile::vault_status,
            profile::unlock_profile,
            profile::finish_portable_link,
            profile::choose_profile,
            attachments::pick_files,
            attachments::reveal_attachment,
            attachments::open_attachment,
            commands::set_network_settings,
            commands::relay_status,
            part6::capabilities,
            part6::set_note,
            part6::pin_conversation,
            part6::mark_read,
            part6::mark_mentions_read,
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
            part6::set_camera_preview,
            part6::set_video_tile,
            part6::subscribe_encoded_video,
            part6::unsubscribe_encoded_video,
            part6::request_video_keyframes,
            part6::set_jpeg_tiles,
            part6::set_screen_share,
            part6::list_share_sources,
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
            website_handle_cmds::website_handle_setup,
            website_handle_cmds::set_website_handle,
            website_handle_cmds::check_website_handle,
            website_handle_cmds::remove_website_handle,
            website_handle_cmds::website_handle_status,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| profile::fatal(&format!("The app could not be built: {e}")));

    app.run(|handle, event| {
        // A refused second launch has no window. Keep it running until its message is dismissed.
        if let RunEvent::ExitRequested { api, .. } = &event {
            if profile::is_refusing(handle) {
                api.prevent_exit();
            }
        }
        if let RunEvent::Exit = event {
            // A portable profile that never unlocked has no engine to stop.
            let Some(state) = handle.try_state::<AppState>() else { return };
            let engine = tauri::async_runtime::block_on(state.engine.write()).take();
            if let Some(engine) = engine {
                if let Err(e) = tauri::async_runtime::block_on(engine.shutdown()) {
                    tracing::warn!(error = %e, "engine shutdown failed");
                }
            }
        }
    });
}

/// Starts the app's state: the profile, the engine and the windows. Any error returned here is shown to
/// the user by [`profile::fatal`].
fn setup(app: &mut tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Managed before any branch, so the link screens can use it before a profile is open.
    app.manage(part6::PendingLink::default());
    let mode = profile::mode();
    // A portable exe's first run. The chooser saves an answer, then the app restarts in that mode.
    if mode == profile::Mode::Undecided {
        app.manage(profile::Chooser);
        profile::show_window(app.handle());
        return Ok(());
    }
    let data_dir = profile::data_dir(app, mode)?;
    // A portable copy that is still pending has no folder yet. The copy creates it, so nothing is made here.
    if profile::copy_pending(mode) {
        app.manage(profile::Locked::new(data_dir));
        profile::show_window(app.handle());
        return Ok(());
    }
    std::fs::create_dir_all(&data_dir)?;
    // The default profile's lock. The single-instance plugin normally stops a second launch before this.
    if mode == profile::Mode::Default {
        let Some(lock) = profile::lock(&data_dir)? else {
            profile::refuse_second_instance(app);
            return Ok(());
        };
        app.manage(lock);
    }
    tracing::info!(dir = %data_dir.display(), ?mode, "data dir");

    // A portable profile has no keychain entry. It waits for the passphrase (profile::unlock_profile).
    if mode == profile::Mode::Portable {
        app.manage(profile::Locked::new(data_dir));
        profile::show_window(app.handle());
        return Ok(());
    }

    // chisle: the data key comes from the OS credential store. If none is available we refuse to start
    // rather than write plaintext.
    let vault = match profile::open_vault(&data_dir) {
        Ok(vault) => vault,
        Err(e) => {
            tracing::error!(error = %e, "no key for the identity; refusing to start");
            return Err(e.into());
        }
    };
    let identity = keystore::load_or_create(&data_dir.join("identity.bin"), commands::now_ms()?, &vault)?;
    tauri::async_runtime::block_on(profile::open_profile(app.handle(), data_dir, vault, identity))?;
    Ok(())
}
