//! Which profile this process opens, and whether it may open it.
//!
//! Profiles:
//! - the default profile: the app data folder, keyed by the OS credential store;
//! - a `DISSENT_DATA_DIR` profile (dev and tests), also keyed by the credential store;
//! - the installed profile, opened by a portable exe whose choice is `installed`. It is the installed app's
//!   own folder and key, so the two share one profile and the lock keeps them from running together;
//! - a portable profile: `DissentData` next to the exe, when the exe folder holds a `portable` or
//!   `dissent-portable` marker. Its data key comes from a passphrase, because the OS credential store
//!   belongs to the machine, not the folder.
//!
//! A portable exe's first run has no `portable-choice` file. The chooser asks how to use profiles, saves the
//! answer, and restarts. See [`Choice`].
//!
//! One process per profile: [`lock`] holds `<data dir>/.lock` for the life of the process.

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dissent_core::{copy_profile, CoreError, Engine};
use dissent_identity::{keystore, Identity, IdentityError, Vault};
use serde::Serialize;
use tauri::{App, AppHandle, Manager, Runtime};

use crate::commands::{self, AppState};
use crate::part6::PendingLink;
use crate::{attachments, avatar_cmds, events, network, status_cmds, storage, tray, updates, voice, voice_settings};

/// The identifier Tauri uses for the app data folder. The installed app's profile lives under it.
const IDENTIFIER: &str = "chat.dissent.desktop";
/// The file that records how a portable exe uses profiles: `installed`, `link`, `copy` or `fresh`.
const CHOICE_FILE: &str = "portable-choice";
const PORTABLE_MARKERS: [&str; 2] = ["portable", "dissent-portable"];
const PORTABLE_DIR: &str = "DissentData";
const LOCK_FILE: &str = ".lock";
const IDENTITY_FILE: &str = "identity.bin";
const SALT_FILE: &str = "vault.salt";
/// The credential-store entry the installed app keeps its data key under.
const INSTALLED_KEY_ENTRY: &str = "identity";
/// The shortest passphrase a new portable profile accepts.
const MIN_PASSPHRASE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The app data folder. The data key is in the OS credential store.
    Default,
    /// `DISSENT_DATA_DIR`. The data key is in the OS credential store, under a per-dir entry.
    Custom,
    /// A portable exe that shares the installed app's profile. Same folder and key as `Default`.
    Installed,
    /// `DissentData` next to the exe. The data key comes from a passphrase.
    Portable,
    /// A portable exe on its first run. No profile is opened until the chooser has saved a choice.
    Undecided,
}

/// How a portable exe uses profiles, as chosen on its first run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open the profile the installed app uses on this computer. Nothing is copied.
    Installed,
    /// A new device of the user's identity, approved from the installed app.
    Link,
    /// A copy of the installed profile, re-sealed under a passphrase.
    Copy,
    /// A new identity under a passphrase, or one restored from a recovery phrase.
    Fresh,
}

impl Choice {
    fn word(self) -> &'static str {
        match self {
            Choice::Installed => "installed",
            Choice::Link => "link",
            Choice::Copy => "copy",
            Choice::Fresh => "fresh",
        }
    }

    fn parse(text: &str) -> Option<Choice> {
        match text.trim() {
            "installed" => Some(Choice::Installed),
            "link" => Some(Choice::Link),
            "copy" => Some(Choice::Copy),
            "fresh" => Some(Choice::Fresh),
            _ => None,
        }
    }
}

/// The folder holding the running exe, when it can be found.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

fn has_marker(dir: &Path) -> bool {
    PORTABLE_MARKERS.iter().any(|m| dir.join(m).is_file())
}

/// Whether the running exe is a portable build, whichever profile it uses.
pub fn is_portable_exe() -> bool {
    exe_dir().is_some_and(|dir| has_marker(&dir))
}

fn read_choice_in(dir: &Path) -> Option<Choice> {
    std::fs::read_to_string(dir.join(CHOICE_FILE)).ok().and_then(|text| Choice::parse(&text))
}

fn write_choice_in(dir: &Path, choice: Choice) -> io::Result<()> {
    let tmp = dir.join(format!("{CHOICE_FILE}.tmp"));
    std::fs::write(&tmp, choice.word())?;
    std::fs::rename(&tmp, dir.join(CHOICE_FILE))
}

/// The choice saved next to the exe, if any.
fn choice() -> Option<Choice> {
    exe_dir().and_then(|dir| read_choice_in(&dir))
}

/// The folder of the installed app's profile on this computer. Only Windows builds have one here.
pub fn installed_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join(IDENTIFIER))
    } else {
        None
    }
}

/// Whether the installed app's profile exists, so the options that use it can be offered.
fn installed_profile_exists() -> bool {
    installed_dir().is_some_and(|dir| dir.join(IDENTITY_FILE).is_file())
}

/// The profile this process uses. `DISSENT_DATA_DIR` wins over everything. A portable exe follows its choice.
pub fn mode() -> Mode {
    let exe = exe_dir();
    mode_for(
        std::env::var_os("DISSENT_DATA_DIR"),
        exe.as_deref().is_some_and(has_marker),
        exe.as_deref().and_then(read_choice_in),
        exe.as_deref().is_some_and(|dir| dir.join(PORTABLE_DIR).exists()),
        installed_profile_exists(),
    )
}

fn mode_for(custom: Option<OsString>, marker: bool, choice: Option<Choice>, portable_data: bool, installed: bool) -> Mode {
    if custom.is_some_and(|d| !d.is_empty()) {
        return Mode::Custom;
    }
    if !marker {
        return Mode::Default;
    }
    match choice {
        Some(Choice::Installed) if installed => Mode::Installed,
        // The installed profile has gone, so the choice can't be honoured. Ask again.
        Some(Choice::Installed) => Mode::Undecided,
        Some(Choice::Link | Choice::Copy | Choice::Fresh) => Mode::Portable,
        // A portable folder from before choices existed keeps working as it did.
        None if portable_data => Mode::Portable,
        None => Mode::Undecided,
    }
}

/// The folder that holds the profile for `mode`. The default and installed profiles use the app data folder.
pub fn data_dir(app: &App, mode: Mode) -> tauri::Result<PathBuf> {
    match mode {
        Mode::Default | Mode::Installed => app.path().app_data_dir(),
        _ => Ok(fixed_dir(mode)?),
    }
}

/// The folder for a custom or portable profile. It needs no app handle, so it is known before Tauri starts.
fn fixed_dir(mode: Mode) -> io::Result<PathBuf> {
    match mode {
        Mode::Custom => Ok(PathBuf::from(std::env::var_os("DISSENT_DATA_DIR").unwrap_or_default())),
        _ => Ok(exe_dir().ok_or_else(|| io::Error::other("cannot find the folder of the exe"))?.join(PORTABLE_DIR)),
    }
}

/// A portable profile that is still waiting for its copy. Its folder is created only when the copy succeeds.
pub fn copy_pending(mode: Mode) -> bool {
    mode == Mode::Portable && choice() == Some(Choice::Copy) && fixed_dir(mode).is_ok_and(|dir| !dir.exists())
}

/// Credential-store entry name for this data dir. The default dir uses "identity". A `DISSENT_DATA_DIR`
/// gets its own entry, so two test instances on one machine don't share a key.
pub fn vault_profile(data_dir: &Path) -> String {
    if std::env::var_os("DISSENT_DATA_DIR").is_some_and(|d| !d.is_empty()) {
        let hash = blake3::hash(data_dir.to_string_lossy().as_bytes()).to_hex();
        format!("identity-{}", &hash.as_str()[..16])
    } else {
        "identity".to_string()
    }
}

/// The default profile is one window per machine: a second launch focuses the first and exits.
/// Dev, installed-choice and portable profiles don't register this plugin, so they can run beside it.
pub fn single_instance<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_single_instance::init(|app: &AppHandle<R>, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    })
}

/// The lock a running profile holds. The OS releases it when the process exits.
#[allow(dead_code)]
pub struct ProfileLock(File);

/// Takes the lock on `data_dir`. `None` means another process has this profile open.
pub fn lock(data_dir: &Path) -> io::Result<Option<ProfileLock>> {
    let file = OpenOptions::new().create(true).write(true).truncate(false).open(data_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(ProfileLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Takes the lock for a custom, installed or portable profile before Tauri starts, and returns it for the
/// life of the process. A refused launch shows its message and exits here, so it never creates a window or a
/// WebView2 instance on the running copy's data folder. The default profile is handled in setup instead.
pub fn claim_early() -> Option<ProfileLock> {
    let mode = mode();
    let dir = match mode {
        Mode::Default | Mode::Undecided => return None,
        Mode::Portable if copy_pending(mode) => return None,
        Mode::Installed => installed_dir().unwrap_or_default(),
        _ => match fixed_dir(mode) {
            Ok(dir) => dir,
            Err(e) => fatal(&format!("Dissent cannot find its data folder: {e}")),
        },
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        fatal(&format!("Dissent cannot create its data folder: {e}"));
    }
    match lock(&dir) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::warn!("this profile is open in another Dissent process; refusing to start");
            show_message("Dissent is already open with this profile.");
            std::process::exit(0);
        }
        Err(e) => fatal(&format!("Dissent cannot lock its data folder: {e}")),
    }
}

/// Set while a refused launch waits for its message. It keeps the event loop alive, because with no
/// window to show Tauri would otherwise exit before the user reads the message.
pub struct Refusal;

/// Whether this launch is refused. lib.rs keeps the process alive while this is true.
pub fn is_refusing(app: &AppHandle) -> bool {
    app.try_state::<Refusal>().is_some()
}

/// Tells the user this profile is open elsewhere, then exits with code 0. Nothing has been opened yet.
/// The message waits on a worker thread: setup runs on the main thread, so the Tauri dialog plugin would
/// queue behind it. The process exits once the user dismisses the message.
pub fn refuse_second_instance(app: &App) {
    tracing::warn!("this profile is open in another Dissent process; refusing to start");
    app.manage(Refusal);
    std::thread::spawn(|| {
        show_message("Dissent is already open with this profile.");
        std::process::exit(0);
    });
}

/// A startup failure the user has to see. Logs it, shows a native error box, then exits with a failure code.
/// The window is still hidden at this point, so nothing is left running on screen and nothing exits silently.
pub fn fatal(error: &str) -> ! {
    tracing::error!(error, "Dissent could not open its profile");
    show_message(&format!(
        "Dissent couldn't open your profile.\n\n{error}\n\nYour data hasn't been changed. Please report this at https://github.com/Kaakati/dissent/issues"
    ));
    std::process::exit(1);
}

/// Shows the main window. It starts hidden, so a refused or still-locked launch never shows an empty app.
pub fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
}

/// Windows: a plain OK box that stays until the user presses OK. It is topmost, so it isn't left behind.
#[cfg(windows)]
fn show_message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (text, title) = (wide(text), wide("Dissent"));
    // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call. A null owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn show_message(text: &str) {
    rfd::MessageDialog::new()
        .set_title("Dissent")
        .set_description(text)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// A portable profile waiting for its passphrase. Managed at startup. `AppState` appears once it opens.
pub struct Locked {
    data_dir: PathBuf,
    /// Serialises unlock attempts, so two calls can't open the engine twice.
    gate: tokio::sync::Mutex<()>,
    /// The passphrase vault of a link that waits for its grant. `finish_portable_link` takes it.
    pending: Mutex<Option<Vault>>,
}

impl Locked {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, gate: tokio::sync::Mutex::new(()), pending: Mutex::new(None) }
    }
}

/// Managed on a portable exe's first run, until the chooser has saved a choice.
pub struct Chooser;

#[derive(Serialize)]
pub struct VaultStatus {
    /// True until the user has picked how this exe uses profiles. The chooser then covers the window.
    chooser: bool,
    /// True until the profile is open. Only a portable profile starts locked.
    locked: bool,
    /// True when no identity exists yet, so the screen asks for a new passphrase.
    new_profile: bool,
    /// The saved choice: "installed", "link", "copy" or "fresh". None before a choice.
    choice: Option<&'static str>,
    /// Whether the installed app's profile is on this computer. Only then are the options that use it offered.
    installed_profile: bool,
}

/// What the UI must do before anything else: choose, unlock, or go on.
#[tauri::command]
pub fn vault_status(app: AppHandle) -> VaultStatus {
    let chooser = app.try_state::<Chooser>().is_some();
    let open = app.try_state::<AppState>().is_some();
    let (locked, new_profile) = match app.try_state::<Locked>() {
        Some(locked) if !open && !chooser => (true, !locked.data_dir.join(IDENTITY_FILE).exists()),
        _ => (false, false),
    };
    VaultStatus {
        chooser,
        locked,
        new_profile,
        choice: choice().map(Choice::word),
        installed_profile: installed_profile_exists(),
    }
}

/// Saves the choice made on a portable exe's first run, then restarts so the app starts in that mode.
#[tauri::command]
pub fn choose_profile(app: AppHandle, choice: String) -> Result<(), String> {
    let choice = Choice::parse(&choice).ok_or("That is not one of the options.")?;
    if choice == Choice::Installed && !installed_profile_exists() {
        return Err("No Dissent profile was found on this computer.".into());
    }
    let dir = exe_dir().ok_or("Cannot find the folder of Dissent.exe.")?;
    write_choice_in(&dir, choice).map_err(|e| format!("Cannot save the choice: {e}"))?;
    app.restart()
}

/// Opens a portable profile with its passphrase. A wrong passphrase is refused and nothing starts. Returns
/// "link" when this device must be linked from the installed app first, or "open" when the profile is open.
#[tauri::command]
pub async fn unlock_profile(app: AppHandle, passphrase: String) -> Result<String, String> {
    let locked = app.try_state::<Locked>().ok_or("This profile does not wait for a passphrase.")?;
    let _gate = locked.gate.lock().await;
    if app.try_state::<AppState>().is_some() {
        return Err("This profile is already open.".into());
    }
    let data_dir = locked.data_dir.clone();
    let creating = !data_dir.join(IDENTITY_FILE).exists();
    if creating && passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(format!("Use at least {MIN_PASSPHRASE} characters."));
    }
    match (creating, choice()) {
        (true, Some(Choice::Copy)) => {
            let salt: [u8; 16] = rand::random();
            let vault = derive(passphrase, salt).await?;
            copy_installed(&app, &data_dir, vault, salt).await?;
            Ok("open".into())
        }
        (true, Some(Choice::Link)) => {
            let salt = salt(&data_dir, true)?;
            let vault = derive(passphrase, salt).await?;
            *locked.pending.lock().map_err(|e| e.to_string())? = Some(vault);
            Ok("link".into())
        }
        _ => {
            let salt = salt(&data_dir, creating)?;
            let vault = derive(passphrase, salt).await?;
            let identity = keystore::load_or_create(&data_dir.join(IDENTITY_FILE), commands::now_ms()?, &vault).map_err(|e| match e {
                IdentityError::Crypto(_) => "That passphrase is wrong.".to_string(),
                other => other.to_string(),
            })?;
            open_profile(&app, data_dir, vault, identity).await?;
            Ok("open".into())
        }
    }
}

/// Takes the device grant for a linked portable profile, saves the identity under the passphrase vault, and
/// opens the profile. The link code comes from `link_device_code`, which is called first.
#[tauri::command]
pub async fn finish_portable_link(app: AppHandle, grant: String) -> Result<(), String> {
    let locked = app.try_state::<Locked>().ok_or("This profile does not wait for a link.")?;
    let _gate = locked.gate.lock().await;
    if app.try_state::<AppState>().is_some() {
        return Err("This profile is already open.".into());
    }
    let vault = locked
        .pending
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or("Enter your passphrase before linking.")?;
    let pending = app.state::<PendingLink>();
    let request = pending.0.lock().map_err(|e| e.to_string())?.take().ok_or("No device link is in progress. Get a new link code.")?;
    let identity = match request.finish(&grant) {
        Ok(identity) => identity,
        Err(e) => {
            // The vault is kept, so the user can try again with a grant for the new code.
            *locked.pending.lock().map_err(|e| e.to_string())? = Some(vault);
            return Err(format!("That grant does not match this link: {e}"));
        }
    };
    keystore::save(&identity, &locked.data_dir.join(IDENTITY_FILE), &vault).map_err(|e| e.to_string())?;
    open_profile(&app, locked.data_dir.clone(), vault, identity).await
}

/// Copies the installed profile into this portable folder under the passphrase vault. The installed app must be
/// closed. Nothing is written into the folder until the copy has been verified, and the installed profile is
/// only read.
async fn copy_installed(app: &AppHandle, data_dir: &Path, vault: Vault, salt: [u8; 16]) -> Result<(), String> {
    let source = installed_dir()
        .filter(|dir| dir.join(IDENTITY_FILE).is_file())
        .ok_or("No Dissent profile was found on this computer.")?;
    // The installed app holds this lock while it runs, so the copy never reads a profile that is changing.
    let installed = lock(&source)
        .map_err(|e| e.to_string())?
        .ok_or("Close Dissent on this computer first, then try again.")?;
    let dest = data_dir.to_path_buf();
    let (vault, identity) = tokio::task::spawn_blocking(move || -> Result<(Vault, Identity), String> {
        // Held until the copy is finished, then released.
        let _installed = installed;
        let from = Vault::open("dissent", INSTALLED_KEY_ENTRY).map_err(|e| e.to_string())?;
        copy_profile(&source, &from, &dest, &vault, |staged| {
            std::fs::write(staged.join(SALT_FILE), salt).map_err(CoreError::from)
        })
        .map_err(|e| e.to_string())?;
        let identity = keystore::load(&dest.join(IDENTITY_FILE), &vault).map_err(|e| e.to_string())?;
        Ok((vault, identity))
    })
    .await
    .map_err(|e| e.to_string())??;
    // The portable folder was not locked while the copy was pending. Lock it now, for the life of the process.
    let portable = lock(data_dir)
        .map_err(|e| e.to_string())?
        .ok_or("Another copy of Dissent is using this folder.")?;
    app.manage(portable);
    open_profile(app, data_dir.to_path_buf(), vault, identity).await
}

/// Argon2 takes about a second, so it runs off the async runtime.
async fn derive(passphrase: String, salt: [u8; 16]) -> Result<Vault, String> {
    tokio::task::spawn_blocking(move || Vault::from_passphrase(&passphrase, &salt))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The 16-byte Argon2 salt kept beside the profile. The salt is not secret. A new profile gets a fresh one.
fn salt(data_dir: &Path, creating: bool) -> Result<[u8; 16], String> {
    let path = data_dir.join(SALT_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => <[u8; 16]>::try_from(bytes.as_slice()).map_err(|_| "The profile's salt file is damaged.".to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound && creating => {
            let salt: [u8; 16] = rand::random();
            std::fs::write(&path, salt).map_err(|e| e.to_string())?;
            Ok(salt)
        }
        Err(e) => Err(format!("cannot read the profile's salt file: {e}")),
    }
}

/// Opens the engine on `data_dir` and wires up the app. Runs at startup for keychain profiles and after
/// the passphrase for portable ones. Nothing here runs before the profile is unlocked.
pub async fn open_profile(app: &AppHandle, data_dir: PathBuf, vault: Vault, identity: Identity) -> Result<(), String> {
    let vault = std::sync::Arc::new(vault);
    let profile = storage::profile_dir(&data_dir, &identity);
    storage::migrate_legacy(&data_dir, &profile).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    tracing::info!(dir = %profile.display(), "engine data dir");
    attachments::allow_downloads(app, &data_dir).map_err(|e| e.to_string())?;
    // The engine keeps pictures under the profile folder (`Store::avatars_dir`), not the data dir.
    avatar_cmds::allow_avatars(app, &profile).map_err(|e| e.to_string())?;

    let settings = network::load(&data_dir);
    let voice = voice::Voice::new(data_dir.clone());
    let (engine, rx) = Engine::start(identity.clone(), profile, crate::node_config(&settings), vault.clone())
        .await
        .map_err(|e| e.to_string())?;
    voice::prepare(&engine, &voice);
    app.manage(voice);
    voice_settings::restore_ptt(app);

    app.manage(AppState {
        data_dir,
        vault,
        identity: std::sync::Mutex::new(identity),
        engine: tokio::sync::RwLock::new(Some(engine)),
    });
    events::forward_events(app.clone(), rx);
    events::forward_notification_clicks(app);
    app.manage(status_cmds::LastInput::new());
    status_cmds::start_idle_watch(app.clone());
    tray::install(app).map_err(|e| e.to_string())?;
    updates::start_background(app.clone());
    attachments::install(app);
    show_window(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-profile-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_lock_on_one_dir_is_refused_and_another_dir_is_free() {
        let dir = scratch("lock");
        let other = scratch("lock-other");
        let first = lock(&dir).unwrap();
        assert!(first.is_some());
        assert!(lock(&dir).unwrap().is_none());
        assert!(lock(&other).unwrap().is_some());
        drop(first);
        assert!(lock(&dir).unwrap().is_some());
    }

    #[test]
    fn a_portable_marker_beside_the_exe_selects_portable_only_once_a_choice_is_made() {
        // Without a marker the exe is the default profile, whatever the choice file says.
        assert_eq!(mode_for(None, false, Some(Choice::Link), false, false), Mode::Default);
        // With a marker and no choice on a new folder, the chooser runs.
        assert_eq!(mode_for(None, true, None, false, false), Mode::Undecided);
        // A portable folder from before choices existed keeps working.
        assert_eq!(mode_for(None, true, None, true, false), Mode::Portable);
        assert_eq!(mode_for(None, true, Some(Choice::Fresh), false, false), Mode::Portable);
        assert_eq!(mode_for(None, true, Some(Choice::Link), false, false), Mode::Portable);
        assert_eq!(mode_for(None, true, Some(Choice::Copy), false, false), Mode::Portable);
    }

    #[test]
    fn the_installed_choice_needs_the_installed_profile() {
        assert_eq!(mode_for(None, true, Some(Choice::Installed), false, true), Mode::Installed);
        // The installed profile is gone, so the chooser runs again instead of creating one.
        assert_eq!(mode_for(None, true, Some(Choice::Installed), false, false), Mode::Undecided);
    }

    #[test]
    fn dissent_data_dir_wins_over_everything() {
        assert_eq!(mode_for(Some("some-dir".into()), true, Some(Choice::Installed), true, true), Mode::Custom);
        assert_eq!(mode_for(Some("".into()), true, Some(Choice::Fresh), false, false), Mode::Portable);
    }

    #[test]
    fn the_choice_file_is_saved_read_back_and_junk_is_ignored() {
        let dir = scratch("choice");
        assert_eq!(read_choice_in(&dir), None);
        for choice in [Choice::Installed, Choice::Link, Choice::Copy, Choice::Fresh] {
            write_choice_in(&dir, choice).unwrap();
            assert_eq!(read_choice_in(&dir), Some(choice));
        }
        std::fs::write(dir.join(CHOICE_FILE), "something else").unwrap();
        assert_eq!(read_choice_in(&dir), None);
    }

    #[test]
    fn the_salt_is_made_once_and_read_back() {
        let dir = scratch("salt");
        assert!(salt(&dir, false).is_err());
        let made = salt(&dir, true).unwrap();
        assert_eq!(salt(&dir, false).unwrap(), made);
    }

    #[test]
    fn a_wrong_passphrase_fails_as_crypto() {
        let dir = scratch("passphrase");
        let path = dir.join(IDENTITY_FILE);
        let right = Vault::from_passphrase("correct horse", &[7; 16]).unwrap();
        keystore::load_or_create(&path, 1, &right).unwrap();
        let wrong = Vault::from_passphrase("battery staple", &[7; 16]).unwrap();
        assert!(matches!(keystore::load(&path, &wrong), Err(IdentityError::Crypto(_))));
    }

    #[test]
    fn a_copy_reseals_under_the_passphrase_and_the_original_is_unchanged() {
        let root = scratch("copy-source");
        let dest = scratch("copy-dest").join(PORTABLE_DIR);
        let keychain = Vault::from_passphrase("stand-in for the keychain key", &[3; 16]).unwrap();
        let identity = Identity::create(1);
        keystore::save(&identity, &root.join(IDENTITY_FILE), &keychain).unwrap();
        let before = std::fs::read(root.join(IDENTITY_FILE)).unwrap();

        let passphrase = Vault::from_passphrase("correct horse", &[9; 16]).unwrap();
        copy_profile(&root, &keychain, &dest, &passphrase, |_| Ok(())).unwrap();

        // The original is untouched, and still opens only with the keychain key.
        assert_eq!(std::fs::read(root.join(IDENTITY_FILE)).unwrap(), before);
        assert!(keystore::load(&root.join(IDENTITY_FILE), &keychain).is_ok());
        // The copy opens with the passphrase, derived again from the same salt.
        let again = Vault::from_passphrase("correct horse", &[9; 16]).unwrap();
        assert_eq!(keystore::load(&dest.join(IDENTITY_FILE), &again).unwrap().device_id(), identity.device_id());
        assert!(keystore::load(&dest.join(IDENTITY_FILE), &keychain).is_err());
    }
}
