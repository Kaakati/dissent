//! Anti-entropy rotation: one offline member must not stop a round from reaching a live one.
//! Loopback only. Gossip cannot deliver the missed message, so anti-entropy has to.

use crate::{
    ChannelId, CommunityId, DeviceId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice,
};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

struct Node {
    engine: Engine,
    identity: Identity,
    dir: PathBuf,
    port: u16,
    _events: broadcast::Receiver<EngineEvent>,
}

impl Node {
    async fn boot(identity: Identity, dir: PathBuf, port: u16) -> Node {
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

    /// Shuts the engine down. Returns what a restart needs: identity, directory and port.
    async fn stop(self) -> (Identity, PathBuf, u16) {
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
}

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "dissent-rotation-{name}-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A port no other test in this process hands out. Binding and releasing a port races with other
/// tests, so ports come from a counter seeded per process instead.
const OFFSET: u16 = 0;
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let base = 30_000 + OFFSET + (std::process::id() % 20_000) as u16;
    base.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The community's members' devices, in the order anti-entropy tries them.
fn member_devices(engine: &Engine, id: CommunityId) -> Vec<DeviceId> {
    let w = engine.read();
    let c = w.communities.get(&id).expect("community is known");
    w.member_devices(c)
}

fn general_channel(engine: &Engine, id: CommunityId) -> ChannelId {
    engine
        .community_view(id)
        .expect("community view")
        .channels
        .iter()
        .find(|c| c.name == "general")
        .and_then(|c| c.id.parse().ok())
        .expect("general channel")
}

/// Waits until the engine holds a device's signed address. Addresses arrive by gossip and replies,
/// so the test must not read them before they have landed.
async fn wait_signed(engine: &Engine, device: DeviceId) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine.read().addresses.has_signed(&device) {
        assert!(Instant::now() < deadline, "no signed address for {device}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_members(engine: &Engine, id: CommunityId, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.community_view(id).map(|v| v.members.len()).ok() != Some(count) {
        assert!(
            Instant::now() < deadline,
            "membership never reached {count}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_first_member_does_not_block_anti_entropy() {
    crate::testutil::bounded(async {
    let alice = Node::boot(Identity::create(now()), scratch("alice"), free_port()).await;
    alice
        .engine
        .set_display_name("alice".into())
        .await
        .expect("name");
    let id: CommunityId = alice
        .engine
        .create_community("Rota".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");

    let bob = Node::boot(Identity::create(now()), scratch("bob"), free_port()).await;
    bob.engine
        .set_display_name("bob".into())
        .await
        .expect("name");
    bob.engine.join_community(&invite).await.expect("bob joins");
    // The founder must know Bob's address before a later joiner asks for it.
    wait_signed(&alice.engine, bob.identity.device_id()).await;
    let carol = Node::boot(Identity::create(now()), scratch("carol"), free_port()).await;
    carol
        .engine
        .set_display_name("carol".into())
        .await
        .expect("name");
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");
    for node in [&alice, &bob, &carol] {
        wait_for_members(&node.engine, id, 3).await;
    }
    // Carol learns each member's signed address from the replies she gets while joining.
    wait_signed(&carol.engine, alice.identity.device_id()).await;
    wait_signed(&carol.engine, bob.identity.device_id()).await;

    // Carol is the observer. From her side, the first member in rotation is X and the second is Y.
    let order = member_devices(&carol.engine, id);
    assert_eq!(order.len(), 2, "carol sees the other two members");
    let (x_device, y_device) = (order[0], order[1]);
    let (x, y) = if alice.identity.device_id() == x_device {
        (alice, bob)
    } else {
        (bob, alice)
    };
    assert_eq!(x.identity.device_id(), x_device);
    assert_eq!(y.identity.device_id(), y_device);

    // Y must hold the current epoch before it writes. Commits reach members by gossip, which can
    // miss a member, so Y pulls the missed commits from X while X is still up.
    y.engine.mls_catch_up(id, x_device).await;
    wait_epoch(&y.engine, id, carol.engine.mls_epoch(id)).await;

    // X and Carol go offline. Y posts a message that neither of them can receive by gossip.
    let (_x_identity, x_dir, _x_port) = x.stop().await;
    let (c_identity, c_dir, c_port) = carol.stop().await;
    let general = general_channel(&y.engine, id);
    y.engine
        .send_message(id, general, "missed-while-away".into())
        .await
        .expect("y posts");

    // Carol restarts. Her startup does not sync, and gossip never replays the message.
    let carol = Node::boot(c_identity, c_dir.clone(), c_port).await;
    assert!(
        !has_message(&carol.engine, id, general, "missed-while-away"),
        "the message is missing before anti-entropy"
    );

    // One round. X is first in rotation and fails, so the round must reach Y.
    carol.engine.anti_entropy().await;
    assert!(
        has_message(&carol.engine, id, general, "missed-while-away"),
        "anti-entropy reached the live member"
    );
    assert!(
        carol
            .engine
            .inner
            .sync_backoff
            .lock()
            .unwrap()
            .contains_key(&x_device),
        "the offline member was tried and backed off"
    );

    let (_, c_dir, _) = carol.stop().await;
    let (_, y_dir, _) = y.stop().await;
    for dir in [c_dir, x_dir, y_dir] {
        let _ = std::fs::remove_dir_all(dir);
    }
    }).await;
}

/// Carol joins from an invite that names only Alice. Alice then goes offline. Carol must still
/// reach Bob, using the signed addresses that Alice and Bob passed on. No contacts are exchanged.
#[tokio::test(flavor = "multi_thread")]
async fn joiner_reaches_members_it_was_not_introduced_to() {
    crate::testutil::bounded(async {
    let alice = Node::boot(Identity::create(now()), scratch("alice2"), free_port()).await;
    alice
        .engine
        .set_display_name("alice".into())
        .await
        .expect("name");
    let id: CommunityId = alice
        .engine
        .create_community("Introductions".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");

    let bob = Node::boot(Identity::create(now()), scratch("bob2"), free_port()).await;
    bob.engine
        .set_display_name("bob".into())
        .await
        .expect("name");
    bob.engine.join_community(&invite).await.expect("bob joins");
    wait_signed(&alice.engine, bob.identity.device_id()).await;

    let carol = Node::boot(Identity::create(now()), scratch("carol2"), free_port()).await;
    carol
        .engine
        .set_display_name("carol".into())
        .await
        .expect("name");
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");
    wait_for_members(&carol.engine, id, 3).await;
    wait_signed(&carol.engine, bob.identity.device_id()).await;

    // Alice goes offline. Carol's only route to Bob is the address she was passed.
    let (_, alice_dir, _) = alice.stop().await;
    let bob_root = bob.identity.root_id();
    let carol_root = carol.identity.root_id();
    carol
        .engine
        .send_dm(bob_root, "hi bob".into())
        .await
        .expect("carol DMs bob");
    let acked = Instant::now() + Duration::from_secs(10);
    while !carol
        .engine
        .dm_messages(bob_root, 10)
        .iter()
        .any(|m| m.text == "hi bob" && !m.pending)
    {
        assert!(Instant::now() < acked, "carol's first DM was never acknowledged");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while !bob
        .engine
        .dm_messages(carol_root, 100)
        .iter()
        .any(|m| m.text == "hi bob")
    {
        assert!(Instant::now() < deadline, "bob never received carol's DM");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    carol
        .engine
        .sync_with(id, bob.identity.device_id())
        .await
        .expect("carol syncs with bob's device");

    let (_, carol_dir, _) = carol.stop().await;
    let (_, bob_dir, _) = bob.stop().await;
    for dir in [alice_dir, carol_dir, bob_dir] {
        let _ = std::fs::remove_dir_all(dir);
    }
    }).await;
}

fn has_message(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(id, channel, 100_000)
        .iter()
        .any(|m| m.text == text)
}

/// Waits until `engine` reaches the given MLS epoch.
async fn wait_epoch(engine: &Engine, id: CommunityId, want: Option<u64>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.mls_epoch(id) != want {
        assert!(Instant::now() < deadline, "epoch never reached {want:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drains every event a receiver holds now.
fn drain(rx: &mut broadcast::Receiver<EngineEvent>) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// Counts the events of one kind for one channel.
fn count_for(events: &[EngineEvent], channel: ChannelId, kind: fn(&EngineEvent, &str) -> bool) -> usize {
    events.iter().filter(|ev| kind(ev, &channel.to_string())).count()
}

/// Waits until `engine` holds `text` in `channel`.
async fn wait_text(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !has_message(engine, id, channel, text) {
        assert!(Instant::now() < deadline, "no message {text:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The event for a stored message is emitted before the store is read back; let it land.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// A message that arrives by gossip, and is then offered again by a sync round, is announced once. An edit and a
/// reaction change a message that exists, so they announce no new one.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_announced_once_however_it_arrives() {
    crate::testutil::bounded(async {
    let alice = Node::boot(Identity::create(now()), scratch("once-alice"), free_port()).await;
    alice.engine.set_display_name("alice".into()).await.expect("name");
    let id: CommunityId = alice
        .engine
        .create_community("Once".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::boot(Identity::create(now()), scratch("once-bob"), free_port()).await;
    bob.engine.set_display_name("bob".into()).await.expect("name");
    bob.engine.join_community(&invite).await.expect("bob joins");
    wait_for_members(&alice.engine, id, 2).await;
    wait_for_members(&bob.engine, id, 2).await;
    let general = general_channel(&alice.engine, id);
    let mut events = bob.engine.subscribe();
    let is_message = |ev: &EngineEvent, channel: &str| {
        matches!(ev, EngineEvent::Message { channel_id, .. } if channel_id == channel)
    };
    let is_changed = |ev: &EngineEvent, channel: &str| {
        matches!(ev, EngineEvent::MessagesChanged { channel_id, .. } if channel_id == channel)
    };

    // Gossip delivers the message while Bob is online: one new message.
    let sent = alice
        .engine
        .send_message(id, general, "announced-once".into())
        .await
        .expect("alice posts");
    wait_text(&bob.engine, id, general, "announced-once").await;
    let by_gossip = drain(&mut events);
    assert_eq!(count_for(&by_gossip, general, is_message), 1, "one new message from gossip");

    // A sync round offers the same message again. Bob holds it already, so nothing new is announced.
    bob.engine.anti_entropy().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let by_sync = drain(&mut events);
    assert_eq!(count_for(&by_sync, general, is_message), 0, "a sync of a held message announces nothing");

    // An edit and a reaction change the message. They re-read the channel, and announce no new message.
    let target: dissent_events::EventId = sent.id.parse().expect("message id");
    alice
        .engine
        .edit_message(id, general, target, "announced-once (edited)".into())
        .await
        .expect("alice edits");
    wait_text(&bob.engine, id, general, "announced-once (edited)").await;
    alice
        .engine
        .react(id, general, target, "\u{1F44D}".into())
        .await
        .expect("alice reacts");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let by_change = drain(&mut events);
    assert_eq!(count_for(&by_change, general, is_message), 0, "an edit or a reaction is not a new message");
    assert!(count_for(&by_change, general, is_changed) >= 2, "views re-read on the edit and the reaction");

    let (_, a_dir, _) = alice.stop().await;
    let (_, b_dir, _) = bob.stop().await;
    for dir in [a_dir, b_dir] {
        let _ = std::fs::remove_dir_all(dir);
    }
    }).await;
}
