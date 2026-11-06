//! The connection gate: a revoked or banned device is refused on every protocol, a device we know nothing about is a
//! stranger and still joins, strangers share one pool that a member never waits on, one device gets at most its cap,
//! and a contact we share no server with is known.

use crate::common::*;
use dissent_core::test_support::{connection_verdict, dial_and_watch, node_addr, GATED_ALPNS};
use dissent_core::{start_device_link, CommunityId, ModAction};
use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};
use iroh::EndpointId;
use std::time::Duration;

/// The close code a refused device gets.
const REFUSED: u64 = 5;
/// The close code of a full pool or a device over its cap.
const BUSY: u64 = 3;
const GOSSIP: &[u8] = b"/iroh-gossip/1";

/// A bare node on loopback that knows `host`'s address: a device nobody has a certificate for.
async fn stranger(host: &Peer) -> Node {
    let config = NodeConfig { relay: RelayChoice::Disabled, loopback_only: true, dht_lookup: false, ..Default::default() };
    let node = Node::spawn(&DeviceKey::generate(), config).await.expect("spawn a stranger");
    node.remember(&node_addr(&host.engine).await).expect("remember the host");
    node
}

async fn wait_verdict(host: &Peer, device: dissent_identity::DeviceId, want: &str) {
    eventually_within(Duration::from_secs(30), "the gate's verdict", || {
        (connection_verdict(&host.engine, device) == want).then_some(())
    })
    .await;
}

/// Links a device to `owner`'s root, and has it join `community` through `inviter`, so the members learn its
/// certificate.
async fn link_device(owner: &Peer, name: &str, community: CommunityId, inviter: &Peer) -> Peer {
    let pending = start_device_link(name).expect("link request");
    let grant = owner.engine.approve_device(&pending.link().expect("link")).expect("owner approves the device");
    let device = start(pending.finish(&grant).expect("device identity"), scratch(name)).await;
    let invite = inviter.engine.create_invite(community).await.expect("invite");
    device.engine.join_community(&invite).await.expect("device joins the shared server");
    device
}

fn closed_busy(conn: &iroh::endpoint::Connection) -> bool {
    matches!(
        conn.close_reason(),
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) if close.error_code.into_inner() == BUSY
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_banned_or_revoked_device_is_refused_on_every_protocol_and_a_member_is_not() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;
    let community = rustaceans(&alice, &bob).await;
    let invite = alice.engine.create_invite(community).await.expect("invite");
    join_ready(&alice, &carol, &invite).await;
    let tablet = link_device(&bob, "bob-tablet", community, &alice).await;
    wait_verdict(&alice, tablet.identity.device_id(), "known").await;

    alice.engine.moderate(community, ModAction::Ban, carol.identity.root_id()).await.expect("alice bans carol");
    // As in the device tests: the link settles on every side before it can be revoked.
    tokio::time::sleep(Duration::from_secs(2)).await;
    bob.engine.revoke_device(tablet.identity.device_id()).await.expect("bob revokes the tablet");
    wait_verdict(&alice, carol.identity.device_id(), "refused").await;
    wait_verdict(&alice, tablet.identity.device_id(), "refused").await;
    assert_eq!(connection_verdict(&alice.engine, bob.identity.device_id()), "known", "bob's own device is still known");

    let alice_device = alice.identity.device_id();
    for alpn in GATED_ALPNS {
        let name = String::from_utf8_lossy(alpn);
        let banned = dial_and_watch(&carol.engine, alice_device, alpn, Duration::from_secs(3)).await;
        assert_eq!(banned, Ok(Some(REFUSED)), "a banned member is refused on {name}");
        let revoked = dial_and_watch(&tablet.engine, alice_device, alpn, Duration::from_secs(3)).await;
        assert_eq!(revoked, Ok(Some(REFUSED)), "a revoked device is refused on {name}");
    }
    let member = dial_and_watch(&bob.engine, alice_device, GOSSIP, Duration::from_secs(2)).await;
    assert_ne!(member, Ok(Some(REFUSED)), "a member is not refused");

    finish(vec![alice, bob, carol, tablet]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_device_we_know_nothing_about_is_a_stranger_and_still_joins() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let dave = peer("dave").await;
    let community = rustaceans(&alice, &bob).await;
    assert_eq!(connection_verdict(&alice.engine, dave.identity.device_id()), "stranger", "unknown is not refused");
    let invite = alice.engine.create_invite(community).await.expect("invite");
    join_ready(&alice, &dave, &invite).await;
    wait_verdict(&alice, dave.identity.device_id(), "known").await;
    finish(vec![alice, bob, dave]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_flood_of_strangers_fills_their_pool_and_a_member_still_gets_in() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_id = EndpointId::from_bytes(&alice.identity.device_id().0).unwrap();

    // 300 connections from 10 strangers, 30 each: under the per-device cap of 32, over the pool of 256.
    let mut strangers = Vec::new();
    let mut held = Vec::new();
    for _ in 0..10 {
        let node = stranger(&alice).await;
        for _ in 0..30 {
            held.push(node.endpoint().connect(alice_id, GOSSIP).await.expect("the handshake completes"));
        }
        strangers.push(node);
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    // The pool is shared with every other stranger, such as a connection bob opened before alice knew him, so it can
    // hold a few fewer of these.
    let busy = held.iter().filter(|c| closed_busy(c)).count();
    let open = held.iter().filter(|c| c.close_reason().is_none()).count();
    assert!((250..=256).contains(&open) && open + busy == 300, "strangers get 256 places together, and no more: {open} open, {busy} busy");

    let member = dial_and_watch(&bob.engine, alice.identity.device_id(), GOSSIP, Duration::from_secs(2)).await;
    assert_eq!(member, Ok(None), "a member gets in while the strangers' pool is full");

    drop(held);
    for node in strangers {
        node.shutdown().await.unwrap();
    }
    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn one_device_gets_at_most_its_cap_of_connections() {
    let alice = peer("alice").await;
    let alice_id = EndpointId::from_bytes(&alice.identity.device_id().0).unwrap();
    let node = stranger(&alice).await;
    let mut held = Vec::new();
    for _ in 0..33 {
        held.push(node.endpoint().connect(alice_id, GOSSIP).await.expect("the handshake completes"));
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let busy = held.iter().filter(|c| closed_busy(c)).count();
    let open = held.iter().filter(|c| c.close_reason().is_none()).count();
    assert_eq!((busy, open), (1, 32), "only the 33rd connection is turned away");
    drop(held);
    node.shutdown().await.unwrap();
    finish(vec![alice]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_contact_we_share_no_server_with_is_known() {
    let alice = peer("alice").await;
    let erin = peer("erin").await;
    assert_eq!(connection_verdict(&alice.engine, erin.identity.device_id()), "stranger");
    make_contacts_ready(&alice, &erin).await;
    wait_verdict(&alice, erin.identity.device_id(), "known").await;
    finish(vec![alice, erin]).await;
}
