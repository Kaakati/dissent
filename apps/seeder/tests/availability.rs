//! Phase-3 exit criterion: history stays available with every member device offline as long as
//! one seeder is up. Every engine runs on this machine with relays disabled.
//!
//! The seeder joins as a storage member. It keeps and serves sealed history, but it is never added
//! to the community's MLS group, so it cannot read messages. A joiner therefore receives every
//! message event from the seeder as locked, and reads new messages only once a person who is in
//! the group admits them.
//!
//! Every wait has a bound, so a stalled engine fails the test instead of hanging it.

use dissent_core::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, MessageItem, NodeConfig, RelayChoice,
};
use dissent_discovery::Invite;
use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const MESSAGES: usize = 20;
/// Longest any single engine call may take.
const STEP: Duration = Duration::from_secs(30);
/// Longest any whole test may take.
const TEST_LIMIT: Duration = Duration::from_secs(240);

struct Node {
    engine: Engine,
    dir: PathBuf,
    identity: Identity,
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

/// Awaits one engine call, failing with `what` if it takes longer than `STEP`.
async fn step<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(STEP, fut)
        .await
        .unwrap_or_else(|_| panic!("{what} did not finish within {STEP:?}"))
}

async fn start_engine(identity: Identity, dir: PathBuf) -> (Engine, broadcast::Receiver<EngineEvent>) {
    step(
        "engine start",
        Engine::start(identity, dir.clone(), offline(), dissent_core::test_support::vault_for(&dir)),
    )
    .await
    .expect("engine starts")
}

async fn node(name: &str) -> Node {
    let dir = scratch(name);
    let identity = Identity::create(now());
    let (engine, events) = start_engine(identity.clone(), dir.clone()).await;
    step("set name", engine.set_display_name(name.to_string()))
        .await
        .expect("name set");
    Node {
        engine,
        dir,
        identity,
        _events: events,
    }
}

/// Polls `check` every 50 ms for up to 15 s. Panics with `what` on timeout.
async fn eventually<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    eventually_within(Duration::from_secs(15), what, check).await
}

/// Polls `check` every 50 ms for up to `limit`. Panics with `what` on timeout.
async fn eventually_within<T>(
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

fn general_channel(engine: &Engine, id: CommunityId) -> Option<ChannelId> {
    engine
        .community_view(id)
        .ok()?
        .channels
        .iter()
        .find(|c| c.name == "general")
        .and_then(|c| c.id.parse().ok())
}

/// Every message item in `#general`, oldest first, locked or not.
fn general_items(engine: &Engine, id: CommunityId) -> Option<Vec<MessageItem>> {
    let channel = general_channel(engine, id)?;
    Some(engine.channel_messages(id, channel, usize::MAX))
}

fn ids_of(items: &[MessageItem]) -> BTreeSet<String> {
    items.iter().map(|m| m.id.clone()).collect()
}

/// Alice creates the community and posts `MESSAGES` messages. Returns the community, the
/// general channel, the message ids, and the invite.
async fn alice_posts_history(alice: &Node) -> (CommunityId, ChannelId, BTreeSet<String>, String) {
    let server = step(
        "create community",
        alice.engine.create_community("Rustaceans".into()),
    )
    .await
    .expect("create community");
    let id: CommunityId = server.id.parse().unwrap();
    let general = general_channel(&alice.engine, id).expect("general channel");
    let mut ids = BTreeSet::new();
    for i in 0..MESSAGES {
        let item = step(
            "alice posts",
            alice
                .engine
                .send_message(id, general, format!("history {i:02}")),
        )
        .await
        .expect("alice posts");
        ids.insert(item.id);
    }
    let invite = step("alice invite", alice.engine.create_invite(id))
        .await
        .expect("alice invite");
    (id, general, ids, invite)
}

/// An invite for the same community that names only `peers`. Every other field, including the
/// secret and relays, comes from `alice_invite`.
fn invite_via(alice_invite: &str, peers: Vec<String>) -> String {
    let mut invite = Invite::parse(alice_invite).expect("alice invite parses");
    invite.peers = peers;
    invite.to_url().to_string()
}

/// The address of `node` as an invite gives it, for a joiner that knows only that address.
async fn address_of(node: &Node, id: CommunityId) -> Vec<String> {
    let link = step("seeder invite", node.engine.create_invite(id))
        .await
        .expect("invite for address");
    Invite::parse(&link).expect("parses").peers
}

/// Asserts that a storage seeder holds no MLS state for the community. It never joins the group,
/// so it has no commit log for it.
fn assert_no_mls_state(dir: &Path, id: CommunityId) {
    let commits = dir
        .join("communities")
        .join(id.to_string())
        .join("mls_commits.log");
    assert!(
        !commits.exists(),
        "storage seeder has an MLS commit log at {}",
        commits.display()
    );
}

async fn shut_down(node: Node) {
    let dir = node.dir.clone();
    step("shutdown", node.engine.shutdown())
        .await
        .expect("clean shutdown");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shuts an engine down but keeps its data, so it can restart from the same directory.
async fn stop_keeping_data(node: Node) -> (Identity, PathBuf) {
    let Node {
        engine,
        dir,
        identity,
        _events,
    } = node;
    step("shutdown", engine.shutdown())
        .await
        .expect("clean shutdown");
    (identity, dir)
}

/// Runs a whole test body under `TEST_LIMIT`.
async fn bounded(body: impl Future<Output = ()>) {
    tokio::time::timeout(TEST_LIMIT, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {TEST_LIMIT:?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn history_survives_every_member_offline() {
    bounded(async {
        // 1. Alice creates a community and posts history.
        let alice = node("alice").await;
        let (id, _general, history, alice_invite) = alice_posts_history(&alice).await;

        // 2. A storage seeder joins from Alice's invite. It syncs every message event and holds
        //    them sealed. It is never added to the MLS group.
        let seeder = node("seeder").await;
        step(
            "seeder joins as storage",
            seeder.engine.join_as_storage(&alice_invite),
        )
        .await
        .expect("seeder joins as storage");
        eventually("seeder holds every message event", || {
            let items = general_items(&seeder.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;
        assert_no_mls_state(&seeder.dir, id);

        // 3. Alice goes offline. No member device is left.
        shut_down(alice).await;

        // 4. Bob joins from an invite that names only the seeder's address.
        let bob_invite = invite_via(&alice_invite, address_of(&seeder, id).await);
        let bob = node("bob").await;
        let joined = step("bob joins", bob.engine.join_community(&bob_invite))
            .await
            .expect("bob joins from the seeder alone");
        assert_eq!(joined.id, id.to_string());

        // 5. Bob receives every message event from the seeder. He cannot decrypt them, because
        //    they predate his place in the group, so every one is locked.
        eventually("bob receives every message event", || {
            let items = general_items(&bob.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;
        let bob_items = general_items(&bob.engine, id).expect("general");
        assert!(
            bob_items.iter().all(|m| m.locked),
            "history from before Bob joined must stay locked for him"
        );

        shut_down(bob).await;
        shut_down(seeder).await;
    })
    .await;
}

/// The seeder cannot admit Bob. A person in the group does that. Once Alice is back online, she
/// adds Bob, and Bob then reads messages posted after that.
#[tokio::test(flavor = "multi_thread")]
async fn bob_reads_new_messages_once_alice_returns() {
    bounded(async {
        let alice = node("alice").await;
        let (id, _general, history, alice_invite) = alice_posts_history(&alice).await;

        let seeder = node("seeder").await;
        step("seeder joins as storage", seeder.engine.join_as_storage(&alice_invite))
            .await
            .expect("seeder joins as storage");
        eventually("seeder holds every message event", || {
            let items = general_items(&seeder.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        // Alice goes offline, keeping her data so she can come back.
        let (alice_identity, alice_dir) = stop_keeping_data(alice).await;

        let bob_invite = invite_via(&alice_invite, address_of(&seeder, id).await);
        let bob = node("bob").await;
        step("bob joins", bob.engine.join_community(&bob_invite))
            .await
            .expect("bob joins from the seeder alone");
        eventually("bob receives every message event", || {
            let items = general_items(&bob.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        // Alice comes back online from her own data.
        let (engine, events) = start_engine(alice_identity.clone(), alice_dir.clone()).await;
        let alice = Node {
            engine,
            dir: alice_dir,
            identity: alice_identity,
            _events: events,
        };

        // Once Alice has admitted Bob to the group, a new message reaches Bob as plaintext. Alice
        // retries her post until Bob reads it, which covers the time her maintenance takes.
        let general = general_channel(&alice.engine, id).expect("alice general");
        let deadline = Instant::now() + Duration::from_secs(150);
        let mut n = 0;
        loop {
            let text = format!("after alice returned {n}");
            step("alice posts", alice.engine.send_message(id, general, text.clone()))
                .await
                .expect("alice posts");
            n += 1;
            tokio::time::sleep(Duration::from_secs(5)).await;
            let read = general_items(&bob.engine, id)
                .unwrap_or_default()
                .iter()
                .any(|m| m.text.starts_with("after alice returned") && !m.locked);
            if read {
                break;
            }
            assert!(Instant::now() < deadline, "bob never read a message from Alice after she returned");
        }

        // The history stays locked for Bob, and only new messages become readable.
        let bob_items = general_items(&bob.engine, id).expect("general");
        let old: Vec<&MessageItem> = bob_items.iter().filter(|m| history.contains(&m.id)).collect();
        assert_eq!(old.len(), MESSAGES);
        assert!(old.iter().all(|m| m.locked));

        shut_down(alice).await;
        shut_down(bob).await;
        shut_down(seeder).await;
    })
    .await;
}
