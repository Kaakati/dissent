//! Encrypted community messages on loopback: members decrypt, outsiders see locked messages, bans
//! cut a device off, late joiners see earlier messages locked, and restarts keep decrypting.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{
    ChannelId, CommunityId, Engine, EngineEvent, Identity, ModAction, NodeConfig, RelayChoice,
    RootId,
};
use tokio::sync::broadcast;

pub(crate) struct Node {
    pub(crate) engine: Engine,
    pub(crate) identity: Identity,
    dir: PathBuf,
    port: u16,
    _events: broadcast::Receiver<EngineEvent>,
}

impl Node {
    pub(crate) async fn boot(identity: Identity, dir: PathBuf, port: u16) -> Node {
        let net = NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            bind_port: port,
            ..NodeConfig::default()
        };
        let (engine, events) = Engine::start(identity.clone(), dir.clone(), net, crate::test_support::vault_for(&dir))
            .await
            .expect("engine starts");
        Node {
            engine,
            identity,
            dir,
            port,
            _events: events,
        }
    }

    pub(crate) async fn named(name: &str) -> Node {
        let node = Node::boot(Identity::create(now()), scratch(name), free_port()).await;
        node.engine
            .set_display_name(name.into())
            .await
            .expect("name");
        node
    }

    /// Shuts the engine down. Returns what a restart needs.
    pub(crate) async fn stop(self) -> (Identity, PathBuf, u16) {
        let Node {
            engine,
            identity,
            dir,
            port,
            _events,
        } = self;
        engine.shutdown().await.expect("clean shutdown");
        (identity, dir, port)
    }

    pub(crate) fn root(&self) -> RootId {
        self.identity.root_id()
    }
}

pub(crate) fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Process ids get reused and a failed run can leave its directories behind. Without the
    // timestamp, a later run with the same pid would open the old state under a new vault key.
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "dissent-mls-{name}-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A port no other test in this process hands out. Binding and releasing a port races with other
/// tests, so ports come from a counter seeded per process instead.
const OFFSET: u16 = 8_000;
pub(crate) fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    // Below Linux's ephemeral range (32768 and up) and Windows' (49152 and up), so the OS never
    // hands one of these to another socket first.
    let base = 20_000 + OFFSET + (std::process::id() % 4_000) as u16;
    base.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn general(engine: &Engine, id: CommunityId) -> ChannelId {
    engine
        .community_view(id)
        .expect("community view")
        .channels
        .iter()
        .find(|c| c.name == "general")
        .and_then(|c| c.id.parse().ok())
        .expect("general channel")
}

pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    // 120 s: CI's runners take about twice as long as a developer machine. A passing check returns at
    // once, so only a failing one waits this long.
    let deadline = Instant::now() + Duration::from_secs(120);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The message text as `engine` sees it, if the message is there and readable.
pub(crate) fn readable(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(id, channel, 100_000)
        .iter()
        .any(|m| m.text == text && !m.locked)
}

/// Alice founds the community, Bob and Carol join it. Everyone is in the same MLS epoch.
pub(crate) async fn three_members() -> (Node, Node, Node, CommunityId, String) {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Vault".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let carol = Node::named("carol").await;
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");
    for node in [&alice, &bob, &carol] {
        eventually("three members", || {
            node.engine.community_view(id).map(|v| v.members.len()).ok() == Some(3)
        })
        .await;
    }
    // Commits reach members by gossip, which can miss one. Bob pulls the ones he lacks from Alice.
    let alice_device = alice.identity.device_id();
    bob.engine.mls_catch_up(id, alice_device).await;
    let epoch = carol.engine.mls_epoch(id);
    eventually("bob at carol's epoch", || bob.engine.mls_epoch(id) == epoch).await;
    eventually("alice at carol's epoch", || {
        alice.engine.mls_epoch(id) == epoch
    })
    .await;
    (alice, bob, carol, id, invite)
}

#[tokio::test(flavor = "multi_thread")]
async fn members_decrypt_and_an_outsider_sees_only_locked_messages() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, invite) = three_members().await;
    let channel = general(&alice.engine, id);
    alice
        .engine
        .send_message(id, channel, "from alice".into())
        .await
        .expect("alice");
    bob.engine
        .send_message(id, channel, "from bob".into())
        .await
        .expect("bob");
    carol
        .engine
        .send_message(id, channel, "from carol".into())
        .await
        .expect("carol");
    for node in [&alice, &bob, &carol] {
        eventually("every member reads all three", || {
            ["from alice", "from bob", "from carol"]
                .iter()
                .all(|t| readable(&node.engine, id, channel, t))
        })
        .await;
    }

    // Dave syncs the DAG without joining the group, so he holds no keys.
    let dave = Node::named("dave").await;
    dave.engine
        .join_without_group(&invite)
        .await
        .expect("dave syncs the DAG");
    eventually("dave sees the three messages", || {
        dave.engine.channel_messages(id, channel, 100).len() == 3
    })
    .await;
    for item in dave.engine.channel_messages(id, channel, 100) {
        assert!(item.locked, "outsider must not read {item:?}");
        assert!(item.text.is_empty(), "outsider got plaintext: {item:?}");
    }

    // Permission state is identical everywhere, whatever keys each device holds.
    let state = |n: &Node| {
        n.engine
            .read()
            .communities
            .get(&id)
            .unwrap()
            .state()
            .clone()
    };
    // Each device's own events (dave's join and its history support) reach the others a moment apart, so wait
    // for the replays to settle before comparing them.
    eventually("the four replays settle on one state", || {
        let a = state(&alice);
        a == state(&bob) && a == state(&carol) && a == state(&dave)
    })
    .await;
    let (a, b, c) = (state(&alice), state(&bob), state(&carol));
    assert_eq!(a, b, "alice and bob agree on state");
    assert_eq!(a, c, "alice and carol agree on state");
    assert_eq!(a, state(&dave), "the outsider agrees on state");
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn banned_device_cannot_read_new_messages() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, _invite) = three_members().await;
    let channel = general(&alice.engine, id);
    let bob_root = bob.root();
    alice
        .engine
        .moderate(id, ModAction::Ban, bob_root)
        .await
        .expect("alice bans bob");
    // Bob is banned, so he is refused every community request, and he never learns the removal
    // commit or the new topic. He keeps no access to new messages.
    tokio::time::sleep(Duration::from_secs(2)).await;

    alice
        .engine
        .send_message(id, channel, "after the ban".into())
        .await
        .expect("alice posts");
    eventually("carol reads it", || {
        readable(&carol.engine, id, channel, "after the ban")
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !bob.engine
            .channel_messages(id, channel, 100)
            .iter()
            .any(|m| m.text == "after the ban" || m.text.is_empty() && !m.locked),
        "bob must not see the post"
    );
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn late_joiner_sees_earlier_messages_locked_and_new_ones_in_plaintext() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Late".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let channel = general(&alice.engine, id);
    alice
        .engine
        .send_message(id, channel, "earlier".into())
        .await
        .expect("alice");
    for _ in 0..100 {
        if readable(&bob.engine, id, channel, "earlier") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(readable(&bob.engine, id, channel, "earlier"));

    let carol = Node::named("carol").await;
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");
    eventually("carol is in the group", || {
        carol.engine.mls_epoch(id).is_some()
    })
    .await;
    alice
        .engine
        .send_message(id, channel, "later".into())
        .await
        .expect("alice");
    eventually("carol reads later", || {
        readable(&carol.engine, id, channel, "later")
    })
    .await;
    let items = carol.engine.channel_messages(id, channel, 100);
    assert!(
        items.iter().any(|m| m.locked && m.text.is_empty()),
        "earlier message is locked for carol: {items:?}"
    );
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_keeps_the_ability_to_decrypt() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Restart".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let channel = general(&alice.engine, id);
    alice
        .engine
        .send_message(id, channel, "pre".into())
        .await
        .expect("alice");
    eventually("bob reads pre", || {
        readable(&bob.engine, id, channel, "pre")
    })
    .await;

    let (identity, dir, port) = bob.stop().await;
    let bob = Node::boot(identity, dir, port).await;
    assert!(
        bob.engine.mls_epoch(id).is_some(),
        "the group survives the restart"
    );
    assert!(
        readable(&bob.engine, id, channel, "pre"),
        "the earlier plaintext survives the restart"
    );
    alice
        .engine
        .send_message(id, channel, "post".into())
        .await
        .expect("alice");
    eventually("bob decrypts after the restart", || {
        readable(&bob.engine, id, channel, "post")
    })
    .await;
    }).await;
}

/// Bob misses the commit that adds Carol, because he is offline. When he returns, Alice writes at
/// the newer epoch. Bob sees a message from a later epoch than his own, so he fetches the missed
/// commit from its author and decrypts it, with no sync needed.
#[tokio::test(flavor = "multi_thread")]
async fn member_that_missed_a_commit_catches_up_on_a_future_epoch_message() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Catch".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let (bob_id, bob_dir, bob_port) = bob.stop().await;

    let carol = Node::named("carol").await;
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");
    eventually("carol is in the group", || {
        carol.engine.mls_epoch(id).is_some()
    })
    .await;

    let bob = Node::boot(bob_id, bob_dir, bob_port).await;
    assert_eq!(
        bob.engine.mls_epoch(id),
        Some(1),
        "bob is still one epoch behind"
    );
    let channel = general(&alice.engine, id);
    alice
        .engine
        .send_message(id, channel, "at the newer epoch".into())
        .await
        .expect("alice posts");
    eventually("bob catches up and reads it", || {
        readable(&bob.engine, id, channel, "at the newer epoch")
    })
    .await;
    assert_eq!(bob.engine.mls_epoch(id), alice.engine.mls_epoch(id));
    }).await;
}
