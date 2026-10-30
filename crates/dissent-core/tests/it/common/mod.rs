//! Helpers shared by the networked tests. Every engine runs on this machine with relays disabled.
#![allow(dead_code)]

pub mod bounded;
#[allow(unused_imports)]
pub use bounded::*;

use dissent_core::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice, RootId,
};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

pub struct Peer {
    pub engine: Engine,
    pub identity: Identity,
    pub dir: PathBuf,
    /// Kept so the notification channel stays open while the test runs.
    pub _events: broadcast::Receiver<EngineEvent>,
}

pub fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Tests run in parallel. A counter keeps two calls from sharing one directory.
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "dissent-core-it-{name}-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A UDP port the operating system reports as free. Ports from a per-process counter collide
/// between test binaries that run at the same time, so the OS picks one instead. The window
/// between releasing it and the engine binding it is small.
pub fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// Relays off, bound to `port`. `0` picks any free port.
pub fn offline_on(port: u16) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        bind_port: port,
        ..NodeConfig::default()
    }
}

/// Starts an engine with the short test intervals, so presence, call and typing waits are seconds.
pub async fn start_on(identity: Identity, dir: PathBuf, port: u16) -> Peer {
    let (engine, events) = Engine::start_with_timings(
        identity.clone(),
        dir.clone(),
        offline_on(port),
        dissent_core::test_support::vault_for(&dir),
        dissent_core::test_support::fast_timings(),
    )
    .await
    .expect("engine starts");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

pub async fn start(identity: Identity, dir: PathBuf) -> Peer {
    start_on(identity, dir, 0).await
}

/// Shuts the engine down and returns what a restart needs. The data directory is kept.
pub async fn stop(p: Peer) -> (Identity, PathBuf) {
    let Peer {
        engine,
        identity,
        dir,
        _events,
    } = p;
    engine.shutdown().await.expect("clean shutdown");
    (identity, dir)
}

pub async fn peer(name: &str) -> Peer {
    let p = start(Identity::create(now()), scratch(name)).await;
    p.engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    p
}

/// A peer whose presence beacons come only every 60 s. A status change that a peer sees within a
/// few seconds can then only have come from the change itself, not from a periodic beacon.
pub async fn peer_slow_beacons(name: &str) -> Peer {
    let dir = scratch(name);
    let identity = Identity::create(now());
    let timings = dissent_core::Timings {
        beacon_every: Duration::from_secs(60),
        presence_window: Duration::from_secs(120),
        ..dissent_core::test_support::fast_timings()
    };
    let (engine, events) = Engine::start_with_timings(
        identity.clone(),
        dir.clone(),
        offline_on(0),
        dissent_core::test_support::vault_for(&dir),
        timings,
    )
    .await
    .expect("engine starts");
    engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Polls `check` every 50 ms for up to 60 s (scaled, see `bounded::scaled`). Panics with `what` on timeout.
pub async fn eventually<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    eventually_within(Duration::from_secs(60), what, check).await
}

/// Polls `check` every 50 ms for up to `limit` (scaled). Panics with `what` on timeout.
pub async fn eventually_within<T>(
    limit: Duration,
    what: &str,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + scaled(limit);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Like `eventually`, but wakes on every event `engine` emits, not only on the 50 ms poll. Use it
/// when the engine that changes is the one being watched. The subscription is taken before the
/// first check, so an event that lands in between is not lost.
pub async fn until_event<T>(
    engine: &Engine,
    limit: Duration,
    what: &str,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let mut events = engine.subscribe();
    let deadline = tokio::time::Instant::now() + scaled(limit);
    let mut open = true;
    loop {
        if let Some(value) = check() {
            return value;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "timed out waiting for {what}");
        tokio::select! {
            event = events.recv(), if open => {
                // A lagged receiver still has events to report, so only a closed channel stops us
                // listening. Polling continues after that.
                if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                    open = false;
                }
            }
            _ = tokio::time::sleep(left.min(Duration::from_millis(50))) => {}
        }
    }
}

/// Makes `a` and `b` contacts, each adding the other, and returns once each side lists the other.
/// Adding is asynchronous on the far side, so a call or a message sent before this can be refused.
pub async fn make_contacts_ready(a: &Peer, b: &Peer) {
    let (a_root, b_root) = (a.identity.root_id(), b.identity.root_id());
    a.engine
        .add_contact(&b.engine.contact_link())
        .await
        .expect("a adds b");
    b.engine
        .add_contact(&a.engine.contact_link())
        .await
        .expect("b adds a");
    until_event(&a.engine, Duration::from_secs(60), "a lists b as a contact", || {
        lists_contact(&a.engine, b_root).then_some(())
    })
    .await;
    until_event(&b.engine, Duration::from_secs(60), "b lists a as a contact", || {
        lists_contact(&b.engine, a_root).then_some(())
    })
    .await;
}

fn lists_contact(engine: &Engine, user: RootId) -> bool {
    let user = user.to_string();
    engine
        .list_contacts()
        .contacts
        .iter()
        .any(|c| c.root_id == user)
}

/// Waits until `peer` holds `text` in the channel. A send is visible on another engine only after
/// it has gossiped there, so a test asserts on the far side through this.
pub async fn wait_seen(peer: &Peer, community: CommunityId, channel: ChannelId, text: &str) {
    until_event(
        &peer.engine,
        Duration::from_secs(60),
        &format!("{text:?} to reach {}", peer.identity.root_id()),
        || has_message(&peer.engine, community, channel, text).then_some(()),
    )
    .await;
}

/// Joins `joiner` with `invite` and returns once `host` lists the joiner as a member. Until then
/// the host may not admit the joiner's messages, calls or key material.
pub async fn join_ready(host: &Peer, joiner: &Peer, invite: &str) -> CommunityId {
    let joined = joiner
        .engine
        .join_community(invite)
        .await
        .expect("join community");
    let id: CommunityId = joined.id.parse().expect("community id");
    assert_eq!(joined.my_role, "member");
    let joiner_root = joiner.identity.root_id().to_string();
    until_event(&host.engine, Duration::from_secs(60), "the host lists the joiner", || {
        host.engine
            .community_view(id)
            .ok()
            .filter(|v| v.members.iter().any(|m| m.root_id == joiner_root))
            .map(|_| ())
    })
    .await;
    id
}

pub fn has_message(
    engine: &Engine,
    community: CommunityId,
    channel: ChannelId,
    text: &str,
) -> bool {
    engine
        .channel_messages(community, channel, 100_000)
        .iter()
        .any(|m| m.text == text)
}

/// How many messages in the channel start with `prefix`.
pub fn count_prefixed(
    engine: &Engine,
    community: CommunityId,
    channel: ChannelId,
    prefix: &str,
) -> usize {
    engine
        .channel_messages(community, channel, 100_000)
        .iter()
        .filter(|m| m.text.starts_with(prefix))
        .count()
}

/// The bytes of a picture the engine keeps, as the window reads them. Pictures are sealed at rest, so the file itself
/// is never the picture's bytes.
pub fn picture(engine: &Engine, path: impl AsRef<std::path::Path>) -> Vec<u8> {
    let path = path.as_ref();
    let (bytes, _) = engine.open_picture(path).expect("the picture opens");
    assert_ne!(std::fs::read(path).expect("the picture file exists"), bytes, "the picture is sealed at rest");
    bytes
}

pub fn dm_has(engine: &Engine, peer: RootId, text: &str) -> bool {
    engine
        .dm_messages(peer, 100_000)
        .iter()
        .any(|m| m.text == text)
}

pub fn channel_id(engine: &Engine, community: CommunityId, name: &str) -> Option<ChannelId> {
    engine
        .community_view(community)
        .ok()?
        .channels
        .iter()
        .find(|c| c.name == name)
        .and_then(|c| c.id.parse().ok())
}

/// Alice creates "Rustaceans" and Bob joins it. Returns the community id, once Alice lists Bob.
pub async fn rustaceans(alice: &Peer, bob: &Peer) -> CommunityId {
    let server = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create community");
    let id: CommunityId = server.id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.expect("create invite");
    let joined = join_ready(alice, bob, &invite).await;
    assert_eq!(joined, id);
    id
}

/// Shuts every engine down and deletes its data directory.
pub async fn finish(peers: Vec<Peer>) {
    for p in peers {
        p.engine.shutdown().await.expect("clean shutdown");
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}

/// Waits for the next incoming direct call on `rx`. Returns its call id.
pub async fn next_incoming(rx: &mut broadcast::Receiver<EngineEvent>) -> String {
    tokio::time::timeout(scaled(Duration::from_secs(10)), async {
        loop {
            match rx.recv().await {
                Ok(EngineEvent::IncomingCall { call_id, .. }) => return call_id,
                Ok(_) => {}
                Err(e) => panic!("engine events closed: {e}"),
            }
        }
    })
    .await
    .expect("an incoming call arrives")
}

/// Waits for the next incoming direct call on `rx`, then accepts it. Returns its call id.
pub async fn accept_incoming(engine: &Engine, rx: &mut broadcast::Receiver<EngineEvent>) -> String {
    let id = next_incoming(rx).await;
    engine.accept_call(id.clone()).await.expect("call accepted");
    id
}
