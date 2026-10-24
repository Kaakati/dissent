//! Anti-entropy rotation: one offline member must not stop a round from reaching a live one.
//! Loopback only. Gossip cannot deliver the missed message, so anti-entropy has to.

use crate::{ChannelId, CommunityId, DeviceId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice};
use std::net::UdpSocket;
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
        let (engine, events) = Engine::start(identity.clone(), dir.clone(), net)
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
    let dir = std::env::temp_dir().join(format!(
        "dissent-rotation-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

async fn wait_for_members(engine: &Engine, id: CommunityId, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.community_view(id).map(|v| v.members.len()).ok() != Some(count) {
        assert!(Instant::now() < deadline, "membership never reached {count}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_first_member_does_not_block_anti_entropy() {
    let alice = Node::boot(Identity::create(now()), scratch("alice"), free_port()).await;
    alice.engine.set_display_name("alice".into()).await.expect("name");
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
    bob.engine.set_display_name("bob".into()).await.expect("name");
    bob.engine.join_community(&invite).await.expect("bob joins");
    let carol = Node::boot(Identity::create(now()), scratch("carol"), free_port()).await;
    carol.engine.set_display_name("carol".into()).await.expect("name");
    carol.engine.join_community(&invite).await.expect("carol joins");
    for node in [&alice, &bob, &carol] {
        wait_for_members(&node.engine, id, 3).await;
    }
    // Carol must be able to dial both other members. Joins teach her only the founder's address,
    // and the rest of the addresses depend on connections that happened to be made. Contact cards
    // carry each member's address, so add both as contacts.
    carol
        .engine
        .add_contact(&alice.engine.contact_link())
        .await
        .expect("carol adds alice");
    carol
        .engine
        .add_contact(&bob.engine.contact_link())
        .await
        .expect("carol adds bob");

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
}

fn has_message(engine: &Engine, id: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(id, channel, 100_000)
        .iter()
        .any(|m| m.text == text)
}
