//! Helpers shared by the networked tests. Every engine runs on this machine with relays disabled.
#![allow(dead_code)]

use dissent_core::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice, RootId,
};
use std::net::UdpSocket;
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

/// A UDP port that is free right now.
pub fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

pub async fn start_on(identity: Identity, dir: PathBuf, port: u16) -> Peer {
    let (engine, events) = Engine::start(identity.clone(), dir.clone(), offline_on(port))
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

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Polls `check` every 50 ms for up to 10 s. Panics with `what` on timeout.
pub async fn eventually<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    eventually_within(Duration::from_secs(10), what, check).await
}

/// Polls `check` every 50 ms for up to `limit`. Panics with `what` on timeout.
pub async fn eventually_within<T>(
    limit: Duration,
    what: &str,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn has_message(engine: &Engine, community: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(community, channel, 100_000)
        .iter()
        .any(|m| m.text == text)
}

/// How many messages in the channel start with `prefix`.
pub fn count_prefixed(engine: &Engine, community: CommunityId, channel: ChannelId, prefix: &str) -> usize {
    engine
        .channel_messages(community, channel, 100_000)
        .iter()
        .filter(|m| m.text.starts_with(prefix))
        .count()
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

/// Alice creates "Rustaceans" and Bob joins it. Returns the community id.
pub async fn rustaceans(alice: &Peer, bob: &Peer) -> CommunityId {
    let server = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create community");
    let id: CommunityId = server.id.parse().unwrap();
    let invite = alice
        .engine
        .create_invite(id)
        .await
        .expect("create invite");
    let joined = bob
        .engine
        .join_community(&invite)
        .await
        .expect("join community");
    assert_eq!(joined.id, server.id);
    assert_eq!(joined.my_role, "member");
    id
}

/// Shuts every engine down and deletes its data directory.
pub async fn finish(peers: Vec<Peer>) {
    for p in peers {
        p.engine.shutdown().await.expect("clean shutdown");
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}
