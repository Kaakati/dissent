//! Helpers for tests. Not for production use: the vaults here are random and held in memory.

use dissent_identity::Vault;

/// Mutes a device's topic gossip, so only its direct beacons reach peers. See `presence::mute_gossip_for_tests`.
#[cfg(feature = "test-hooks")]
pub use crate::presence::mute_gossip_for_tests;

/// Maps a link preview host to a loopback server. See `previews::map_preview_host`.
#[cfg(feature = "test-hooks")]
pub use crate::previews::map_preview_host;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type Registry = Mutex<HashMap<PathBuf, Arc<Vault>>>;

/// The vault for `dir`. A directory gets the same vault for the life of the process, so an engine
/// restarted on that directory can open the state it wrote.
pub fn vault_for(dir: &Path) -> Arc<Vault> {
    static VAULTS: OnceLock<Registry> = OnceLock::new();
    let mut vaults = VAULTS
        .get_or_init(Registry::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    vaults
        .entry(dir.to_path_buf())
        .or_insert_with(|| Arc::new(Vault::ephemeral_for_tests()))
        .clone()
}

/// Engine intervals short enough for tests. Everything stays well apart: a beacon still arrives
/// several times inside its presence window, so one late beacon does not flicker a peer offline.
pub fn fast_timings() -> crate::Timings {
    use std::time::Duration;
    crate::Timings {
        history_first_retry: Duration::from_secs(3),
        presence_window: Duration::from_secs(8),
        beacon_every: Duration::from_secs(1),
        presence_sweep: Duration::from_secs(1),
        ring_timeout: Duration::from_secs(8),
        tier_sustain: Duration::from_secs(2),
        reelect_sustain: Duration::from_secs(2),
        call_presence_every: Duration::from_secs(2),
        call_presence_ttl: Duration::from_secs(6),
        typing_expiry: Duration::from_secs(2),
        typing_every: Duration::from_secs(1),
    }
}

/// Makes `engine` behave as v0.1.6 did for direct profiles: it never starts a profile sync and
/// cannot answer one, and takes a pushed `Profile` only.
pub fn act_as_v016_profiles(engine: &crate::Engine) {
    engine.inner.legacy_profiles.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The relay URLs this engine's node has adopted, as strings. Empty when the node has none.
pub fn adopted_relays(engine: &crate::Engine) -> Vec<String> {
    engine
        .inner
        .node
        .get()
        .map(|node| node.relays().iter().map(|url| url.to_string()).collect())
        .unwrap_or_default()
}

/// Makes `engine` answer `DmDevicesSupport` with `on`. Off models a device from before multi-device direct
/// conversations, which is never added to a conversation made before it linked.
pub fn set_dm_devices_support(engine: &crate::Engine, on: bool) {
    engine
        .inner
        .dm_devices_support
        .store(on, std::sync::atomic::Ordering::Relaxed);
}
