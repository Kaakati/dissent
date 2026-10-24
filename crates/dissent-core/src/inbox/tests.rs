//! Offline delivery through an inbox. Bob is offline with his seeder as inbox. Alice writes to him,
//! then goes offline. Bob comes back, collects, decrypts, and the inbox is cleared.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dissent_discovery::ContactCard;
use dissent_identity::{DeviceId, DeviceKey, Identity, RootId};
use dissent_net::{NodeConfig, PeerAddr, RelayChoice};

use crate::engine::Engine;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dissent-core-inbox-{name}-{}-{}",
        std::process::id(),
        now() * 1000 + u64::from(rand_suffix())
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn rand_suffix() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn offline() -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        ..NodeConfig::default()
    }
}

async fn start(identity: Identity, dir: PathBuf) -> Engine {
    let (engine, _events) = Engine::start(identity, dir.clone(), offline(), crate::test_support::vault_for(&dir))
        .await
        .expect("engine starts");
    engine
}

/// Teaches `to` the cert and addresses of `of`'s device, as a contact card would.
fn introduce(to: &Engine, of: &Engine) {
    let card = ContactCard::parse(&of.contact_link()).expect("card parses");
    let mut guard = to.write();
    guard.directory.learn(&card.cert);
    for peer in card.peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()) {
        to.learn_address(&mut guard, peer);
    }
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_message_is_collected_from_the_inbox_and_cleared() {
    let alice_id = Identity::create(now());
    let bob_id = Identity::create(now());
    let bob_root: RootId = bob_id.root_id();
    let alice_root: RootId = alice_id.root_id();
    let alice_dir = scratch("alice");
    let bob_dir = scratch("bob");

    // Bob's seeder is a second device of Bob's root, certified by his root key.
    let root = bob_id.root.clone().expect("bob holds his root key");
    let seeder_device = DeviceKey::generate();
    let seeder_cert = root.certify(seeder_device.id(), now());
    let seeder_id = Identity {
        root: Some(root),
        device: seeder_device,
        cert: seeder_cert,
        device_name: "seeder".into(),
    };
    let seeder_dev: DeviceId = seeder_id.device_id();

    let alice = start(alice_id.clone(), alice_dir.clone()).await;
    let bob = start(bob_id.clone(), bob_dir.clone()).await;
    let seeder = start(seeder_id.clone(), scratch("seeder")).await;

    // Every device learns the others' cards, as contact cards and announcements would teach it.
    introduce(&alice, &bob);
    introduce(&alice, &seeder);
    introduce(&bob, &alice);
    introduce(&bob, &seeder);
    introduce(&seeder, &bob);

    seeder.set_serve_inbox(true).expect("seeder serves its inbox");
    bob.set_inboxes(vec![seeder_dev]).await.expect("bob names his inbox");
    eventually("the seeder holds bob's key packages", || {
        seeder.inbox_stats().key_packages > 0
    })
    .await;

    // Alice learns Bob's inbox through the contact request he accepts.
    let bob_link = bob.contact_link();
    alice
        .request_contact(&bob_link, "")
        .await
        .expect("alice asks bob");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    eventually("bob sees alice's request", || {
        !bob.contact_requests().is_empty()
    })
    .await;
    bob.accept_request(alice_root).await.expect("bob accepts alice");
    eventually("alice is bob's contact", || {
        alice.user_profile(bob_root).is_contact
    })
    .await;

    // Bob goes offline. Alice writes, and then goes offline too.
    bob.shutdown().await.expect("bob shuts down");
    let sent = alice
        .send_dm(bob_root, "while you were away".into())
        .await
        .expect("alice writes to bob");
    assert!(sent.text == "while you were away");
    eventually("the message is queued at the inbox", || {
        alice
            .dm_messages(bob_root, 10)
            .iter()
            .any(|m| m.text == "while you were away" && m.queued)
    })
    .await;
    assert!(seeder.inbox_stats().envelopes > 0, "the inbox holds the mail");
    alice.shutdown().await.expect("alice shuts down");

    // Bob comes back, collects from the inbox, and reads the message.
    let bob = start(bob_id.clone(), bob_dir.clone()).await;
    introduce(&bob, &seeder);
    let handled = bob.collect_inboxes().await.expect("bob collects");
    assert!(handled > 0, "bob collected something");
    eventually("bob reads the message", || {
        bob.dm_messages(alice_root, 10)
            .iter()
            .any(|m| m.text == "while you were away")
    })
    .await;

    // The inbox is cleared once Bob acknowledged the envelopes.
    assert_eq!(seeder.inbox_stats().envelopes, 0, "the inbox is cleared");

    bob.shutdown().await.expect("bob shuts down again");
    seeder.shutdown().await.expect("seeder shuts down");
    let _ = std::fs::remove_dir_all(alice_dir);
    let _ = std::fs::remove_dir_all(bob_dir);
}
