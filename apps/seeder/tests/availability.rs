//! Phase-3 exit criterion: history stays available with every member device offline as long as
//! one seeder is up. Every engine runs on this machine with relays disabled.

use dissent_core::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice,
};
use dissent_discovery::Invite;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const MESSAGES: usize = 20;

struct Node {
    engine: Engine,
    dir: PathBuf,
    /// Kept so the notification channel stays open while the test runs.
    _events: broadcast::Receiver<EngineEvent>,
}

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Tests run in parallel. A counter keeps two calls from sharing one directory.
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "dissent-seeder-it-{name}-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn offline() -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        dht_lookup: false,
        ..NodeConfig::default()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn node(name: &str) -> Node {
    let dir = scratch(name);
    let (engine, events) = Engine::start(Identity::create(now()), dir.clone(), offline())
        .await
        .expect("engine starts");
    engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    Node {
        engine,
        dir,
        _events: events,
    }
}

/// Polls `check` every 50 ms for up to 15 s. Panics with `what` on timeout.
async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn general_channel(engine: &Engine, id: CommunityId) -> Option<ChannelId> {
    engine
        .community_view(id)
        .ok()?
        .channels
        .iter()
        .find(|c| c.name == "general")
        .and_then(|c| c.id.parse().ok())
}

/// The texts of every message in `#general`, oldest first.
fn general_texts(engine: &Engine, id: CommunityId) -> Option<Vec<String>> {
    let channel = general_channel(engine, id)?;
    Some(
        engine
            .channel_messages(id, channel, usize::MAX)
            .into_iter()
            .map(|m| m.text)
            .collect(),
    )
}

fn expected_texts() -> Vec<String> {
    (0..MESSAGES).map(|i| format!("history {i:02}")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn history_survives_every_member_offline() {
    // 1. Alice creates a community and posts history.
    let alice = node("alice").await;
    let server = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create community");
    let id: CommunityId = server.id.parse().unwrap();
    let general = general_channel(&alice.engine, id).expect("general channel");
    for text in expected_texts() {
        alice
            .engine
            .send_message(id, general, text)
            .await
            .expect("alice posts");
    }
    let alice_invite = alice
        .engine
        .create_invite(id)
        .await
        .expect("alice invite");

    // 2. A seeder joins from Alice's invite and syncs the history.
    let seeder = node("seeder").await;
    seeder
        .engine
        .join_community(&alice_invite)
        .await
        .expect("seeder joins");
    let wanted = expected_texts();
    eventually("seeder holds all history", || {
        let texts = general_texts(&seeder.engine, id)?;
        (texts == wanted).then_some(())
    })
    .await;

    // 3. Alice goes offline. No member device is left.
    let alice_dir = alice.dir.clone();
    alice.engine.shutdown().await.expect("alice shuts down");
    let _ = std::fs::remove_dir_all(&alice_dir);

    // 4. Bob joins from an invite that names only the seeder's address.
    let genesis = Invite::parse(&alice_invite)
        .expect("alice invite parses")
        .genesis;
    let seeder_peers = Invite::parse(
        &seeder
            .engine
            .create_invite(id)
            .await
            .expect("seeder invite"),
    )
    .expect("seeder invite parses")
    .peers;
    assert_eq!(seeder_peers.len(), 1, "seeder advertises one address");
    let bob_invite = Invite {
        genesis,
        peers: seeder_peers,
        secret: None,
    }
    .to_url()
    .to_string();

    let bob = node("bob").await;
    let joined = bob
        .engine
        .join_community(&bob_invite)
        .await
        .expect("bob joins from the seeder alone");
    assert_eq!(joined.id, server.id);

    // 5. Bob ends up with every message, in order.
    eventually("bob receives all history", || {
        let texts = general_texts(&bob.engine, id)?;
        (texts == wanted).then_some(())
    })
    .await;

    for n in [bob, seeder] {
        let dir = n.dir.clone();
        n.engine.shutdown().await.expect("clean shutdown");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
