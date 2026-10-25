//! Attachments: the native file picker, drag and drop onto the window, and opening or revealing downloaded files.
//!
//! Opening and revealing only work inside `<data_dir>/downloads`. Paths are resolved first (symlinks and `..`
//! included), so a message can never make the app open an arbitrary file.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tracing::warn;

use crate::commands::AppState;

pub const EVENT_DROPPED: &str = "dissent://files-dropped";
pub const EVENT_DRAG_OVER: &str = "dissent://drag-over";
pub const EVENT_DRAG_LEAVE: &str = "dissent://drag-leave";
const WINDOW: &str = "main";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// The MIME type from the file extension. Unknown extensions give `application/octet-stream`.
pub fn mime_for(path: &Path) -> String {
    mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string()
}

/// Describes a regular file. Directories and missing files are errors.
pub fn describe(path: &Path) -> Result<PickedFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(PickedFile {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size: meta.len(),
        mime: mime_for(path),
    })
}

/// `<data_dir>/downloads`, the only folder the app opens files from.
pub fn downloads_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("downloads")
}

/// Resolves `candidate` to its real path and checks it sits inside `downloads`. Symlinks and `..` are resolved
/// before the check, so a link or traversal that leaves the folder is rejected.
pub fn validate_inside(downloads: &Path, candidate: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(downloads).map_err(|_| "no downloads folder yet".to_string())?;
    let real = std::fs::canonicalize(candidate).map_err(|e| format!("cannot open that path: {e}"))?;
    if !real.starts_with(&root) {
        return Err("that path is outside the downloads folder".into());
    }
    Ok(real)
}

/// Shows the native picker with multi-select. Cancelling gives an empty list.
#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Result<Vec<PickedFile>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_files())
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for file in picked.unwrap_or_default() {
        let path = file.into_path().map_err(|e| e.to_string())?;
        out.push(describe(&path)?);
    }
    Ok(out)
}

/// Opens the folder that holds a downloaded file, with the file selected.
#[tauri::command]
pub fn reveal_attachment(app: AppHandle, state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let real = validate_inside(&downloads_dir(&state.data_dir), Path::new(&path))?;
    app.opener().reveal_item_in_dir(&real).map_err(|e| e.to_string())
}

/// Opens a downloaded file with the OS default application.
#[tauri::command]
pub fn open_attachment(app: AppHandle, state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let real = validate_inside(&downloads_dir(&state.data_dir), Path::new(&path))?;
    if !real.is_file() {
        return Err("only files can be opened".into());
    }
    app.opener()
        .open_path(real.to_string_lossy().into_owned(), None::<String>)
        .map_err(|e| e.to_string())
}

/// Forwards window drag-and-drop to the UI. The overlay is driven by drag-over and drag-leave.
pub fn install(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        warn!("main window missing; drag and drop is off");
        return;
    };
    let handle = app.clone();
    window.on_window_event(move |event| {
        let WindowEvent::DragDrop(drop) = event else { return };
        match drop {
            tauri::DragDropEvent::Enter { .. } | tauri::DragDropEvent::Over { .. } => {
                let _ = handle.emit(EVENT_DRAG_OVER, ());
            }
            tauri::DragDropEvent::Leave => {
                let _ = handle.emit(EVENT_DRAG_LEAVE, ());
            }
            tauri::DragDropEvent::Drop { paths, .. } => {
                let _ = handle.emit(EVENT_DRAG_LEAVE, ());
                let files: Vec<PickedFile> = paths.iter().filter_map(|p| describe(p).ok()).collect();
                if !files.is_empty() {
                    let _ = handle.emit(EVENT_DROPPED, files);
                }
            }
            _ => {}
        }
    });
}

/// Creates the downloads folder and allows the asset protocol to read it, and nothing else.
pub fn allow_downloads(app: &AppHandle, data_dir: &Path) -> tauri::Result<()> {
    let downloads = downloads_dir(data_dir);
    std::fs::create_dir_all(&downloads)?;
    app.asset_protocol_scope().allow_directory(&downloads, true)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-att-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn mime_comes_from_the_extension() {
        assert_eq!(mime_for(Path::new("photo.png")), "image/png");
        assert_eq!(mime_for(Path::new("notes.PDF")), "application/pdf");
        assert_eq!(mime_for(Path::new("archive.unknownext")), "application/octet-stream");
        assert_eq!(mime_for(Path::new("no-extension")), "application/octet-stream");
    }

    #[test]
    fn a_file_inside_downloads_is_accepted() {
        let data = scratch("inside");
        let downloads = downloads_dir(&data);
        std::fs::create_dir_all(downloads.join("sub")).unwrap();
        std::fs::write(downloads.join("sub/a.txt"), b"x").unwrap();
        assert!(validate_inside(&downloads, &downloads.join("sub/a.txt")).is_ok());
        assert!(validate_inside(&downloads, &downloads.join("sub/../sub/a.txt")).is_ok());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn dot_dot_out_of_downloads_is_rejected() {
        let data = scratch("dotdot");
        let downloads = downloads_dir(&data);
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(data.join("secret.txt"), b"x").unwrap();
        let escape = downloads.join("../secret.txt");
        assert!(validate_inside(&downloads, &escape).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_missing_path_is_rejected() {
        let data = scratch("missing");
        let downloads = downloads_dir(&data);
        std::fs::create_dir_all(&downloads).unwrap();
        assert!(validate_inside(&downloads, &downloads.join("nope.txt")).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_symlink_out_of_downloads_is_rejected() {
        let data = scratch("symlink");
        let downloads = downloads_dir(&data);
        std::fs::create_dir_all(&downloads).unwrap();
        let outside = data.join("outside.txt");
        std::fs::write(&outside, b"x").unwrap();
        let link = downloads.join("link.txt");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&outside, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, &link);
        if made.is_err() {
            // Creating symlinks on Windows needs Developer Mode or admin rights. Skip rather than fail there.
            eprintln!("skipped: cannot create a symlink in this environment");
        } else {
            assert!(validate_inside(&downloads, &link).is_err());
        }
        let _ = std::fs::remove_dir_all(&data);
    }
}
