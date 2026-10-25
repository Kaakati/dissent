//! Helpers for tests. Not for production use: the vaults here are random and held in memory.

use dissent_identity::Vault;
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
