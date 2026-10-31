//! Offline delivery through an inbox. Bob is offline with his seeder as inbox. Alice writes to him,
//! then goes offline. Bob comes back, collects, decrypts, and the inbox is cleared.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dissent_discovery::ContactCard;
use dissent_identity::{DeviceId, DeviceKey, Identity, RootId};
use dissent_net::{NodeConfig, PeerAddr, RelayChoice};

use crate::engine::Engine;
use crate::conv_actions::parse_message_id;
use crate::groups::{ConvPacket, REFUSED_TTL_MS};
use crate::contacts::ContactMsg;
use crate::inbox::{
    sign_over, Collect, DepositKeys, Envelope, InboxStats, Probe, ProbeAnswer, COLLECT_DOMAIN, DEPOSITS_PER_HOUR,
    DEPOSIT_DOMAIN, KEYS_DOMAIN, KEY_PACKAGES_PER_DEVICE, MAX_AGE_MS, MAX_HOSTED, NOT_SERVED, PROBE_DOMAIN,
};
use crate::rpc::Request;
use crate::views::EngineEvent;
use crate::rpc::{decode, Response};
use crate::views::DmItem;

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
    let deadline = Instant::now() + crate::testutil::scaled(Duration::from_secs(60));
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
    // `send_dm` returns queued at once; the welcome and the message reach the inbox in the background,
    // and alice must wait for both before she goes offline.
    eventually("the inbox holds the welcome and the message", || seeder.inbox_stats().envelopes >= 2).await;
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

/// The people in an offline test: Alice, Bob, who is offline, and Bob's seeder, which is his inbox.
struct Offline {
    alice: Engine,
    alice_root: RootId,
    bob_id: Identity,
    bob_dir: PathBuf,
    bob_root: RootId,
    seeder: Engine,
    seeder_id: Identity,
    seeder_dir: PathBuf,
}

/// Alice is Bob's contact, and Bob's seeder is his inbox. Bob then goes offline, so what Alice sends him waits at the
/// seeder.
async fn bob_offline_behind_his_inbox(tag: &str) -> Offline {
    let alice_id = Identity::create(now());
    let bob_id = Identity::create(now());
    let bob_root: RootId = bob_id.root_id();
    let alice_root: RootId = alice_id.root_id();
    let alice_dir = scratch(&format!("{tag}-alice"));
    let bob_dir = scratch(&format!("{tag}-bob"));
    let seeder_dir = scratch(&format!("{tag}-seeder"));

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

    let alice = start(alice_id.clone(), alice_dir).await;
    let bob = start(bob_id.clone(), bob_dir.clone()).await;
    let seeder = start(seeder_id.clone(), seeder_dir.clone()).await;
    introduce(&alice, &bob);
    introduce(&alice, &seeder);
    introduce(&bob, &alice);
    introduce(&bob, &seeder);
    introduce(&seeder, &bob);

    seeder.set_serve_inbox(true).expect("seeder serves its inbox");
    bob.set_inboxes(vec![seeder_dev]).await.expect("bob names his inbox");
    eventually("the seeder holds bob's key packages", || seeder.inbox_stats().key_packages > 0).await;

    alice.request_contact(&bob.contact_link(), "").await.expect("alice asks bob");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    eventually("bob sees alice's request", || !bob.contact_requests().is_empty()).await;
    bob.accept_request(alice_root).await.expect("bob accepts alice");
    eventually("alice is bob's contact", || alice.user_profile(bob_root).is_contact).await;

    bob.shutdown().await.expect("bob shuts down");
    Offline {
        alice,
        alice_root,
        bob_id,
        bob_dir,
        bob_root,
        seeder,
        seeder_id,
        seeder_dir,
    }
}

/// Alice's messages to Bob, in the order they were written.
fn sent(alice: &Engine, bob_root: RootId) -> Vec<DmItem> {
    alice
        .dm_messages(bob_root, 200)
        .into_iter()
        .filter(|m| m.text.starts_with("message "))
        .collect()
}

/// Makes deposits at the seeder from Alice's device, as if she had sent them, until the hourly limit is reached.
/// Each one is a valid envelope for Bob, so the seeder counts it like any other.
fn fill_hourly_limit(alice: &Engine, seeder: &Engine, bob_root: RootId) {
    let me = alice.inner.identity.clone();
    let room = DEPOSITS_PER_HOUR - seeder.inbox_stats().envelopes;
    for n in 0..room {
        let packet = format!("filler {n}").into_bytes();
        let id = *blake3::hash(&packet).as_bytes();
        let recipient = DeviceKey::generate().id();
        let expires_ms = now() + MAX_AGE_MS / 2;
        let sig = sign_over(&me.device, DEPOSIT_DOMAIN, &(recipient, bob_root, id, expires_ms, &packet))
            .expect("the filler deposit signs");
        let env = Envelope {
            recipient,
            recipient_root: bob_root,
            id,
            expires_ms,
            packet,
            from: me.cert.clone(),
            sig,
        };
        assert!(matches!(seeder.receive_deposit(env), Response::Ack), "the inbox takes filler {n}");
    }
    assert_eq!(seeder.inbox_stats().envelopes, DEPOSITS_PER_HOUR, "the inbox is at its hourly limit");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_over_the_hourly_deposit_limit_is_refused_and_its_message_fails() {
    let Offline { alice, seeder, bob_root, .. } = bob_offline_behind_his_inbox("limit").await;

    // The first message is deposited with the welcome that starts the group: two envelopes.
    alice.send_dm(bob_root, "message 0".into()).await.expect("alice writes to bob");
    eventually("the first message is deposited", || seeder.inbox_stats().envelopes >= 2).await;
    fill_hourly_limit(&alice, &seeder, bob_root);

    alice.send_dm(bob_root, "message 1".into()).await.expect("alice writes to bob");
    eventually("the second message is refused", || {
        sent(&alice, bob_root).last().is_some_and(|m| m.delivery == "failed")
    })
    .await;

    let mine = sent(&alice, bob_root);
    assert_eq!(mine.first().map(|m| m.delivery.as_str()), Some("queued_inbox"), "the first message was deposited");
    let last = mine.last().expect("messages were sent");
    assert_eq!(last.reason.as_deref(), Some("deposit rate limit reached"), "the inbox's reason is shown");
    assert!(last.retryable, "a refused message can be sent again");
    assert_eq!(seeder.inbox_stats().envelopes, DEPOSITS_PER_HOUR, "the refused deposit was not stored");

    alice.shutdown().await.expect("alice shuts down");
    seeder.shutdown().await.expect("seeder shuts down");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_message_is_sent_again_sealed_anew_once_the_inbox_accepts() {
    let Offline {
        alice,
        alice_root,
        bob_id,
        bob_dir,
        bob_root,
        seeder,
        seeder_id,
        seeder_dir,
    } = bob_offline_behind_his_inbox("retry").await;

    alice.send_dm(bob_root, "message 0".into()).await.expect("alice writes to bob");
    eventually("the first message is deposited", || seeder.inbox_stats().envelopes >= 2).await;
    fill_hourly_limit(&alice, &seeder, bob_root);
    alice.send_dm(bob_root, "message 1".into()).await.expect("alice writes to bob");
    eventually("the second message is refused", || {
        sent(&alice, bob_root).last().is_some_and(|m| m.delivery == "failed")
    })
    .await;
    let refused = sent(&alice, bob_root).pop().expect("a message was sent");
    let refused_id = parse_message_id(&refused.id).expect("the chat's id parses");
    let held = seeder.inbox_stats().envelopes;
    let old_ciphertext = {
        let w = alice.read();
        let entry = w.refused.packets.get(&refused_id).expect("the refused packet is kept");
        match &entry.item.packet {
            ConvPacket::Message { ciphertext, .. } => ciphertext.clone(),
            _ => panic!("the refused packet is a message"),
        }
    };

    // Past the envelope lifetime the message stays failed with its reason, and it cannot be sent again.
    alice
        .write()
        .refused
        .packets
        .get_mut(&refused_id)
        .expect("the refused packet is kept")
        .refused_ms = now().saturating_sub(REFUSED_TTL_MS + 1);
    let expired = sent(&alice, bob_root).pop().expect("a message was sent");
    assert_eq!(expired.delivery, "failed");
    assert!(expired.reason.is_some(), "the reason is still shown");
    assert!(!expired.retryable, "an expired message has no retry");
    assert!(alice.retry_message(&refused.id).await.is_err(), "an expired message cannot be sent again");

    // Back inside the lifetime, it can, and the inbox takes it because it restarted: its hourly count is not stored.
    alice
        .write()
        .refused
        .packets
        .get_mut(&refused_id)
        .expect("the refused packet is kept")
        .refused_ms = now();
    seeder.shutdown().await.expect("seeder shuts down");
    let seeder = start(seeder_id.clone(), seeder_dir.clone()).await;
    introduce(&alice, &seeder);

    alice.retry_message(&refused.id).await.expect("alice retries the refused message");
    eventually("the retried message is deposited", || {
        sent(&alice, bob_root)
            .last()
            .is_some_and(|m| m.id == refused.id && m.delivery == "queued_inbox")
    })
    .await;
    assert_eq!(seeder.inbox_stats().envelopes, held + 1, "the inbox holds the retried packet");

    // The retry is the same message under new ciphertext: it was sealed again under the current epoch.
    let resent: Vec<ConvPacket> = seeder
        .read()
        .inbox
        .envelopes
        .iter()
        .filter_map(|e| decode::<ConvPacket>(&e.packet).ok())
        .filter(|p| matches!(p, ConvPacket::Message { id, .. } if *id == refused_id))
        .collect();
    assert_eq!(resent.len(), 1, "one copy of the message is at the inbox");
    match &resent[0] {
        ConvPacket::Message { ciphertext, .. } => {
            assert_ne!(ciphertext, &old_ciphertext, "the retry is sealed anew, not resent as it was");
        }
        _ => unreachable!("filtered to messages"),
    }

    // Bob comes back, collects, and reads the message that was refused.
    let bob = start(bob_id.clone(), bob_dir.clone()).await;
    introduce(&bob, &seeder);
    bob.collect_inboxes().await.expect("bob collects");
    eventually("bob reads the retried message", || {
        bob.dm_messages(alice_root, 200).iter().any(|m| m.text == "message 1")
    })
    .await;

    alice.shutdown().await.expect("alice shuts down");
    bob.shutdown().await.expect("bob shuts down");
    seeder.shutdown().await.expect("seeder shuts down");
}

/// A deposit for `root` made by `from`, as `deposit_packet` would make it.
fn deposit_for(from: &Engine, root: RootId, tag: &str) -> Envelope {
    let me = from.inner.identity.clone();
    let packet = tag.as_bytes().to_vec();
    let id = *blake3::hash(&packet).as_bytes();
    let recipient = DeviceKey::generate().id();
    let expires_ms = now() + MAX_AGE_MS / 2;
    let sig = sign_over(&me.device, DEPOSIT_DOMAIN, &(recipient, root, id, expires_ms, &packet)).expect("deposit signs");
    Envelope {
        recipient,
        recipient_root: root,
        id,
        expires_ms,
        packet,
        from: me.cert.clone(),
        sig,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_friends_device_holds_mail_only_for_the_people_it_hosts() {
    let bob_id = Identity::create(now());
    let bob_root = bob_id.root_id();
    let bob = start(bob_id, scratch("host-bob")).await;
    let carol = start(Identity::create(now()), scratch("host-carol")).await;
    let dave_id = Identity::create(now());
    let dave_root = dave_id.root_id();
    let dave = start(dave_id, scratch("host-dave")).await;
    introduce(&bob, &carol);
    introduce(&dave, &carol);
    introduce(&carol, &bob);
    let carol_dev = carol.inner.identity.device_id();

    // Carol holds nothing for anyone yet. Bob may list her, but she answers that she does not serve him.
    bob.set_inboxes(vec![carol_dev]).await.expect("bob lists carol's device");
    let status = bob.inbox_status().await;
    assert_eq!(status.len(), 1);
    assert!(status[0].reachable && status[0].chosen, "carol answers");
    assert!(!status[0].serving, "carol does not hold bob's mail before she agrees");
    assert!(
        !matches!(carol.receive_deposit(deposit_for(&dave, bob_root, "early")), Response::Ack),
        "a device that hosts no one refuses deposits"
    );

    // Carol hosts Bob. His key packages and deposits for him are taken, and his status shows them.
    carol.host_inbox_for(bob_root, true).expect("carol hosts bob");
    bob.deposit_key_packages().await.expect("bob deposits key packages");
    assert_eq!(carol.inbox_stats().key_packages, KEY_PACKAGES_PER_DEVICE);
    assert!(matches!(carol.receive_deposit(deposit_for(&dave, bob_root, "for bob")), Response::Ack));
    let status = bob.inbox_status().await;
    assert!(status[0].serving, "carol now holds bob's mail");
    assert_eq!(status[0].envelopes, 1);
    assert_eq!(status[0].key_packages, KEY_PACKAGES_PER_DEVICE as u32);
    assert!(status[0].bytes > 0);
    let hosted = carol.hosted_inboxes();
    assert_eq!(hosted.len(), 1);
    assert_eq!(hosted[0].root, bob_root.to_string());
    assert_eq!(hosted[0].envelopes, 1);

    // Dave is not hosted: his deposits and key packages are refused, and his probe shows nothing,
    // not even that Carol holds mail for someone else.
    assert!(!matches!(carol.receive_deposit(deposit_for(&bob, dave_root, "for dave")), Response::Ack));
    dave.set_inboxes(vec![carol_dev]).await.expect("dave lists carol");
    assert_eq!(carol.inbox_stats().key_packages, KEY_PACKAGES_PER_DEVICE, "dave's key packages are refused");
    let status = dave.inbox_status().await;
    assert!(status[0].reachable && !status[0].serving);
    assert_eq!((status[0].envelopes, status[0].key_packages, status[0].bytes), (0, 0, 0));
    assert!(matches!(carol.take_key_package(dave_root), Response::Error(_)));

    // A probe signed by one device but naming another's cert is answered as from a stranger.
    let forged = {
        let cert = bob.inner.identity.cert.clone();
        let ts_ms = now();
        let sig = sign_over(&dave.inner.identity.device, PROBE_DOMAIN, &(cert.root, cert.device, ts_ms)).expect("signs");
        Probe { cert, ts_ms, sig }
    };
    assert!(matches!(carol.receive_probe(forged), Response::InboxProbe(a) if *a == ProbeAnswer::default()));

    // Carol stops: what she held for Bob is dropped, and his status shows she no longer serves him.
    carol.host_inbox_for(bob_root, false).expect("carol stops hosting bob");
    assert_eq!(carol.inbox_stats(), InboxStats::default());
    assert!(!bob.inbox_status().await[0].serving);

    // Hosting oneself goes through the inbox switch, not this list.
    assert!(carol.host_inbox_for(carol.inner.me, true).is_err());

    bob.shutdown().await.expect("bob shuts down");
    carol.shutdown().await.expect("carol shuts down");
    dave.shutdown().await.expect("dave shuts down");
}

#[tokio::test(flavor = "multi_thread")]
async fn hosted_people_survive_a_restart_and_are_capped() {
    let carol_id = Identity::create(now());
    let carol_dir = scratch("cap-carol");
    let carol = start(carol_id.clone(), carol_dir.clone()).await;
    let roots: Vec<RootId> = (0..MAX_HOSTED).map(|_| Identity::create(now()).root_id()).collect();
    for root in &roots {
        carol.host_inbox_for(*root, true).expect("within the cap");
    }
    carol.host_inbox_for(roots[0], true).expect("hosting someone again is not a new slot");
    assert!(carol.host_inbox_for(Identity::create(now()).root_id(), true).is_err(), "one past the cap is refused");
    carol.shutdown().await.expect("carol shuts down");

    let carol = start(carol_id, carol_dir).await;
    assert_eq!(carol.inbox_stats().hosted, MAX_HOSTED, "the hosted list is kept on disk");
    carol.shutdown().await.expect("carol shuts down");
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_do_not_show_whether_a_device_is_an_inbox() {
    // Carol holds mail for no one. Erin holds Bob's. Dave asks both for himself, and gets the same answers.
    let bob_root = Identity::create(now()).root_id();
    let carol = start(Identity::create(now()), scratch("same-carol")).await;
    let erin = start(Identity::create(now()), scratch("same-erin")).await;
    let dave_id = Identity::create(now());
    let dave_root = dave_id.root_id();
    let dave = start(dave_id, scratch("same-dave")).await;
    erin.host_inbox_for(bob_root, true).expect("erin hosts bob");

    let answers = |inbox: &Engine| {
        let deposit = format!("{:?}", inbox.receive_deposit(deposit_for(&dave, dave_root, "for dave")));
        let keys = {
            let cert = dave.inner.identity.cert.clone();
            let packages = vec![b"package".to_vec()];
            let sig = sign_over(&dave.inner.identity.device, KEYS_DOMAIN, &packages).expect("signs");
            format!("{:?}", inbox.receive_key_packages(DepositKeys { cert, packages, sig }))
        };
        let take = format!("{:?}", inbox.take_key_package(dave_root));
        let collect = {
            let device = dave.inner.identity.device_id();
            let ts_ms = now();
            let sig = sign_over(&dave.inner.identity.device, COLLECT_DOMAIN, &(device, ts_ms)).expect("signs");
            format!("{:?}", inbox.receive_collect(Collect { device, ts_ms, sig }))
        };
        (deposit, keys, take, collect)
    };
    let (none, other) = (answers(&carol), answers(&erin));
    assert_eq!(none, other, "an inbox for someone else answers as a device that is no inbox");
    assert!(none.0.contains(NOT_SERVED), "{}", none.0);
    assert_eq!(none.3, format!("{:?}", Response::Envelopes(Vec::new())), "a collect gets an empty list, not an error");
    // A served person whose packages ran out reads the same as an unserved one.
    assert_eq!(format!("{:?}", erin.take_key_package(bob_root)), none.2);

    carol.shutdown().await.expect("carol shuts down");
    erin.shutdown().await.expect("erin shuts down");
    dave.shutdown().await.expect("dave shuts down");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invisible_host_still_takes_mail_for_the_people_it_hosts() {
    let bob_root = Identity::create(now()).root_id();
    let alice_id = Identity::create(now());
    let alice_root = alice_id.root_id();
    let alice = start(alice_id, scratch("invisible-alice")).await;
    let carol = start(Identity::create(now()), scratch("invisible-carol")).await;
    introduce(&alice, &carol);
    carol.host_inbox_for(bob_root, true).expect("carol hosts bob");
    carol
        .set_status(crate::presence::Status::Invisible, None)
        .expect("carol goes invisible");
    let carol_dev = carol.inner.identity.device_id();
    let ask = |req: Request| {
        let bytes = alice.request_bytes(&req).expect("request encodes");
        let node = alice.node();
        async move { node.request(carol_dev, bytes).await.ok().and_then(|r| decode::<Response>(&r).ok()) }
    };

    // A deposit for Bob, whom Carol hosts, is answered: hosting is consent.
    let held = ask(Request::Deposit(Box::new(deposit_for(&alice, bob_root, "for bob")))).await;
    assert!(matches!(held, Some(Response::Ack)), "{held:?}");
    assert_eq!(carol.inbox_stats().envelopes, 1);
    // Anything else from a device Carol did not contact gets no answer, as if she were offline: a deposit
    // for someone she does not host, and a probe for Alice herself.
    let other = ask(Request::Deposit(Box::new(deposit_for(&alice, alice_root, "for alice")))).await;
    assert!(other.is_none(), "{other:?}");
    let probe = {
        let cert = alice.inner.identity.cert.clone();
        let ts_ms = now();
        let sig = sign_over(&alice.inner.identity.device, PROBE_DOMAIN, &(cert.root, cert.device, ts_ms)).expect("signs");
        ask(Request::InboxProbe(Box::new(Probe { cert, ts_ms, sig }))).await
    };
    assert!(probe.is_none(), "{probe:?}");

    alice.shutdown().await.expect("alice shuts down");
    carol.shutdown().await.expect("carol shuts down");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_tells_the_person_when_it_starts_and_stops_holding_their_mail() {
    let bob_id = Identity::create(now());
    let bob_root = bob_id.root_id();
    let bob_dir = scratch("notice-bob");
    let bob = start(bob_id.clone(), bob_dir.clone()).await;
    let carol = start(Identity::create(now()), scratch("notice-carol")).await;
    introduce(&carol, &bob);
    introduce(&bob, &carol);
    let carol_root = carol.inner.me;
    let carol_dev = carol.inner.identity.device_id().to_string();
    let mut events = bob.subscribe();

    carol.host_inbox_for(bob_root, true).expect("carol hosts bob");
    eventually("bob hears carol holds his mail", || {
        bob.inbox_notices().first().is_some_and(|n| n.on && n.device == carol_dev && n.root == carol_root.to_string())
    })
    .await;
    let mut seen = None;
    while let Ok(ev) = events.try_recv() {
        if let EngineEvent::InboxHostChanged { device_id, on, .. } = ev {
            seen = Some((device_id, on));
        }
    }
    assert_eq!(seen, Some((carol_dev.clone(), true)), "the app is told, for an in-app notice");
    assert!(!bob.inbox_notices()[0].chosen, "bob has not picked it yet");

    // Setting what is already set sends nothing.
    carol.host_inbox_for(bob_root, true).expect("again");
    let queued = |e: &Engine| {
        e.read()
            .contact_book
            .outbox
            .iter()
            .filter(|o| matches!(o.msg.msg, ContactMsg::InboxHost { .. }))
            .count()
    };
    assert_eq!(queued(&carol), 0, "no notice for a change that is none");

    carol.host_inbox_for(bob_root, false).expect("carol stops");
    eventually("bob hears carol stopped", || bob.inbox_notices().first().is_some_and(|n| !n.on)).await;
    assert_eq!(bob.inbox_notices().len(), 1, "a device's newer notice replaces its older one");

    // A notice is kept across a restart, so the Offline delivery page still shows it.
    bob.shutdown().await.expect("bob shuts down");
    let bob = start(bob_id, bob_dir).await;
    assert!(bob.inbox_notices().first().is_some_and(|n| !n.on && n.device == carol_dev));

    bob.shutdown().await.expect("bob shuts down");
    carol.shutdown().await.expect("carol shuts down");
}
