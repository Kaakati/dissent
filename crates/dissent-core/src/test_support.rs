//! Helpers for tests. Not for production use: the vaults here are random and held in memory.

use dissent_identity::Vault;

/// Mutes a device's topic gossip, so only its direct beacons reach peers. See `presence::mute_gossip_for_tests`.
#[cfg(feature = "test-hooks")]
pub use crate::presence::mute_gossip_for_tests;

/// Moves `engine`'s call to forwarded (or back to a mesh), announcing it to only `reach` peers, as when most of them
/// see another roster and refuse the move. Returns false when there is no call to move.
#[cfg(feature = "test-hooks")]
pub fn force_tier_move(engine: &crate::Engine, forwarded: bool, reach: usize) -> bool {
    engine.force_tier_move_now(forwarded, reach)
}

/// Makes `engine` refuse every tier announcement, as a device that sees another roster does.
#[cfg(feature = "test-hooks")]
pub fn refuse_tier_moves(engine: &crate::Engine, on: bool) {
    engine.refuse_tier_moves_now(on);
}

/// Makes `engine` swap in its next tier move whatever links it made, as a build without the links-needed rule did,
/// so a test can leave a device alone in a room of its own. Only that one move: the rejoin after it keeps the rule.
#[cfg(feature = "test-hooks")]
pub fn swap_without_links(engine: &crate::Engine, on: bool) {
    engine.swap_without_links_now(on);
}

/// Starts a second tier move on `engine` while its first is still staged, past the guard that keeps moves one at a time.
#[cfg(feature = "test-hooks")]
pub fn start_second_move(engine: &crate::Engine, forwarded: bool) -> bool {
    engine.start_second_move_now(forwarded)
}

/// How many times `engine`'s current call room has run its loop. A number that stops growing means nothing drives it.
#[cfg(feature = "test-hooks")]
pub fn call_loop_ticks(engine: &crate::Engine) -> u64 {
    engine.call_loop_ticks_now()
}

/// Drops `engine`'s mesh call to `device`, as a link that died. False without one.
#[cfg(feature = "test-hooks")]
pub async fn drop_mesh_call(engine: &crate::Engine, device: dissent_identity::DeviceId) -> bool {
    engine.drop_mesh_call_now(device).await
}

/// Runs two connects from `engine` to `device` that cross: the second starts while the first is still in its handshake,
/// and each holds the answer it gets, so an answer comes back while the other connect waits for its own. Returns both
/// outcomes as text, or `None` when `engine` is not the side that offers to `device`.
#[cfg(feature = "test-hooks")]
pub async fn race_connects(engine: &crate::Engine, device: dissent_identity::DeviceId) -> Option<(String, String)> {
    let (first, second) = engine.race_connects_now(device).await?;
    let show = |r: crate::Result<()>| r.map_or_else(|e| e.to_string(), |()| "linked".into());
    Some((show(first), show(second)))
}

/// Whether `engine`'s mesh call to `device` works: connected, and heard from within the last few seconds.
#[cfg(feature = "test-hooks")]
pub async fn mesh_call_live(engine: &crate::Engine, device: dissent_identity::DeviceId) -> bool {
    engine.mesh_call_live_now(device).await
}

/// Holds back `engine`'s call `Features` for `hold`, as a joiner whose first one is slow to arrive on a loaded machine.
#[cfg(feature = "test-hooks")]
pub fn hold_call_features(engine: &crate::Engine, hold: std::time::Duration) {
    engine.hold_call_features_now(hold);
}

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
/// Publishes `bytes` as an uploaded decoration without the check an honest sender runs, as a modified client could.
pub async fn publish_unchecked_decoration(engine: &crate::Engine, bytes: Vec<u8>) -> crate::Result<()> {
    let picture = engine.add_picture_blob(&bytes).await?;
    engine.publish_decoration(Some(dissent_events::Decoration::Upload(picture))).await
}

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
        unheard_grace: Duration::from_secs(10),
        tier_return_backoff: Duration::from_secs(3),
        tier_return_backoff_cap: Duration::from_secs(12),
        settle_after_swap: Duration::from_secs(3),
        pending_add_wait: Duration::from_secs(2),
        rejoin_after: Duration::from_secs(3),
        connect_grace: Duration::from_secs(5),
        relink_after: Duration::from_secs(2),
        relink_backoff_cap: Duration::from_secs(8),
        relink_drop_after: Duration::from_secs(6),
        epoch_check_every: Duration::from_secs(3),
        epoch_check_gap: Duration::from_secs(1),
    }
}

/// Makes `engine` decode gossip as a client released before `GossipMsg::EpochCheck` does: it drops an epoch
/// check as undecodable and handles every other message as before.
pub fn act_as_released_gossip(engine: &crate::Engine) {
    engine.inner.legacy_gossip.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The fork `engine` noticed in `community`: the epoch, and the devices heard on the other branch.
pub fn fork_state(
    engine: &crate::Engine,
    community: dissent_identity::CommunityId,
) -> Option<(u64, std::collections::BTreeSet<dissent_identity::DeviceId>)> {
    engine.fork_state(community)
}

/// How many epoch checks `engine` has sent in `community`.
pub fn epoch_checks_sent(engine: &crate::Engine, community: dissent_identity::CommunityId) -> u64 {
    engine.epoch_checks_sent(community)
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

/// Makes `engine`'s mesh calls drop `percent` of the datagrams they send, from the next step of its call on, calls it
/// opens later included. 100 holds its links down: a call it is offered never connects. 0 stops it.
#[cfg(feature = "test-hooks")]
pub fn drop_mesh_media(engine: &crate::Engine, percent: u32) {
    engine.inner.mesh_drop_percent.store(percent.min(100), std::sync::atomic::Ordering::Relaxed);
}

/// Where the event `id` stands on `engine`, with its MLS epoch for `community`, as text for a failing test to print.
#[cfg(feature = "test-hooks")]
pub fn event_debug(engine: &crate::Engine, community: dissent_identity::CommunityId, id: &str) -> String {
    let epoch = engine.mls_epoch(community);
    let Ok(id) = id.parse::<dissent_events::EventId>() else {
        return format!("bad id {id}");
    };
    let w = engine.read();
    let Some(c) = w.communities.get(&community) else {
        return "no such community".into();
    };
    format!("mls epoch {epoch:?}; {}", c.event_debug(&id))
}

/// While `on`, `engine` applies no MLS commit, as when they have not reached it yet. Off, the next commit check or
/// announcement applies them.
#[cfg(feature = "test-hooks")]
pub fn hold_mls_commits(engine: &crate::Engine, on: bool) {
    engine.inner.hold_commits.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Makes `engine`'s mesh call to `peer_root` drop `percent` of the datagrams it sends, from the next membership tick
/// on, a call it makes to them later included. 100 kills that direction of the link. 0 stops it. False without the peer.
#[cfg(feature = "test-hooks")]
pub fn drop_mesh_media_to(engine: &crate::Engine, peer_root: &str, percent: u32) -> bool {
    engine.drop_mesh_media_to_now(peer_root, percent)
}

/// Closes `engine`'s media link to `peer_root`, as a network change does. False without one.
#[cfg(feature = "test-hooks")]
pub fn close_media_link(engine: &crate::Engine, peer_root: &str) -> bool {
    engine.close_media_link_now(peer_root)
}

/// Makes `engine`'s leaf links drop `percent` of the datagrams they send, from the next membership tick on. 0 stops it.
#[cfg(feature = "test-hooks")]
pub fn drop_leaf_media(engine: &crate::Engine, percent: u32) -> bool {
    engine.drop_leaf_media_now(percent)
}

/// Whether `engine`'s leaf link to its forwarder works: connected, and heard lately.
#[cfg(feature = "test-hooks")]
pub async fn leaf_link_live(engine: &crate::Engine) -> bool {
    engine.leaf_link_live_now().await
}

/// The downlinks `engine`'s relay holds, its own leaf included. Zero when it does not forward.
#[cfg(feature = "test-hooks")]
pub async fn forwarder_downlinks(engine: &crate::Engine) -> usize {
    engine.forwarder_downlinks_now().await
}

/// How many relink offers `engine` made for its link to `peer_root` in its current room.
#[cfg(feature = "test-hooks")]
pub fn relink_attempts(engine: &crate::Engine, peer_root: &str) -> u32 {
    engine.relink_attempts_now(peer_root)
}

/// Makes `list_folder` fail after linking `files` files, to test that it undoes them. 0 never fails.
#[cfg(feature = "test-hooks")]
pub fn fail_folder_after(engine: &crate::Engine, files: u64) {
    engine.inner.fail_folder_after.store(files, std::sync::atomic::Ordering::Relaxed);
}

/// How many files `engine` serves in place.
pub fn linked_count(engine: &crate::Engine) -> usize {
    engine.inner.files.linked_count()
}

/// Lists `manifest` as a folder with no files behind it, as a lying lister would.
#[cfg(feature = "test-hooks")]
pub async fn list_raw_folder(
    engine: &crate::Engine,
    community: crate::CommunityId,
    category: dissent_events::CategoryId,
    name: &str,
    manifest: &crate::tracker::folder::Manifest,
) -> crate::Result<String> {
    engine.list_raw_folder(community, category, name, manifest).await
}

/// Makes `engine` answer `Request::Features` with `bits`. Zero models a device from before the probe.
/// Makes `engine` send the call features of a client from before `FEATURE_SEEDERS_ONLY`: it says it elects a member
/// outside the call. Only what it says changes; it still elects as this build does.
pub fn send_old_call_features(engine: &crate::Engine) {
    let old = crate::calls::FEATURE_BOOST_TIERS
        | crate::calls::FEATURE_VIDEO_LAYERS
        | crate::calls::FEATURE_VOLUNTEERS
        | crate::calls::FEATURE_SEEDER_FORWARDERS
        | crate::calls::FEATURE_PAUSED_VIDEO;
    engine.inner.call_features_override.store(old, std::sync::atomic::Ordering::Relaxed);
}

pub fn set_features(engine: &crate::Engine, bits: u64) {
    engine.inner.features.store(bits, std::sync::atomic::Ordering::Relaxed);
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
/// `engine`'s leaf links' counters in a forwarded call, for a test that timed out.
pub async fn leaf_stats(engine: &crate::Engine) -> String {
    engine.leaf_stats_now().await
}

pub fn received_layer(engine: &crate::Engine, sender_root: &str, screen: bool) -> Option<u8> {
    engine.received_layer_now(sender_root, screen)
}

/// On the forwarder `engine`: the layer it plans to send `viewer_root` of `sender_root`'s camera (or
/// screen) now. `None` while that stream is paused for the viewer, or has no layer table.
pub async fn planned_layer(engine: &crate::Engine, viewer_root: &str, sender_root: &str, screen: bool) -> Option<u8> {
    engine.planned_layer_now(viewer_root, sender_root, screen).await
}

/// The bitrate `engine`'s screen share may reach now, in kbps.
pub fn screen_target(engine: &crate::Engine) -> Option<u32> {
    engine.screen_target_now()
}

/// The call loop's timing since the last call: (ticks, ticks over 40 ms late, worst lateness in ms, audio frames received).
pub fn take_loop_stats(engine: &crate::Engine) -> Option<(u64, u64, f64, u64)> {
    engine.take_loop_stats_now()
}

/// The link's last bandwidth estimate in `engine`'s call, in kbps.
pub fn link_estimate(engine: &crate::Engine) -> Option<u32> {
    engine.link_estimate_now()
}

/// While `on`, `engine`'s camera and screen are not stepped down for a busy CPU, so a test of layers or sizes is not at
/// the mercy of the runner's load. False when `engine` is not in a call.
#[cfg(feature = "test-hooks")]
pub fn hold_cpu_governor(engine: &crate::Engine, on: bool) -> bool {
    engine.hold_cpu_governor_now(on)
}

/// Holds `engine`'s estimate of its link at `kbps` when it sizes its video, so a test checks what follows from a rate
/// rather than the machine's load. `None` lets it go. False when `engine` is not in a call.
pub fn force_link_estimate(engine: &crate::Engine, kbps: Option<u32>) -> bool {
    engine.force_link_estimate_now(kbps)
}

/// Starts `engine`'s estimate of each of its mesh links again from `kbps`, as a delay spike leaves it, so a test sees
/// whether the estimate climbs back. False when `engine` is not in a call.
#[cfg(feature = "test-hooks")]
pub async fn dip_link_estimate(engine: &crate::Engine, kbps: u32) -> bool {
    engine.dip_link_estimate_now(kbps).await
}

/// The share of its frame interval `engine`'s screen encoder last reported: above 0.8 it steps down.
#[cfg(feature = "test-hooks")]
pub fn screen_cpu_share(engine: &crate::Engine) -> Option<f64> {
    engine.screen_cpu_share_now()
}

/// What `engine`'s screen share sends now, as (width, height, fps, kbps).
pub fn screen_plan(engine: &crate::Engine) -> Option<(u32, u32, u32, u32)> {
    engine.screen_plan_now()
}

/// What `engine` received of `sender_root`'s camera (or screen): the last frame's layer, how many came, and when the last did.
pub fn video_received_debug(engine: &crate::Engine, sender_root: &str, screen: bool) -> String {
    engine.video_received_debug(sender_root, screen)
}

/// On the forwarder `engine`: what it does with `sender_root`'s camera (or screen) for `viewer_root`.
pub async fn forwarder_video_debug(engine: &crate::Engine, viewer_root: &str, sender_root: &str, screen: bool) -> String {
    engine.forwarder_video_debug(viewer_root, sender_root, screen).await
}

/// On the forwarder `engine`: the root id of the active speaker, whom a tight budget keeps sharp.
pub async fn active_speaker(engine: &crate::Engine) -> Option<String> {
    engine.active_speaker_now().await
}

/// On the forwarder `engine`: the layers `sender_root`'s camera announced, as (width, kbps),
/// smallest first.
pub async fn sender_layers(engine: &crate::Engine, sender_root: &str) -> Vec<(u32, u32)> {
    engine.sender_layers_now(sender_root, false).await
}

/// On the forwarder `engine`: the layers `sender_root`'s screen announced, as (width, kbps), smallest first.
pub async fn sender_screen_layers(engine: &crate::Engine, sender_root: &str) -> Vec<(u32, u32)> {
    engine.sender_layers_now(sender_root, true).await
}

/// On the forwarder `engine`: holds `viewer_root`'s downlink to `kbps` of video, as a measured
/// estimate would. Returns false when `engine` does not forward its call.
pub async fn set_viewer_budget(engine: &crate::Engine, viewer_root: &str, kbps: u32) -> bool {
    engine.set_viewer_budget_now(viewer_root, kbps).await
}

/// On the forwarder `engine`: starts the estimate of `viewer_root`'s downlink again from `kbps`, as a delay spike
/// leaves it. Returns false when `engine` does not forward its call.
#[cfg(feature = "test-hooks")]
pub async fn dip_viewer_estimate(engine: &crate::Engine, viewer_root: &str, kbps: u32) -> bool {
    engine.with_viewer_downlink(viewer_root, |sfu, viewer| sfu.reset_downlink_estimate(viewer, kbps)).await
}

/// On the forwarder `engine`: the estimate of `viewer_root`'s downlink, and the video cap it sets (`None` for none),
/// in kbps, as text for a test's log.
#[cfg(feature = "test-hooks")]
pub async fn viewer_estimate(engine: &crate::Engine, viewer_root: &str) -> String {
    let mut out = String::from("none");
    engine
        .with_viewer_downlink(viewer_root, |sfu, viewer| {
            out = format!("{:?}", sfu.downlink_estimate(viewer));
            true
        })
        .await;
    out
}

/// On the forwarder `engine`: drops `percent` of the datagrams to `viewer_root`, as a lossy link does. 0 stops it.
/// Returns false when `engine` does not forward its call.
#[cfg(feature = "test-hooks")]
pub async fn drop_to_viewer(engine: &crate::Engine, viewer_root: &str, percent: u32) -> bool {
    engine.with_viewer_downlink(viewer_root, |sfu, viewer| sfu.set_downlink_drop(viewer, percent)).await
}

/// Whether downlink bandwidth estimates cap budgets on `engine`'s relays: its call's, and those of calls it forwards as
/// a volunteer. Off, a set budget is the only limit. Returns false when `engine` forwards no call.
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

#[cfg(feature = "test-hooks")]
/// Lists `bytes` in `category` as a lying lister would: the listing claims `claimed` as its fingerprint, and its blob
/// is `bytes` encrypted under the key that fingerprint gives. Returns the listing id. See `tracker::catalog`.
pub async fn list_with_false_fingerprint(
    engine: &crate::Engine,
    community: dissent_identity::CommunityId,
    category: dissent_events::CategoryId,
    name: &str,
    bytes: Vec<u8>,
    claimed: [u8; 32],
) -> crate::Result<String> {
    engine.list_with_false_fingerprint(community, category, name, bytes, claimed).await
}

/// Sends `pairs` to `device` as `PushFilesKeys`, as any member could. Returns whether it answered `Ack`.
pub async fn push_files_keys(
    engine: &crate::Engine,
    device: dissent_identity::DeviceId,
    community: dissent_identity::CommunityId,
    pairs: Vec<(u64, [u8; 32])>,
) -> bool {
    matches!(
        engine.call(device, &crate::rpc::Request::PushFilesKeys { community, pairs }).await,
        Ok(crate::rpc::Response::Ack)
    )
}

/// The newest files-key generation `engine` would seal under, if it holds one.
pub fn newest_files_generation(engine: &crate::Engine, community: dissent_identity::CommunityId) -> Option<u64> {
    engine.with_tracker(community, |l| l.newest().map(|(g, _)| g))
}

/// How many IP sockets `engine`'s endpoint has bound. Zero for a device that hides its IP.
pub fn bound_socket_count(engine: &crate::Engine) -> usize {
    engine.node().bound_socket_count()
}

/// Whether `engine` looks devices up through number0's DNS, which asks the system resolver outside any proxy.
pub fn uses_dns_lookup(engine: &crate::Engine) -> bool {
    engine.node().uses_dns_lookup()
}

/// The direct IP addresses `engine` holds for `device`, from the signed addresses it has learned. A
/// device that hides its IP should have none.
pub fn direct_addrs_known(engine: &crate::Engine, device: dissent_identity::DeviceId) -> usize {
    engine
        .read()
        .addresses
        .all()
        .into_iter()
        .filter(|stored| stored.addr.id == device)
        .map(|stored| stored.addr.direct.len())
        .sum()
}

/// An engine held without keeping it alive, for a watchdog: a stopped engine is gone, and its sockets with it.
pub struct WeakEngine(std::sync::Weak<crate::engine::Inner>);

impl WeakEngine {
    pub fn of(engine: &crate::Engine) -> Self {
        WeakEngine(std::sync::Arc::downgrade(&engine.inner))
    }

    /// The engine, while anything else still holds it.
    pub fn upgrade(&self) -> Option<crate::Engine> {
        self.0.upgrade().map(|inner| crate::Engine { inner })
    }
}

/// The call loop's step, its age and the mesh lock's state, without blocking. For a watchdog.
pub fn call_breadcrumbs(engine: &crate::Engine) -> String {
    engine.call_breadcrumbs()
}

/// Where `engine`'s call loop stands: ticks, speaking checks and each link's frame counters.
pub fn call_debug(engine: &crate::Engine) -> String {
    engine.call_debug()
}

/// A hash of the media key and the sender id of `engine`'s current channel call, and the epoch that key belongs to.
pub fn call_key_state(engine: &crate::Engine) -> Option<([u8; 4], u64, u64)> {
    engine.call_key_state()
}

/// Whether `engine` is still telling its call's peers its mute, and the rounds of mute RPCs it has sent them so far.
pub fn mute_telling(engine: &crate::Engine) -> Option<(bool, u64)> {
    engine.mute_telling()
}

/// Makes `engine`'s blob server refuse every request for `time`, as a sender that is not serving yet.
pub fn refuse_blobs_for(engine: &crate::Engine, time: std::time::Duration) {
    engine.write().blob_scopes.refuse_until = Some(std::time::Instant::now() + time);
}

/// How many requests `engine`'s blob server refused under `refuse_blobs_for`.
pub fn refused_blobs(engine: &crate::Engine) -> u64 {
    engine.read().blob_scopes.refused_by_test
}

/// The number of blobs and the encoded size of this device's holdings answer for `community`, as a member asking with no
/// digest would get it.
pub fn holdings_answer(engine: &crate::Engine, community: dissent_identity::CommunityId) -> (usize, usize) {
    let holdings = engine.my_holdings(community);
    let answer: Vec<([u8; 32], u64)> = holdings.iter().map(|h| (h.blob, h.bytes)).collect();
    (answer.len(), postcard::to_allocvec(&answer).map(|b| b.len()).unwrap_or(0))
}

/// Makes `engine` act as a release before upload slots: it does not say it takes a busy answer, and its downloads count one
/// as a failure.
pub fn act_as_before_upload_slots(engine: &crate::Engine) {
    set_features(engine, crate::custom_roles::FEATURES & !crate::custom_roles::FEATURE_SEED_SLOTS);
    engine.inner.files.act_as_before_upload_slots();
}

/// How many requests of `device` this seeder held for a free upload slot.
pub fn slot_holds(engine: &crate::Engine, device: dissent_identity::DeviceId) -> u64 {
    engine.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).slot_holds.get(&device).copied().unwrap_or(0)
}

/// How many requests of `device` this seeder refused for full upload slots.
pub fn slot_refusals(engine: &crate::Engine, device: dissent_identity::DeviceId) -> u64 {
    engine.inner.tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).slot_refusals.get(&device).copied().unwrap_or(0)
}

/// The protocols every incoming connection can arrive on, as the connection gate sees them: requests, gossip, file
/// blobs and both call media links.
pub const GATED_ALPNS: [&[u8]; 5] = [b"dissent/rpc/0", b"/iroh-gossip/1", dissent_files::ALPN, b"dissent/media/0", b"dissent/media/1"];

/// What `engine`'s connection gate makes of `device` now, after rebuilding its snapshot: `known`, `stranger` or
/// `refused`.
pub fn connection_verdict(engine: &crate::Engine, device: dissent_identity::DeviceId) -> &'static str {
    use dissent_net::ConnectionGate as _;
    crate::conn_gate::rebuild(&engine.inner);
    match engine.inner.conn_gate.verdict(device) {
        dissent_net::Gate::Known => "known",
        dissent_net::Gate::Stranger => "stranger",
        dissent_net::Gate::Refuse => "refused",
    }
}

/// Dials `peer` from `engine`'s node on `alpn` and waits up to `wait` for the peer to close it. Returns the close code
/// the peer gave, or `None` when the connection stayed open.
pub async fn dial_and_watch(
    engine: &crate::Engine,
    peer: dissent_identity::DeviceId,
    alpn: &[u8],
    wait: std::time::Duration,
) -> Result<Option<u64>, String> {
    let id = iroh::EndpointId::from_bytes(&peer.0).map_err(|e| e.to_string())?;
    let conn = engine.node().endpoint().connect(id, alpn).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(wait, conn.closed()).await {
        Ok(iroh::endpoint::ConnectionError::ApplicationClosed(close)) => Ok(Some(close.error_code.into_inner())),
        Ok(other) => Err(other.to_string()),
        Err(_) => Ok(None),
    }
}

/// The address `engine`'s node can be dialled at now.
pub async fn node_addr(engine: &crate::Engine) -> dissent_net::PeerAddr {
    engine.node().addr().await
}
