//! Profile pictures and community icons. The UI sends a picture as base64: a WebP or PNG, already
//! cropped to 256 px. The engine encrypts it and publishes it. The views carry the local file of
//! each picture, and `for_ui` turns those paths into URLs the webview can load.

use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use dissent_core::CommunityId;
use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State};

use crate::commands::{engine, err, parse, AppState};

/// The event that tells the UI a picture changed. Its payload is `{ root_id }` or `{ community_id }`.
pub const AVATAR_EVENT: &str = "dissent://avatar-changed";

fn decode(bytes: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(bytes).map_err(err)
}

#[tauri::command]
pub async fn set_avatar(state: State<'_, AppState>, bytes: String) -> Result<(), String> {
    let bytes = decode(&bytes)?;
    engine(&state).await?.set_avatar(bytes).await.map_err(err)
}

#[tauri::command]
pub async fn clear_avatar(state: State<'_, AppState>) -> Result<(), String> {
    engine(&state).await?.clear_avatar().await.map_err(err)
}

#[tauri::command]
pub async fn set_community_icon(state: State<'_, AppState>, community_id: String, bytes: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let bytes = decode(&bytes)?;
    engine(&state).await?.set_community_icon(community, bytes).await.map_err(err)
}

#[tauri::command]
pub async fn clear_community_icon(state: State<'_, AppState>, community_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.clear_community_icon(community).await.map_err(err)
}

/// Serialises a view for the UI. Under `avatar` and `icon`, local picture paths become asset URLs.
pub fn for_ui<T: Serialize>(view: &T) -> Result<Value, String> {
    let mut value = serde_json::to_value(view).map_err(err)?;
    asset_urls(&mut value);
    Ok(value)
}

fn asset_urls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                let url = match (key.as_str(), &*field) {
                    ("avatar" | "icon", Value::String(path)) if Path::new(path).is_absolute() => Some(asset_url(path)),
                    _ => None,
                };
                match url {
                    Some(url) => *field = Value::String(url),
                    None => asset_urls(field),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(asset_urls),
        _ => {}
    }
}

/// The URL the webview loads a local file from. It matches Tauri's `convertFileSrc`.
pub fn asset_url(path: &str) -> String {
    let base = if cfg!(windows) { "http://asset.localhost/" } else { "asset://localhost/" };
    let mut out = String::from(base);
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Lets the webview load the decrypted pictures under the data dir. Called once at startup.
pub fn allow_avatars(app: &tauri::AppHandle, data_dir: &Path) -> tauri::Result<()> {
    let dir = data_dir.join("avatars");
    std::fs::create_dir_all(&dir)?;
    app.asset_protocol_scope().allow_directory(&dir, true)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_pictures_become_asset_urls_and_nothing_else_changes() {
        // Absolute paths as each platform writes them: a Unix path is relative on Windows.
        let (mine, theirs) = if cfg!(windows) {
            (r"C:\Users\ada\dissent\avatars\ab.webp", r"D:\data\avatars\cd.png")
        } else {
            ("/home/ada/dissent/avatars/ab.webp", "/data/avatars/cd.png")
        };
        let view = json!({
            "display_name": "Sami",
            "avatar": mine,
            "members": [{ "root_id": "x", "avatar": null }, { "root_id": "y", "avatar": theirs }],
            "icon": null,
        });
        let out = for_ui(&view).unwrap();
        let avatar = out["avatar"].as_str().unwrap();
        assert_eq!(avatar, asset_url(mine));
        assert!(avatar.ends_with("ab.webp"), "{avatar}");
        assert!(avatar.starts_with(if cfg!(windows) { "http://asset.localhost/" } else { "asset://localhost/" }));
        assert!(out["members"][0]["avatar"].is_null());
        assert_eq!(out["members"][1]["avatar"].as_str().unwrap(), asset_url(theirs));
        assert_eq!(out["display_name"], "Sami");
        assert!(out["icon"].is_null());
    }

    #[test]
    fn an_image_url_is_not_rewritten() {
        let view = json!({ "avatar": "data:image/webp;base64,AAAA" });
        assert_eq!(for_ui(&view).unwrap()["avatar"], "data:image/webp;base64,AAAA");
    }
}
