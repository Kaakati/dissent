//! Phase-3 exit criterion: history stays available with every member device offline as long as
//! one seeder is up. Every engine runs on this machine with relays disabled.
//!
//! Messages are encrypted with the community's MLS group. A device only reads messages from the
//! epochs it has keys for. So a device that joins later sees earlier messages as locked. The
//! guarantee tested here is that every message event reaches the joiner, locked or readable. Text
//! is only checked where MLS gives a key.
//!
//! Every wait has a bound, so a stalled engine fails the test instead of hanging it.

use dissent_core::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, MessageItem, NodeConfig,
    RelayChoice,
};
use dissent_discovery::Invite;
use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
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

async fn node(name: &str) -> Node {
    let dir = scratch(name);
    let (engine, events) = step(
        "engine start",
        Engine::start(Identity::create(now()), dir.clone(), offline()),
    )
    .await
    .expect("engine starts");
    step(
        "set name",
        engine.set_display_name(name.to_string()),
    )
    .await
    .expect("name set");
    Node {
        engine,
        dir,
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

/// A link that names `peers` only, for the community whose genesis is in `invite`.
fn invite_via(alice_invite: &str, peers: Vec<String>) -> String {
    let original = Invite::parse(alice_invite).expect("alice invite parses");
    Invite {
        genesis: original.genesis,
        peers,
        secret: original.secret,
    }
    .to_url()
    .to_string()
}

/// The address of `node` as an invite gives it, for a joiner that knows only that address.
async fn address_of(node: &Node, id: CommunityId) -> Vec<String> {
    let link = step("seeder invite", node.engine.create_invite(id))
        .await
        .expect("invite for address");
    Invite::parse(&link).expect("parses").peers
}

async fn shut_down(node: Node) {
    let dir = node.dir.clone();
    step("shutdown", node.engine.shutdown())
        .await
        .expect("clean shutdown");
    let _ = std::fs::remove_dir_all(&dir);
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

        // 2. A seeder joins from Alice's invite and syncs every message event. It cannot decrypt
        //    the messages, because they predate its MLS epoch, so they show as locked.
        let seeder = node("seeder").await;
        step(
            "seeder joins",
            seeder.engine.join_community(&alice_invite),
        )
        .await
        .expect("seeder joins");
        eventually("seeder holds every message event", || {
            let items = general_items(&seeder.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        // 3. Alice goes offline. No member device is left.
        shut_down(alice).await;

        // 4. Bob joins from an invite that names only the seeder's address.
        let bob_invite = invite_via(&alice_invite, address_of(&seeder, id).await);
        let bob = node("bob").await;
        let joined = step("bob joins", bob.engine.join_community(&bob_invite))
            .await
            .expect("bob joins from the seeder alone");
        assert_eq!(joined.id, id.to_string());

        // 5. Bob receives every message event, locked or readable.
        eventually("bob receives every message event", || {
            let items = general_items(&bob.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        shut_down(bob).await;
        shut_down(seeder).await;
    })
    .await;
}

/// Stronger than the availability promise. The seeder is a plain member, and Alice is offline.
/// Core lets any current member commit adds of current members, so the seeder admits Bob to the
/// MLS group. A message posted after Bob joins must then reach him as plaintext.
#[tokio::test(flavor = "multi_thread")]
async fn seeder_member_admits_later_joiner_and_post_readable_messages() {
    bounded(async {
        let alice = node("alice").await;
        let (id, _general, history, alice_invite) = alice_posts_history(&alice).await;

        let seeder = node("seeder").await;
        step("seeder joins", seeder.engine.join_community(&alice_invite))
            .await
            .expect("seeder joins");
        eventually("seeder holds every message event", || {
            let items = general_items(&seeder.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        // Alice goes offline. The seeder is a plain member and must admit Bob on its own.
        shut_down(alice).await;

        // Bob joins through the seeder alone, and must still receive every history event.
        let bob_invite = invite_via(&alice_invite, address_of(&seeder, id).await);
        let bob = node("bob").await;
        step("bob joins", bob.engine.join_community(&bob_invite))
            .await
            .expect("bob joins");
        eventually("bob receives every message event", || {
            let items = general_items(&bob.engine, id)?;
            (ids_of(&items) == history).then_some(())
        })
        .await;

        // A message posted after Bob joined is readable by him, with Alice still offline.
        let general = general_channel(&seeder.engine, id).expect("general");
        step(
            "seeder posts",
            seeder
                .engine
                .send_message(id, general, "after bob joined".into()),
        )
        .await
        .expect("seeder posts");
        eventually("bob reads the post-join message", || {
            general_items(&bob.engine, id)?
                .iter()
                .any(|m| m.text == "after bob joined" && !m.locked)
                .then_some(())
        })
        .await;

        shut_down(bob).await;
        shut_down(seeder).await;
    })
    .await;
}
