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
        dht_probe_every: Duration::from_secs(2),
    }
}

/// Makes `engine` behave as v0.1.6 did for direct profiles: it never starts a profile sync and
/// cannot answer one, and takes a pushed `Profile` only.
pub fn act_as_v016_profiles(engine: &crate::Engine) {
    engine.inner.legacy_profiles.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Makes `engine` behave as a released device from before `CallMsg::Features` does in calls: it sends
/// no `Features`, and answers one as an undecodable request.
pub fn act_as_released_calls(engine: &crate::Engine) {
    engine.inner.legacy_calls.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Makes every community's boost level read as `level` on `engine`, as if that many members hosted
/// relays. `None` goes back to counting hosts.
pub fn set_boost_level(engine: &crate::Engine, level: Option<u8>) {
    engine
        .inner
        .boost_level_override
        .store(level.unwrap_or(u8::MAX), std::sync::atomic::Ordering::Relaxed);
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

/// Makes `engine` act as if it were in a call, or stop. Relay probes wait during calls.
pub fn pretend_in_call(engine: &crate::Engine, on: bool) {
    engine.inner.pretend_in_call.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// How many hosted relays `engine` has probed, pass or not.
pub fn relays_probed(engine: &crate::Engine) -> usize {
    engine.read().relay_probed.len()
}

/// `engine`'s measured upload in its current call, in kbps. `None` before the first measurement.
pub fn own_call_capacity(engine: &crate::Engine) -> Option<u32> {
    engine.own_call_capacity()
}

/// The forwarder election in `engine`'s current call over measured capacities alone: how many live
/// devices have one, and the sender id it picks. The default capacity is never used here.
pub fn measured_election(engine: &crate::Engine) -> (usize, Option<u64>) {
    engine.measured_election()
}

/// How many hosted relays `engine` holds a passed check for.
pub fn relay_checks(engine: &crate::Engine) -> usize {
    engine.read().relay_checks.len()
}

/// Publishes `relays` as hosted by this person in `community` without the checks
/// `set_hosted_relays` runs, as another app could. Replay accepts any https URL.
pub async fn publish_hosted_relays_unchecked(
    engine: &crate::Engine,
    community: dissent_identity::CommunityId,
    relays: Vec<String>,
) -> crate::Result<()> {
    engine
        .author_publish(community, dissent_events::ChannelId::CONTROL, dissent_events::Payload::RelayHostSet { relays })
        .await
        .map(|_| ())
}

/// Runs one relay-host check now, as the maintenance tick does. See `boosts`.
pub async fn check_relay_hosts(engine: &crate::Engine) {
    engine.check_relay_hosts().await;
}
