//! "Hide my IP through Tor": the bridge to the Tor this person runs, and its status for the indicator. The node,
//! the engine's fetches and the updater all go through the bridge (`dissent_core::SocksBridge`), which carries every
//! connection through Tor's SOCKS port by host name. Nothing has another way out: with Tor down the bridge refuses
//! every tunnel, and the app is offline until Tor is back.

use std::net::SocketAddr;
use std::time::Duration;

use dissent_core::{socks_bridge, SocksBridge};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use url::Url;

use crate::commands::AppState;
use crate::network::{self, IpMode, NetworkSettings};

/// How long the status check waits for Tor to open a connection to a relay.
const CHECK_WAIT: Duration = Duration::from_secs(20);

/// The running bridge, while Tor mode is on. Replaced when the SOCKS address changes, dropped when Tor mode is off.
#[derive(Default)]
pub struct Tor(tokio::sync::Mutex<Option<SocksBridge>>);

/// What the indicator shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrivacyStatus {
    pub mode: IpMode,
    /// Tor's SOCKS address, in Tor mode.
    pub tor_socks: Option<String>,
    /// In Tor mode: Tor opened a connection to a relay just now. `None` in the other modes.
    pub tor_ok: Option<bool>,
    /// Why Tor could not, when it could not.
    pub tor_error: Option<String>,
}

/// The bridge's address for these settings: started, or restarted on a new SOCKS address, in Tor mode; stopped and
/// `None` otherwise. An error in Tor mode means nothing may start: the engine would have no way out but direct.
pub async fn proxy_for(app: &AppHandle, settings: &NetworkSettings) -> Result<Option<Url>, String> {
    let tor = app.state::<Tor>();
    let mut bridge = tor.0.lock().await;
    if settings.ip_mode() != IpMode::Tor {
        *bridge = None;
        return Ok(None);
    }
    let socks = settings.tor_socks_addr()?;
    if bridge.as_ref().is_none_or(|b| b.socks() != socks) {
        *bridge = Some(SocksBridge::spawn(socks).await.map_err(|e| e.to_string())?);
    }
    // The node's connections are the relay's purpose: their own Tor circuit, apart from previews and updates.
    Ok(bridge.as_ref().map(|b| b.url("relay")))
}

/// The bridge's address for update checks while it runs: a Tor circuit of their own.
pub async fn running_proxy(app: &AppHandle) -> Option<Url> {
    let tor = app.try_state::<Tor>()?;
    let bridge = tor.0.lock().await;
    bridge.as_ref().map(|b| b.url("updates"))
}

/// The relay Tor is asked to reach for the status check: the first one this device uses.
fn check_target(settings: &NetworkSettings) -> Option<(String, u16)> {
    let first = network::enabled(settings).into_iter().next()?;
    let host = first.url.host_str()?.trim_end_matches('.').to_string();
    Some((host, first.url.port_or_known_default().unwrap_or(443)))
}

async fn status(settings: &NetworkSettings) -> PrivacyStatus {
    let mode = settings.ip_mode();
    if mode != IpMode::Tor {
        return PrivacyStatus { mode, tor_socks: None, tor_ok: None, tor_error: None };
    }
    let socks: Result<SocketAddr, String> = settings.tor_socks_addr();
    let checked = match (socks, check_target(settings)) {
        (Ok(socks), Some((host, port))) => socks_bridge::reachable(socks, &host, port, CHECK_WAIT).await,
        (Err(e), _) => Err(e),
        (_, None) => Err("no relay to reach".into()),
    };
    PrivacyStatus {
        mode,
        tor_socks: Some(settings.tor_socks.clone()),
        tor_ok: Some(checked.is_ok()),
        tor_error: checked.err(),
    }
}

/// How this device hides its IP now, and in Tor mode whether Tor is up. The check opens one connection through Tor to
/// a relay this device uses, and nothing else.
#[tauri::command]
pub async fn privacy_status(state: tauri::State<'_, AppState>) -> Result<PrivacyStatus, String> {
    let settings = network::load(&state.data_dir);
    Ok(status(&settings).await)
}
