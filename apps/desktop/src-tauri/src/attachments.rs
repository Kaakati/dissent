//! Attachments: the native file picker, drag and drop onto the window, and opening or revealing downloaded files.
//!
//! Opening and revealing only work for files Dissent saved itself, in whichever download folder it used, and for files
//! this device shared, where they were shared from. The webview may read those files and the thumbnails, never the
//! rest of a download folder. Paths are resolved first (symlinks and `..` included), so a message can never make the
//! app open an arbitrary file.

use std::path::{Path, PathBuf};

use dissent_core::Engine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tracing::warn;

use crate::commands::{engine, AppState};

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

/// Resolves `candidate` to its real path and checks that Dissent saved it. `saved` holds the finished downloads as the
/// engine recorded them, in any folder it has used. Symlinks and `..` are resolved first, so a path passes only when
/// its real file is one Dissent saved. Another file in the same folder is refused.
pub fn validate_saved(saved: &[PathBuf], candidate: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(candidate).map_err(|e| format!("cannot open that path: {e}"))?;
    if !saved.iter().any(|p| std::fs::canonicalize(p).is_ok_and(|s| s == real)) {
        return Err("that file was not saved by Dissent".into());
    }
    Ok(real)
}

/// Lets the asset protocol read each saved download that is still on disk, and nothing else in its folder. Called at
/// startup and on every transfer change, so a finished download can be shown.
pub fn allow_saved(app: &AppHandle, saved: &[PathBuf]) {
    for path in saved.iter().filter(|p| p.is_file()) {
        if let Err(e) = app.asset_protocol_scope().allow_file(path) {
            warn!(error = %e, path = %path.display(), "could not allow a saved download");
        }
    }
}

/// The folder a download goes to unless the user picked one: the OS Downloads folder. A portable exe keeps its
/// profile's folder instead, so it does not write to the host's Downloads. `None` leaves the engine on that folder.
pub fn default_download_dir(app: &AppHandle) -> Option<PathBuf> {
    if crate::profile::mode() == crate::profile::Mode::Portable {
        return None;
    }
    app.path().download_dir().ok()
}

/// Allows the asset protocol to read the files a freshly started engine already saved.
pub fn prepare_engine(app: &AppHandle, engine: &Engine) {
    allow_saved(app, &engine.saved_files());
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

/// The folder under the data dir that holds pasted files. They stay after the send, as a picked file stays where it
/// is: the sender's own message shows its picture from that file.
const PASTED_DIR: &str = "pasted";

/// The file name a paste is saved under: the last part of what the webview gave, without any folders.
fn pasted_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        "pasted-file".into()
    } else {
        base.to_string()
    }
}

/// Writes pasted bytes to `<data dir>/pasted/<unique>/<name>` and describes the file, so it is staged and sent like a
/// picked one. Each paste gets its own folder, so two pastes with the same name stay apart.
pub fn save_pasted(data_dir: &Path, name: &str, bytes: &[u8]) -> Result<PickedFile, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let dir = data_dir.join(PASTED_DIR).join(format!("{nanos}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(pasted_name(name));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    describe(&path)
}

/// Saves a file pasted into the composer. The webview has its bytes but no path, so they arrive as base64.
#[tauri::command]
pub async fn stage_pasted_file(state: tauri::State<'_, AppState>, name: String, bytes: String) -> Result<PickedFile, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(bytes).map_err(|e| e.to_string())?;
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || save_pasted(&data_dir, &name, &bytes))
        .await
        .map_err(|e| e.to_string())?
}

/// Opens the folder that holds a saved download, with the file selected.
#[tauri::command]
pub async fn reveal_attachment(app: AppHandle, state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let real = validate_saved(&engine(&state).await?.saved_files(), Path::new(&path))?;
    app.opener().reveal_item_in_dir(&real).map_err(|e| e.to_string())
}

/// Opens a saved download with the OS default application.
#[tauri::command]
pub async fn open_attachment(app: AppHandle, state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let real = validate_saved(&engine(&state).await?.saved_files(), Path::new(&path))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-att-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(path: &Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn mime_comes_from_the_extension() {
        assert_eq!(mime_for(Path::new("photo.png")), "image/png");
        assert_eq!(mime_for(Path::new("notes.PDF")), "application/pdf");
        assert_eq!(mime_for(Path::new("archive.unknownext")), "application/octet-stream");
        assert_eq!(mime_for(Path::new("no-extension")), "application/octet-stream");
    }

    #[test]
    fn a_paste_is_saved_under_its_own_name_and_folder() {
        let data = scratch("pasted");
        let a = save_pasted(&data, "pasted-image-20261029-120000.png", b"png").unwrap();
        let b = save_pasted(&data, "pasted-image-20261029-120000.png", b"png2").unwrap();
        assert_eq!(a.name, "pasted-image-20261029-120000.png");
        assert_eq!((a.size, a.mime.as_str()), (3, "image/png"));
        assert_ne!(a.path, b.path);
        assert!(Path::new(&a.path).starts_with(data.join(PASTED_DIR)));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_pasted_name_cannot_leave_its_folder() {
        assert_eq!(pasted_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(pasted_name("../a/b.png"), "b.png");
        assert_eq!(pasted_name(".."), "pasted-file");
        assert_eq!(pasted_name(""), "pasted-file");
    }

    #[test]
    fn a_saved_file_is_accepted_in_any_folder_it_was_saved_to() {
        let data = scratch("saved");
        let old = data.join("old-downloads");
        let new = data.join("Downloads");
        file(&old.join("a.txt"), b"a");
        file(&new.join("b.txt"), b"b");
        let saved = vec![old.join("a.txt"), new.join("b.txt")];
        assert!(validate_saved(&saved, &old.join("a.txt")).is_ok());
        assert!(validate_saved(&saved, &new.join("..").join("Downloads").join("b.txt")).is_ok());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_file_dissent_did_not_save_is_refused_even_inside_a_download_folder() {
        let data = scratch("unsaved");
        let downloads = data.join("Downloads");
        file(&downloads.join("saved.txt"), b"a");
        file(&downloads.join("other.txt"), b"b");
        let saved = vec![downloads.join("saved.txt")];
        assert!(validate_saved(&saved, &downloads.join("other.txt")).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn dot_dot_out_of_a_download_folder_to_an_unsaved_file_is_refused() {
        let data = scratch("dotdot");
        let downloads = data.join("Downloads");
        file(&downloads.join("a.txt"), b"a");
        file(&data.join("secret.txt"), b"x");
        let saved = vec![downloads.join("a.txt")];
        assert!(validate_saved(&saved, &downloads.join("..").join("secret.txt")).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_saved_file_that_was_moved_away_is_refused() {
        let data = scratch("moved");
        let downloads = data.join("Downloads");
        file(&downloads.join("a.txt"), b"a");
        let saved = vec![downloads.join("a.txt")];
        std::fs::remove_file(downloads.join("a.txt")).unwrap();
        assert!(validate_saved(&saved, &downloads.join("a.txt")).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_symlink_to_an_unsaved_file_is_refused() {
        let data = scratch("symlink");
        let downloads = data.join("Downloads");
        file(&downloads.join("ok.txt"), b"a");
        let outside = data.join("outside.txt");
        file(&outside, b"x");
        let link = downloads.join("link.txt");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&outside, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, &link);
        if made.is_err() {
            // Creating symlinks on Windows needs Developer Mode or admin rights. Skip rather than fail there.
            eprintln!("skipped: cannot create a symlink in this environment");
        } else {
            let saved = vec![downloads.join("ok.txt")];
            assert!(validate_saved(&saved, &link).is_err());
        }
        let _ = std::fs::remove_dir_all(&data);
    }
}
