//! System tray: the icon and its menu, and what closing the window does. The menu has Open Dissent, a Status submenu,
//! Mute notifications and Quit Dissent. Left-click opens the window. The glyph follows the system theme: white on a
//! dark taskbar, #1e1f22 on a light one, and a template image on macOS so the menu bar tints it.

use dissent_core::Status;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Theme, WindowEvent, Wry};
use tauri_plugin_notification::NotificationExt;
use tracing::warn;

use crate::commands::AppState;
use crate::shell_cmds::{self, ShellState};
use crate::shell_settings::ShellSettings;

const TRAY_ID: &str = "main";
const WINDOW: &str = "main";
/// White glyph, for dark themes.
const LIGHT_GLYPH: &[u8] = include_bytes!("../icons/tray@2x.png");
/// #1e1f22 glyph, for light themes.
const DARK_GLYPH: &[u8] = include_bytes!("../icons/tray-dark@2x.png");

/// The Status submenu, in order: menu id, label and the status it sets.
const STATUSES: [(&str, &str, Status); 4] = [
    ("status:online", "Online", Status::Online),
    ("status:idle", "Idle", Status::Idle),
    ("status:dnd", "Do Not Disturb", Status::Dnd),
    ("status:invisible", "Invisible", Status::Invisible),
];

/// The check marks the tray shows. Kept so a change made elsewhere can update them.
pub struct TrayMenu {
    statuses: Vec<(Status, CheckMenuItem<Wry>)>,
    mute: CheckMenuItem<Wry>,
}

/// The glyph for a taskbar theme. Light taskbars get the dark glyph.
fn glyph_for(theme: Theme) -> tauri::Result<Image<'static>> {
    match theme {
        Theme::Light => Image::from_bytes(DARK_GLYPH),
        _ => Image::from_bytes(LIGHT_GLYPH),
    }
}

/// Builds the tray icon and its menu, and wires the window: closing it keeps Dissent in the tray when that is on,
/// and quits cleanly when it is off.
pub fn install(app: &AppHandle, shell: ShellSettings, status: Status) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Dissent", true, None::<&str>)?;
    let status_menu = Submenu::with_id(app, "status", "Status", true)?;
    let mut statuses = Vec::new();
    for (id, label, value) in STATUSES {
        let item = CheckMenuItem::with_id(app, id, label, true, value == status, None::<&str>)?;
        status_menu.append(&item)?;
        statuses.push((value, item));
    }
    let mute = CheckMenuItem::with_id(app, "mute", "Mute notifications", true, shell.notifications_muted, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Dissent", true, None::<&str>)?;
    let (sep_a, sep_b) = (PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?);
    let items: [&dyn IsMenuItem<Wry>; 6] = [&open, &status_menu, &sep_a, &mute, &sep_b, &quit_item];
    let menu = Menu::with_items(app, &items)?;
    let theme = app
        .get_webview_window(WINDOW)
        .and_then(|w| w.theme().ok())
        .unwrap_or(Theme::Dark);

    app.manage(TrayMenu { statuses, mute });
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(glyph_for(theme)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Dissent")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    if let Some(window) = app.get_webview_window(WINDOW) {
        let handle = app.clone();
        window.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                close_window(&handle);
            }
            WindowEvent::ThemeChanged(theme) => {
                if let (Some(tray), Ok(glyph)) = (handle.tray_by_id(TRAY_ID), glyph_for(*theme)) {
                    if let Err(e) = tray.set_icon(Some(glyph)) {
                        warn!(error = %e, "could not switch the tray glyph");
                    }
                }
            }
            _ => {}
        });
    }
    Ok(())
}

/// Closing the window hides it while "keep running" is on, and quits cleanly when it is off. The first hide shows
/// a notice, so the user knows the app is still running.
fn close_window(app: &AppHandle) {
    let shell = app.state::<ShellState>();
    if !shell.get().keep_running {
        quit(app);
        return;
    }
    if let Some(window) = app.get_webview_window(WINDOW) {
        match window.hide() {
            Ok(()) => tracing::info!("the window closed to the tray"),
            Err(e) => warn!(error = %e, "could not hide the window"),
        }
    }
    if !shell.get().tray_notice_shown {
        if let Err(e) = shell.mark_tray_notice_shown() {
            warn!(error = %e, "could not save the tray notice");
        }
        match app.notification().builder().title("Dissent").body("Dissent is still running in the tray").show() {
            Ok(()) => tracing::info!("the tray notice was shown"),
            Err(e) => warn!(error = %e, "could not show the tray notice"),
        }
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "quit" => quit(app),
        "mute" => {
            let muted = app.state::<TrayMenu>().mute.is_checked().unwrap_or(false);
            if let Err(e) = shell_cmds::set_muted(app, muted) {
                warn!(error = %e, "could not change notification muting");
            }
        }
        other => {
            if let Some((_, _, status)) = STATUSES.iter().find(|(id, _, _)| *id == other) {
                set_status(app, *status);
            }
        }
    }
}

/// Sets our status from the tray. The engine's status event then updates the check marks.
fn set_status(app: &AppHandle, status: Status) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let engine = app.state::<AppState>().engine.read().await.clone();
        if let Some(engine) = engine {
            if let Err(e) = engine.set_status(status, None) {
                warn!(error = %e, "could not set the status from the tray");
            }
        }
    });
}

/// Moves the status check mark to `status`. Called when the engine reports a status change.
pub fn sync_status(app: &AppHandle, status: Status) {
    if let Some(menu) = app.try_state::<TrayMenu>() {
        for (value, item) in &menu.statuses {
            let _ = item.set_checked(*value == status);
        }
    }
}

/// Moves the Mute notifications check mark to `muted`.
pub fn sync_mute(app: &AppHandle, muted: bool) {
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let _ = menu.mute.set_checked(muted);
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Shuts the engine down cleanly, then exits. The run-loop exit hook finds no engine left to stop. The disclaimer's
/// Quit uses it too.
pub fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let engine = tauri::async_runtime::block_on(state.engine.write()).take();
    if let Some(engine) = engine {
        if let Err(e) = tauri::async_runtime::block_on(engine.shutdown()) {
            warn!(error = %e, "engine shutdown failed");
        }
    }
    app.exit(0);
}
