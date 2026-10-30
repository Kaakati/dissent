//! The unread badge on the taskbar button or dock icon, and the message notifications the OS still holds. The UI sends
//! what is unread whenever that changes, when the window gains focus and when the open conversation changes; the count
//! is worked out here. The conversation the user is viewing never counts: its messages are read as they land.

use serde::Deserialize;
use tauri::{AppHandle, WebviewWindow};

/// One conversation's or server's unread state, as the UI knows it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BadgeSource {
    /// The UI's key: `dm:<peer>`, `group:<id>`, `ch:<community>:<channel>` or `server:<community>`.
    pub key: String,
    /// Unread messages from others.
    #[serde(default)]
    pub unread: u32,
    /// The unread messages that mention the user.
    #[serde(default)]
    pub mentions: u32,
    /// Muted: only mentions count.
    #[serde(default)]
    pub muted: bool,
}

/// The number on the badge, or `None` for no badge: unread messages and mentions everywhere except `viewing`. A muted
/// source counts its mentions only. A source's mentions are among its unread messages, so they are not added twice.
pub fn badge_count(sources: &[BadgeSource], viewing: Option<&str>) -> Option<i64> {
    let total: i64 = sources
        .iter()
        .filter(|s| Some(s.key.as_str()) != viewing)
        .map(|s| i64::from(if s.muted { s.mentions } else { s.unread.max(s.mentions) }))
        .sum();
    (total > 0).then_some(total)
}

/// Side of the Windows overlay icon, in pixels. The taskbar draws it over the corner of the app icon.
#[cfg(any(windows, test))]
const DOT_PX: u32 = 16;

/// The Windows overlay: a red dot with a smoothed edge, as RGBA. Windows has no badge count for a desktop app.
#[cfg(any(windows, test))]
fn dot_rgba() -> Vec<u8> {
    let r = DOT_PX as f32 / 2.0;
    let mut px = Vec::with_capacity((DOT_PX * DOT_PX * 4) as usize);
    for y in 0..DOT_PX {
        for x in 0..DOT_PX {
            let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
            let alpha = (r - d).clamp(0.0, 1.0);
            px.extend_from_slice(&[0xE5, 0x48, 0x4D, (alpha * 255.0).round() as u8]);
        }
    }
    px
}

fn show(window: &WebviewWindow, count: Option<i64>) -> tauri::Result<()> {
    #[cfg(windows)]
    window.set_overlay_icon(count.map(|_| tauri::image::Image::new_owned(dot_rgba(), DOT_PX, DOT_PX)))?;
    #[cfg(not(windows))]
    window.set_badge_count(count)?;
    Ok(())
}

/// Sets the badge from what is unread. `viewing` is the conversation on screen in a focused window, scrolled to its end.
#[tauri::command]
pub fn set_unread_badge(window: WebviewWindow, sources: Vec<BadgeSource>, viewing: Option<String>) {
    let count = badge_count(&sources, viewing.as_deref());
    if let Err(e) = show(&window, count) {
        tracing::debug!(error = %e, "the unread badge could not be set");
    }
}

/// The user is in the app: the message notifications it delivered are cleared and a flashing taskbar button stops.
/// Windows keeps delivered notifications in the notification center and counts them on the taskbar button. macOS has no
/// way to withdraw one through notify-rust; its dock badge is [`set_unread_badge`]'s.
#[tauri::command]
pub fn clear_message_notifications(app: AppHandle, window: WebviewWindow) {
    let _ = window.request_user_attention(None);
    #[cfg(windows)]
    clear_toasts(&app.config().identifier);
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().remove_all_active();
    }
    #[cfg(target_os = "macos")]
    let _ = app;
}

/// Whether the exe runs from a cargo build folder. The notification plugin gives toasts the app's id only outside one;
/// there they are PowerShell's, and not the app's to clear.
#[cfg(any(windows, test))]
fn in_build_dir(exe_dir: &std::path::Path) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    let dir = exe_dir.display().to_string();
    dir.ends_with(&format!("{sep}target{sep}debug")) || dir.ends_with(&format!("{sep}target{sep}release"))
}

#[cfg(windows)]
fn clear_toasts(identifier: &str) {
    use windows::UI::Notifications::ToastNotificationManager;
    let Ok(exe) = tauri::utils::platform::current_exe() else { return };
    if exe.parent().is_none_or(in_build_dir) {
        return;
    }
    let id = windows::core::HSTRING::from(identifier);
    if let Err(e) = ToastNotificationManager::History().and_then(|h| h.ClearWithId(&id)) {
        tracing::debug!(error = %e, "delivered notifications could not be cleared");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(key: &str, unread: u32, mentions: u32, muted: bool) -> BadgeSource {
        BadgeSource { key: key.into(), unread, mentions, muted }
    }

    #[test]
    fn nothing_unread_shows_no_badge() {
        assert_eq!(badge_count(&[], None), None);
        assert_eq!(badge_count(&[src("dm:a", 0, 0, false)], None), None);
    }

    #[test]
    fn unread_messages_add_up_and_mentions_are_not_counted_twice() {
        let sources = [src("dm:a", 3, 1, false), src("group:g", 2, 0, false), src("server:c", 0, 4, false)];
        assert_eq!(badge_count(&sources, None), Some(9));
    }

    #[test]
    fn the_viewed_conversation_never_counts() {
        let sources = [src("dm:a", 3, 0, false), src("dm:b", 1, 0, false)];
        assert_eq!(badge_count(&sources, Some("dm:a")), Some(1));
        assert_eq!(badge_count(&sources[..1], Some("dm:a")), None);
    }

    #[test]
    fn a_muted_conversation_counts_only_its_mentions() {
        assert_eq!(badge_count(&[src("group:g", 5, 0, true)], None), None);
        assert_eq!(badge_count(&[src("group:g", 5, 2, true)], None), Some(2));
    }

    #[test]
    fn the_ui_payload_decodes_with_missing_counts() {
        let sources: Vec<BadgeSource> = serde_json::from_str(r#"[{"key":"dm:a","unread":2},{"key":"server:c","mentions":1}]"#).unwrap();
        assert_eq!(badge_count(&sources, None), Some(3));
    }

    #[test]
    fn the_overlay_dot_is_opaque_inside_and_clear_at_the_corners() {
        let px = dot_rgba();
        assert_eq!(px.len(), (DOT_PX * DOT_PX * 4) as usize);
        let alpha = |x: u32, y: u32| px[((y * DOT_PX + x) * 4 + 3) as usize];
        assert_eq!(alpha(DOT_PX / 2, DOT_PX / 2), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(DOT_PX - 1, DOT_PX - 1), 0);
    }

    #[test]
    fn a_build_folder_exe_is_told_apart_from_an_installed_one() {
        let sep = std::path::MAIN_SEPARATOR;
        assert!(in_build_dir(std::path::Path::new(&format!("x{sep}target{sep}debug"))));
        assert!(in_build_dir(std::path::Path::new(&format!("x{sep}target{sep}release"))));
        assert!(!in_build_dir(std::path::Path::new(&format!("x{sep}Programs{sep}Dissent"))));
    }
}
