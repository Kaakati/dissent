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
        forward_offer_every: Duration::from_secs(1),
    }
}

/// Asks `from` for the ciphertext of `hash` from `engine`'s node, as any device that learned the hash could, and keeps
/// it. The error says why, `refused` when the provider's blob server refused this device.
pub async fn fetch_raw_blob(engine: &crate::Engine, from: &dissent_net::PeerAddr, hash: [u8; 32], cipher_size: u64) -> Result<(), String> {
    let addr = from.clone().to_iroh().map_err(|e| e.to_string())?;
    let policy = dissent_files::MirrorPolicy { max_file_size: u64::MAX / 4, max_total: u64::MAX / 2 };
    let node = engine.node();
    match engine.inner.files.mirror(node.endpoint(), &[addr], hash, cipher_size, policy, |_, _| {}).await {
        Ok(()) => Ok(()),
        Err(dissent_files::FilesError::Refused(_)) => Err("refused".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// The blobs `engine`'s store keeps pinned. A blob that is no longer pinned goes with the next garbage collection.
pub async fn pinned_blobs(engine: &crate::Engine) -> Vec<[u8; 32]> {
    engine.inner.files.pinned_hashes().await.unwrap_or_default()
}

/// Makes `engine`'s blob server treat `device` as a device it holds no certificate for, until the server asks the
/// device for its record. Stands in for a member who joined while `engine` was away.
pub fn gate_forget_device(engine: &crate::Engine, device: dissent_identity::DeviceId) {
    engine.write().blob_scopes.forgotten.insert(device);
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

/// Makes `engine`'s history readiness check ask member devices, or not. Off, no device answers, so only the
/// devices' own `HistorySupport` events count.
pub fn set_history_probes(engine: &crate::Engine, on: bool) {
    engine.inner.history_probes.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Turns `engine`'s support for history messages on or off, as an upgrade or a downgrade would. It declares
/// the change in each community.
pub fn set_history_support(engine: &crate::Engine, on: bool) {
    engine.set_history_support(on);
}

/// Whether `engine`'s replay of `community` holds a `HistorySupport` from `device`, and its value.
pub fn history_declaration(engine: &crate::Engine, community: dissent_identity::CommunityId, device: dissent_identity::DeviceId) -> Option<bool> {
    engine.read().communities.get(&community)?.state().history_support.get(&device).map(|(_, yes)| *yes)
}

/// Whether `engine` would send a message in `community` in the history form now, from its readiness flag.
pub fn history_ready_now(engine: &crate::Engine, community: dissent_identity::CommunityId) -> bool {
    engine.history_ready_now(community)
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

/// Turns simulcast layers on or off for `engine`. Off, it sends one stream and never announces
/// layers, as a released client does.
pub fn set_simulcast(engine: &crate::Engine, on: bool) {
    engine.inner.simulcast.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The simulcast layers `engine`'s camera (or screen) was asked for by its forwarder, as a mask.
/// `None` while it sends one stream, as in a mesh.
pub fn wanted_layers(engine: &crate::Engine, screen: bool) -> Option<u8> {
    engine.wanted_layers_now(screen)
}

/// The simulcast layer of the last frame `engine` received of `sender_root`'s camera (or screen)
/// in its current call.
pub fn received_layer(engine: &crate::Engine, sender_root: &str, screen: bool) -> Option<u8> {
    engine.received_layer_now(sender_root, screen)
}

/// On the forwarder `engine`: the layer it plans to send `viewer_root` of `sender_root`'s camera (or
/// screen) now. `None` while that stream is paused for the viewer, or has no layer table.
pub async fn planned_layer(engine: &crate::Engine, viewer_root: &str, sender_root: &str, screen: bool) -> Option<u8> {
    engine.planned_layer_now(viewer_root, sender_root, screen).await
}

/// On the forwarder `engine`: the root id of the active speaker, whom a tight budget keeps sharp.
pub async fn active_speaker(engine: &crate::Engine) -> Option<String> {
    engine.active_speaker_now().await
}

/// On the forwarder `engine`: the layers `sender_root`'s camera announced, as (width, kbps),
/// smallest first.
pub async fn sender_layers(engine: &crate::Engine, sender_root: &str) -> Vec<(u32, u32)> {
    engine.sender_layers_now(sender_root).await
}

/// On the forwarder `engine`: holds `viewer_root`'s downlink to `kbps` of video, as a measured
/// estimate would. Returns false when `engine` does not forward its call.
pub async fn set_viewer_budget(engine: &crate::Engine, viewer_root: &str, kbps: u32) -> bool {
    engine.set_viewer_budget_now(viewer_root, kbps).await
}

/// Whether downlink bandwidth estimates cap budgets on `engine`'s relay. Off, a set budget is the
/// only limit. Returns false when `engine` does not forward the call.
pub async fn use_link_estimates(engine: &crate::Engine, on: bool) -> bool {
    engine.use_link_estimates_now(on).await
}

/// On a volunteer `engine`: how many leaves its relays for calls it is not in carry.
pub async fn relay_downlinks(engine: &crate::Engine) -> usize {
    engine.relay_downlinks_now().await
}

/// On a volunteer `engine`: makes its relays keep their links but drop every packet while `on`, as a
/// stalled or hostile forwarder would.
pub async fn drop_relayed(engine: &crate::Engine, on: bool) {
    engine.drop_relayed_now(on).await;
}

/// How many live offers to forward `engine`'s current call it holds from volunteers.
pub fn forward_offers(engine: &crate::Engine) -> usize {
    engine.forward_offers_now()
}

/// Sends `engine`'s signed offer to forward the call in `channel` straight to `device`, as a
/// volunteer would, and returns the reply: "ack", or why it was refused.
pub async fn offer_to_forward(
    engine: &crate::Engine,
    community: dissent_identity::CommunityId,
    channel: dissent_events::ChannelId,
    device: dissent_identity::DeviceId,
) -> String {
    engine.offer_to_forward_now(community, channel, device).await
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
