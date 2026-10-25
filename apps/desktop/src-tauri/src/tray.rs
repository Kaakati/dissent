//! System tray. Left-click shows the window, the menu has Open and Quit, and closing the window hides it here.
//! The glyph follows the system theme: white on a dark taskbar, #1e1f22 on a light one.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Theme, WindowEvent};
use tracing::warn;

use crate::commands::AppState;

const TRAY_ID: &str = "main";
const WINDOW: &str = "main";
/// White glyph, for dark themes.
const LIGHT_GLYPH: &[u8] = include_bytes!("../icons/tray@2x.png");
/// #1e1f22 glyph, for light themes.
const DARK_GLYPH: &[u8] = include_bytes!("../icons/tray-dark@2x.png");

/// The glyph for a taskbar theme. Light taskbars get the dark glyph.
fn glyph_for(theme: Theme) -> tauri::Result<Image<'static>> {
    match theme {
        Theme::Light => Image::from_bytes(DARK_GLYPH),
        _ => Image::from_bytes(LIGHT_GLYPH),
    }
}

/// Builds the tray icon and wires the window so closing it hides it instead of quitting.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Dissent", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit_item])?;
    let theme = app
        .get_webview_window(WINDOW)
        .and_then(|w| w.theme().ok())
        .unwrap_or(Theme::Dark);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(glyph_for(theme)?)
        .tooltip("Dissent")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => quit(app),
            _ => {}
        })
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
                if let Some(w) = handle.get_webview_window(WINDOW) {
                    let _ = w.hide();
                }
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

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Shuts the engine down cleanly, then exits. The run-loop exit hook finds no engine left to stop.
fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let engine = tauri::async_runtime::block_on(state.engine.write()).take();
    if let Some(engine) = engine {
        if let Err(e) = tauri::async_runtime::block_on(engine.shutdown()) {
            warn!(error = %e, "engine shutdown failed");
        }
    }
    app.exit(0);
}
