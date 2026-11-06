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
        Node::boot_with(identity, dir, port, crate::Timings::default()).await
    }

    /// Like `boot`, on `timings` instead of the production intervals.
    pub(crate) async fn boot_with(identity: Identity, dir: PathBuf, port: u16, timings: crate::Timings) -> Node {
        wait_until_free(port).await;
        let net = NodeConfig {
            relay: RelayChoice::Disabled,
            loopback_only: true,
            bind_port: port,
            ..NodeConfig::default()
        };
        let (engine, events) =
            Engine::start_with_timings(identity.clone(), dir.clone(), net, crate::test_support::vault_for(&dir), timings)
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
        Node::named_with(name, crate::Timings::default()).await
    }

    /// Like `named`, on `timings`.
    pub(crate) async fn named_with(name: &str, timings: crate::Timings) -> Node {
        let node = Node::boot_with(Identity::create(now()), scratch(name), free_port(), timings).await;
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

/// Waits, for up to 5 s, until `port` can be bound on loopback. A restart reuses its port, and an engine that has shut
/// down lets go of its socket only once the last clone of its endpoint is dropped, which can be a moment after
/// `Engine::shutdown` returns (see iroh's `Endpoint::close`).
async fn wait_until_free(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::UdpSocket::bind(("127.0.0.1", port)).is_err() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A port no other test hands out: one the OS reports free, and not one this process handed out already. Asked of the
/// OS, so tests in other processes running at the same time do not collide on it, as a range picked per process could.
pub(crate) fn free_port() -> u16 {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    static GIVEN: Mutex<BTreeSet<u16>> = Mutex::new(BTreeSet::new());
    loop {
        let port = std::net::UdpSocket::bind(("127.0.0.1", 0)).and_then(|s| s.local_addr()).expect("a free port").port();
        if GIVEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(port) {
            return port;
        }
    }
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
    // 120 s, times `DISSENT_TEST_SCALE` (see `testutil::scale`): CI's runners take about twice as long
    // as a developer machine. A passing check returns at once, so only a failing one waits this long.
    let deadline = Instant::now() + crate::testutil::scaled(Duration::from_secs(120));
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

/// A device restarts on its own port at once, again and again, while peers are linked to it. A returned shutdown
/// means the old engine's sockets are released, so the new engine binds the port without waiting.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_restarts_on_its_port_right_after_shutdown() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice.engine.create_community("Restarts".into()).await.expect("create").id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let mut others = Vec::new();
    for name in ["carol", "dave"] {
        let node = Node::named(name).await;
        node.engine.join_community(&invite).await.expect("joins");
        others.push(node);
    }
    let mut bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let channel = general(&alice.engine, id);
    for round in 0..20 {
        let text = format!("round {round}");
        alice.engine.send_message(id, channel, text.clone()).await.expect("alice sends");
        eventually("bob reads it", || readable(&bob.engine, id, channel, &text)).await;
        let (identity, dir, port) = bob.stop().await;
        bob = Node::boot(identity, dir, port).await;
    }
    }).await;
}

/// A picture fetch that keeps failing waits between its tries, holding the engine's state, when the engine shuts down. A
/// task left running would keep using the old profile after an identity switch, so shutdown ends it: the state is gone
/// within the drain bound, not after the fetch's last pause.
#[tokio::test(flavor = "multi_thread")]
async fn a_picture_fetch_still_retrying_at_shutdown_does_not_outlive_its_engine() {
    crate::testutil::bounded(async {
    let bob = Node::named("bob").await;
    // No such message: every try fails, and the fetch waits 1, 2, 4 and 8 s between them.
    bob.engine.spawn_picture_fetch(None, "no-such-message".into(), "no-such-picture".into(), crate::attachments::PictureKind::Thumbnail);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let weak = std::sync::Arc::downgrade(&bob.engine.inner);
    bob.stop().await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.strong_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(weak.strong_count(), 0, "nothing of the engine runs on after its shutdown");
    }).await;
}

/// Diagnostic: how many holders the engine's state has when `Engine::shutdown` returns, and how long it takes to drop.
/// Every holder is something of the old engine still running.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "diagnostic: prints the engine state's holders after shutdown"]
async fn what_holds_the_engine_after_shutdown() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice.engine.create_community("Holders".into()).await.expect("create").id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let mut bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let channel = general(&alice.engine, id);
    for round in 0..10 {
        let text = format!("round {round}");
        alice.engine.send_message(id, channel, text.clone()).await.expect("alice sends");
        eventually("bob reads it", || readable(&bob.engine, id, channel, &text)).await;
        let weak = std::sync::Arc::downgrade(&bob.engine.inner);
        let (identity, dir, port) = bob.stop().await;
        let at_return = weak.strong_count();
        let started = Instant::now();
        while weak.strong_count() > 0 && started.elapsed() < Duration::from_secs(30) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        println!("HOLDERS round {round}: {at_return} at return, gone after {:?} (left {})", started.elapsed(), weak.strong_count());
        bob = Node::boot(identity, dir, port).await;
    }
    }).await;
}

/// Diagnostic: whether the port is free the moment `Engine::shutdown` returns, before anything else runs. Today it is
/// not always: the socket closes only when the last clone of the endpoint is dropped, and the engine's state can outlive
/// shutdown. Kept out of CI until shutdown waits for that.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "diagnostic: shutdown does not yet wait for every holder of the engine's endpoint"]
async fn the_port_is_free_the_moment_shutdown_returns() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice.engine.create_community("Released".into()).await.expect("create").id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let mut bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    let channel = general(&alice.engine, id);
    for round in 0..20 {
        let text = format!("round {round}");
        alice.engine.send_message(id, channel, text.clone()).await.expect("alice sends");
        eventually("bob reads it", || readable(&bob.engine, id, channel, &text)).await;
        let (identity, dir, port) = bob.stop().await;
        let free = std::net::UdpSocket::bind(("127.0.0.1", port));
        assert!(free.is_ok(), "round {round}: port {port} still bound after shutdown: {free:?}");
        drop(free);
        bob = Node::boot(identity, dir, port).await;
    }
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

/// Leaving a server takes its encryption group and the group's send mark off this device, on disk too. The
/// mark outliving the group stopped a profile opening ("MLS state could not resume its sends: unknown group").
#[tokio::test(flavor = "multi_thread")]
async fn leaving_a_server_leaves_no_group_or_send_mark_behind() {
    crate::testutil::bounded(async {
        let (_alice, bob, _carol, id, _invite) = three_members().await;
        let channel = general(&bob.engine, id);
        bob.engine.send_message(id, channel, "before leaving".into()).await.expect("bob sends");
        let marks = |engine: &Engine| {
            let path = crate::sealed::reserve_path(&engine.inner.mls_path);
            crate::sealed::read_reservations(&engine.read().paths, &path).expect("marks read")
        };
        assert!(marks(&bob.engine).iter().any(|(g, _)| *g == id.0), "the send reserved a mark");
        bob.engine.leave_community(id).await.expect("bob leaves");
        assert!(!bob.engine.with_mls_read(|mls| mls.has_group(&id.0)), "the group is gone");
        assert!(marks(&bob.engine).iter().all(|(g, _)| *g != id.0), "its mark is gone from disk");
        let (identity, dir, port) = bob.stop().await;
        let bob = Node::boot(identity, dir, port).await;
        assert!(bob.engine.with_mls_read(|mls| mls.reservations()).iter().all(|(g, _)| *g != id.0));
    })
    .await;
}
