//! "Hide my IP through Tor": the bridge to Tor, and its status for the indicator. The node, the engine's fetches and
//! the updater all go through the bridge (`dissent_core::SocksBridge`), which carries every connection through a Tor
//! SOCKS port by host name. Nothing has another way out: with Tor down the bridge refuses every tunnel, and the app is
//! offline until Tor is back.
//!
//! The Tor is the one built into the app by default: arti, a program shipped next to the app's own and run by
//! `dissent_core::TorSidecar` in the profile's `tor` folder, on a port that changes each time it starts (the bridge
//! follows it). Or it is the Tor this person runs, at the SOCKS address in the settings.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dissent_core::{socks_bridge, SidecarState, SocksBridge, TorSidecar};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use url::Url;

use crate::network::{self, IpMode, NetworkSettings};

/// How long the status check waits for Tor to open a connection to a relay.
const CHECK_WAIT: Duration = Duration::from_secs(20);
/// The profile's folder for the built-in Tor: its config, state and directory cache.
const TOR_DIR: &str = "tor";

/// The running bridge and its Tor, while Tor mode is on. Replaced when the Tor changes, dropped when Tor mode is off.
#[derive(Default)]
pub struct Tor(tokio::sync::Mutex<Option<Running>>);

struct Running {
    bridge: SocksBridge,
    tor: Source,
}

enum Source {
    /// This person's Tor, at this SOCKS address.
    Own(SocketAddr),
    /// The built-in one.
    BuiltIn(TorSidecar),
}

/// What the indicator shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrivacyStatus {
    pub mode: IpMode,
    /// In Tor mode: whether the Tor is the one built into the app.
    pub tor_builtin: Option<bool>,
    /// Tor's SOCKS address, in Tor mode with this person's own Tor.
    pub tor_socks: Option<String>,
    /// In Tor mode: Tor opened a connection to a relay just now. `None` in the other modes.
    pub tor_ok: Option<bool>,
    /// Why Tor could not, when it could not.
    pub tor_error: Option<String>,
    /// The built-in Tor stopped for good: the Tor network no longer accepts its version, and only an update helps.
    pub tor_obsolete: bool,
}

/// The built-in Tor's program: `arti` next to the app's own (where the bundle puts it), or `DISSENT_ARTI` for
/// development. A build without it still starts the sidecar, which fails at once: Tor mode then stays offline and
/// says why.
fn arti_program() -> PathBuf {
    if let Some(path) = std::env::var_os("DISSENT_ARTI") {
        return PathBuf::from(path);
    }
    let name = format!("arti{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(&name))).unwrap_or_else(|| PathBuf::from(name))
}

/// The bridge's address for these settings and the profile in `profile`: started, or restarted for another Tor, in
/// Tor mode; stopped and `None` otherwise. An error in Tor mode means nothing may start: the engine would have no
/// way out but direct.
pub async fn proxy_for(app: &AppHandle, settings: &NetworkSettings, profile: &Path) -> Result<Option<Url>, String> {
    let tor = app.state::<Tor>();
    let mut running = tor.0.lock().await;
    if settings.ip_mode() != IpMode::Tor {
        stop_running(running.take()).await;
        return Ok(None);
    }
    let dir = profile.join(TOR_DIR);
    let own = if settings.tor_builtin() { None } else { Some(settings.tor_socks_addr()?) };
    let same = match (running.as_ref().map(|r| &r.tor), own) {
        (Some(Source::Own(at)), Some(wanted)) => *at == wanted,
        (Some(Source::BuiltIn(sidecar)), None) => sidecar.dir() == dir,
        _ => false,
    };
    if !same {
        stop_running(running.take()).await;
        *running = Some(match own {
            Some(socks) => Running { bridge: SocksBridge::spawn(socks).await.map_err(|e| e.to_string())?, tor: Source::Own(socks) },
            None => {
                let sidecar = TorSidecar::start(arti_program(), dir).map_err(|e| e.to_string())?;
                let bridge = SocksBridge::follow(sidecar.socks()).await.map_err(|e| e.to_string())?;
                Running { bridge, tor: Source::BuiltIn(sidecar) }
            }
        });
    }
    // The node's connections are the relay's purpose: their own Tor circuit, apart from previews and updates.
    Ok(running.as_ref().map(|r| r.bridge.url("relay")))
}

async fn stop_running(running: Option<Running>) {
    if let Some(Running { tor: Source::BuiltIn(sidecar), .. }) = running {
        sidecar.stop().await;
    }
}

/// Stops the bridge and the built-in Tor: when the app quits, and before an update replaces Tor's program.
pub async fn stop(app: &AppHandle) {
    let Some(tor) = app.try_state::<Tor>() else { return };
    let running = tor.0.lock().await.take();
    stop_running(running).await;
}

/// The bridge's address for update checks while it runs: a Tor circuit of their own.
pub async fn running_proxy(app: &AppHandle) -> Option<Url> {
    let tor = app.try_state::<Tor>()?;
    let running = tor.0.lock().await;
    running.as_ref().map(|r| r.bridge.url("updates"))
}

/// The relay Tor is asked to reach for the status check: the first one this device uses.
fn check_target(settings: &NetworkSettings) -> Option<(String, u16)> {
    let first = network::enabled(settings).into_iter().next()?;
    let host = first.url.host_str()?.trim_end_matches('.').to_string();
    Some((host, first.url.port_or_known_default().unwrap_or(443)))
}

/// Where the built-in Tor is, as the SOCKS address to check or why there is none.
fn builtin_socks(state: Option<SidecarState>) -> Result<SocketAddr, String> {
    match state {
        Some(SidecarState::Running(socks)) => Ok(socks),
        Some(SidecarState::Starting) => Err("The built-in Tor is starting.".into()),
        Some(SidecarState::Restarting(why)) => Err(format!("The built-in Tor stopped ({why}). Starting it again.")),
        Some(SidecarState::Obsolete) => Err("Tor stopped: update Dissent. The Tor network no longer accepts this version.".into()),
        Some(SidecarState::Failed(why)) => Err(format!("The built-in Tor could not start: {why}")),
        Some(SidecarState::Stopped) | None => Err("The built-in Tor is not running. It starts when the app connects.".into()),
    }
}

async fn status(app: &AppHandle, settings: &NetworkSettings) -> PrivacyStatus {
    let mode = settings.ip_mode();
    if mode != IpMode::Tor {
        return PrivacyStatus { mode, tor_builtin: None, tor_socks: None, tor_ok: None, tor_error: None, tor_obsolete: false };
    }
    let builtin = settings.tor_builtin();
    let sidecar = match app.try_state::<Tor>() {
        Some(tor) => match tor.0.lock().await.as_ref().map(|r| &r.tor) {
            Some(Source::BuiltIn(sidecar)) => Some(sidecar.state()),
            _ => None,
        },
        None => None,
    };
    let obsolete = builtin && sidecar == Some(SidecarState::Obsolete);
    let socks: Result<SocketAddr, String> = if builtin { builtin_socks(sidecar) } else { settings.tor_socks_addr() };
    let checked = match (socks, check_target(settings)) {
        (Ok(socks), Some((host, port))) => socks_bridge::reachable(socks, &host, port, CHECK_WAIT).await,
        (Err(e), _) => Err(e),
        (_, None) => Err("no relay to reach".into()),
    };
    PrivacyStatus {
        mode,
        tor_builtin: Some(builtin),
        tor_socks: (!builtin).then(|| settings.tor_socks.clone()),
        tor_ok: Some(checked.is_ok()),
        tor_error: checked.err(),
        tor_obsolete: obsolete,
    }
}

/// How this device hides its IP now, and in Tor mode whether Tor is up. The check opens one connection through Tor to
/// a relay this device uses, and nothing else.
#[tauri::command]
pub async fn privacy_status(app: AppHandle, state: tauri::State<'_, crate::commands::AppState>) -> Result<PrivacyStatus, String> {
    let settings = network::load(&state.data_dir);
    Ok(status(&app, &settings).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_indicator_says_why_the_built_in_tor_is_not_usable() {
        let socks: SocketAddr = "127.0.0.1:41234".parse().unwrap();
        assert_eq!(builtin_socks(Some(SidecarState::Running(socks))), Ok(socks));
        assert!(builtin_socks(Some(SidecarState::Obsolete)).unwrap_err().starts_with("Tor stopped: update Dissent"));
        assert!(builtin_socks(Some(SidecarState::Failed("no arti".into()))).unwrap_err().contains("no arti"));
        assert!(builtin_socks(Some(SidecarState::Restarting("it exited".into()))).is_err());
        assert!(builtin_socks(None).is_err());
    }
}
