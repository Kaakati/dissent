//! Identity change notices on loopback: a rotation by someone we share a server with, and a new
//! device of someone we have messaged.

use std::time::Duration;

use dissent_events::{ChannelId, Event, EventBody, Hlc, Payload, SignedEvent};
use dissent_identity::{CommunityId, DeviceKey, KeyRotation, RootKey};
use tokio::sync::broadcast;

use crate::membership::tests::{free_port, now, scratch, Node};
use crate::{EngineEvent, Identity};

/// Waits for an event on `events` that `want` accepts, or panics after thirty seconds.
async fn expect_event(
    events: &mut broadcast::Receiver<EngineEvent>,
    what: &str,
    want: impl Fn(&EngineEvent) -> bool,
) {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(ev) if want(&ev) => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("events closed"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .unwrap_or_else(|_| panic!("no notice: {what}"));
}

/// Alice founds a community and Bob joins it. Returns both, the community and an invite.
async fn alice_and_bob() -> (Node, Node, CommunityId, String) {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Notices".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let bob = Node::named("bob").await;
    bob.engine.join_community(&invite).await.expect("bob joins");
    (alice, bob, id, invite)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotation_by_someone_we_share_a_server_with_raises_a_notice() {
    crate::testutil::bounded(async {
    let (alice, bob, id, _invite) = alice_and_bob().await;
    let old = bob.identity.root.clone().expect("bob holds root");
    // Only the new root can announce a rotation, so the event comes from a device it certifies.
    let new = RootKey::generate();
    let new_text = new.id().to_string();
    let device = DeviceKey::generate();
    let cert = new.certify(device.id(), now());
    let rotation = KeyRotation::by_owner(&old, &new, now());
    let heads = alice.engine.read().communities[&id].heads();
    let body = EventBody {
        community: id,
        channel: ChannelId::CONTROL,
        author: new.id(),
        device: device.id(),
        parents: heads,
        hlc: Hlc {
            wall_ms: now(),
            counter: 0,
        },
        payload: Payload::KeyRotation(rotation),
    };
    let signed = SignedEvent {
        event: Event::sign(body, &device),
        cert,
    };

    let mut events = alice.engine.subscribe();
    alice
        .engine
        .ingest(id, vec![signed])
        .expect("alice ingests the rotation");

    expect_event(&mut events, "a rotated notice", |ev| {
        matches!(ev, EngineEvent::IdentityChanged { user, reason }
            if *user == new_text && reason == "rotated")
    })
    .await;
    assert!(alice
        .engine
        .user_profile(new.id())
        .identity_changed_ms
        .is_some());
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_of_someone_we_messaged_raises_a_notice() {
    crate::testutil::bounded(async {
    let (alice, bob, _id, invite) = alice_and_bob().await;
    let bob_root = bob.identity.root_id();
    alice
        .engine
        .send_dm(bob_root, "hello".into())
        .await
        .expect("alice messages bob");

    let mut events = alice.engine.subscribe();
    // A second device of Bob's root, certified by his root key.
    let root = bob.identity.root.clone().expect("bob holds root");
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), now());
    let phone_identity = Identity {
        root: Some(root),
        device,
        cert,
        device_name: "phone".into(),
    };
    let phone = Node::boot(phone_identity, scratch("phone"), free_port()).await;
    phone
        .engine
        .join_community(&invite)
        .await
        .expect("phone joins");

    let bob_text = bob_root.to_string();
    expect_event(&mut events, "a new device notice", |ev| {
        matches!(ev, EngineEvent::IdentityChanged { user, reason }
            if *user == bob_text && reason == "new_device")
    })
    .await;
    assert!(alice.engine.user_profile(bob_root).identity_changed_ms.is_some());
    }).await;
}
