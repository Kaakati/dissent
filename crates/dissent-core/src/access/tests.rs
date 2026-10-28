//! The access gate on loopback: an invite with the wrong secret cannot join, and tokens bind a
//! request to its community.

use dissent_discovery::Invite;
use dissent_identity::CommunityId;

use crate::membership::tests::Node;

#[tokio::test(flavor = "multi_thread")]
async fn an_invite_with_the_wrong_secret_cannot_join() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Gated".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let honest = alice.engine.create_invite(id).await.expect("invite");

        // The same genesis and peers, with a secret the founder never issued.
        let mut forged = Invite::parse(&honest).expect("parse");
        forged.secret = [9; 32];
        let mallory = Node::named("mallory").await;
        let refused = mallory.engine.join_community(forged.to_url().as_str()).await;
        assert!(refused.is_err(), "a join with the wrong secret must fail");
        assert!(
            mallory.engine.community_view(id).is_err(),
            "mallory must not hold the community"
        );

        // The real invite still works.
        mallory
            .engine
            .join_community(&honest)
            .await
            .expect("the honest invite joins");
    })
    .await;
}

#[test]
fn retired_secrets_stop_working_after_the_grace_period() {
    use super::{community_token, Keys, GRACE_MS};
    let id = CommunityId([7; 32]);
    let mut keys = Keys::from_invite(0, [1; 32]);
    assert!(keys.install(1, [2; 32], [9; 32], 1_000));
    let old = Some(community_token(&[1; 32], &id));
    assert!(keys.accepts(&id, old, 1_001), "the retired secret works in the grace period");
    assert!(
        !keys.accepts(&id, old, 1_000 + GRACE_MS + 1),
        "and stops after it"
    );
    assert!(!keys.accepts(&id, None, 1_001), "no token is never accepted");
    let current = Some(community_token(&[2; 32], &id));
    assert!(keys.accepts(&id, current, u64::MAX), "the current secret always works");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_member_is_refused_once_the_secret_rotates() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members};
        use crate::ModAction;
        let (alice, bob, carol, id, _invite) = three_members().await;
        assert_eq!(alice.engine.access_keys(id).map(|k| k.generation), Some(0));

        alice
            .engine
            .moderate(id, ModAction::Ban, carol.root())
            .await
            .expect("alice bans carol");

        // Alice and bob, still in the group, learn the new secret from the DAG.
        eventually("alice holds the rotated secret", || {
            alice.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;
        eventually("bob holds the rotated secret", || {
            bob.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;

        // Carol keeps the old invite and secret, and cannot sync with the members.
        assert!(
            carol
                .engine
                .sync_with(id, alice.identity.device_id())
                .await
                .is_err(),
            "a banned member's sync must be refused"
        );
        assert_eq!(carol.engine.access_keys(id).map(|k| k.generation), Some(0));

        // Carol stays on the old topic: a post on the new one never reaches her.
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "after the ban".into())
            .await
            .expect("alice posts");
        eventually("bob reads it", || readable(&bob.engine, id, channel, "after the ban")).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!readable(&carol.engine, id, channel, "after the ban"));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_who_leaves_is_refused_once_the_secret_rotates() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members};
        let (alice, bob, carol, id, _invite) = three_members().await;

        carol.engine.leave_community(id).await.expect("carol leaves");

        // Alice, an admin, rotates for the departure once the removal commit has applied. Bob learns it from the DAG.
        eventually("alice holds the rotated secret", || {
            alice.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;
        eventually("bob holds the rotated secret", || {
            bob.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;
        assert_eq!(carol.engine.access_keys(id).map(|k| k.generation), Some(0));

        // Carol keeps the old secret, so she cannot follow the members to the new topic or the new token.
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "after the leave".into())
            .await
            .expect("alice posts");
        eventually("bob reads it", || readable(&bob.engine, id, channel, "after the leave")).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!readable(&carol.engine, id, channel, "after the leave"));

        // A control event after the rotation is sealed under the new secret. Carol's old one does not open it.
        alice.engine.rename_community(id, "After".into()).await.expect("alice renames");
        let rename = latest_rename(&alice, id);
        let old = carol.engine.access_keys(id).expect("carol kept her secret");
        assert!(
            dissent_events::control::open(&[(old.generation, old.secret)], &rename.event.body).is_err(),
            "the leaver's secret must not open a later control event"
        );
        let new = alice.engine.access_keys(id).expect("alice's secret");
        assert!(
            dissent_events::control::open(&[(new.generation, new.secret)], &rename.event.body).is_ok(),
            "the rotated secret opens it"
        );
        eventually("bob sees the rename", || name_of(&bob, id) == "After").await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leave_does_not_rotate_while_a_member_device_does_not_accept_it() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members};
        use std::sync::atomic::Ordering;
        let (alice, bob, carol, id, _invite) = three_members().await;
        // Bob models a released client, which would reject a rotation that covers a leaver.
        bob.engine.inner.rotation_support.store(false, Ordering::Relaxed);

        carol.engine.leave_community(id).await.expect("carol leaves");
        eventually("alice sees carol leave", || {
            alice.engine.community_view(id).map(|v| v.members.len()).ok() == Some(2)
        })
        .await;
        // The departure's own retries, then a maintenance pass: still no rotation.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        alice.engine.mls_reconcile(id).await;
        alice.engine.rotate_if_due(id).await;
        assert_eq!(alice.engine.access_keys(id).map(|k| k.generation), Some(0));
        assert_eq!(bob.engine.access_keys(id).map(|k| k.generation), Some(0));

        // Bob keeps access: he reads alice's message and her control events.
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "still here".into())
            .await
            .expect("alice posts");
        eventually("bob reads it", || readable(&bob.engine, id, channel, "still here")).await;
        alice.engine.rename_community(id, "Still".into()).await.expect("alice renames");
        eventually("bob sees the rename", || name_of(&bob, id) == "Still").await;

        // Once bob accepts it, the next pass rotates for carol, and bob follows.
        bob.engine.inner.rotation_support.store(true, Ordering::Relaxed);
        alice.engine.rotate_if_due(id).await;
        assert_eq!(alice.engine.access_keys(id).map(|k| k.generation), Some(1));
        eventually("bob holds the rotated secret", || {
            bob.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;
    })
    .await;
}

/// The rename that set `id`'s current name, as `node` stores it.
fn latest_rename(node: &Node, id: CommunityId) -> dissent_events::SignedEvent {
    let w = node.engine.read();
    w.communities.get(&id).and_then(|c| c.latest_rename()).expect("a stored rename")
}

fn name_of(node: &Node, id: CommunityId) -> String {
    node.engine.community_view(id).map(|v| v.name).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_offline_during_a_ban_catches_up_with_the_rotated_secret() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members, Node};
        use crate::ModAction;
        let (alice, bob, carol, id, _invite) = three_members().await;
        let bob_root = bob.root();

        // Carol goes offline with only the old secret, and comes back after the ban.
        let (carol_id, carol_dir, carol_port) = carol.stop().await;
        alice
            .engine
            .moderate(id, ModAction::Ban, bob_root)
            .await
            .expect("alice bans bob");
        eventually("alice holds the rotated secret", || {
            alice.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;

        let carol = Node::boot(carol_id, carol_dir, carol_port).await;
        assert_eq!(carol.engine.access_keys(id).map(|k| k.generation), Some(0));
        eventually("carol learns the rotated secret", || {
            carol.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;

        // Carol is on the new topic now, so a new post reaches her.
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "welcome back".into())
            .await
            .expect("alice posts");
        eventually("carol reads it", || readable(&carol.engine, id, channel, "welcome back")).await;

    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invite_from_before_a_ban_admits_a_new_member_during_the_grace_period() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members, Node};
        use crate::ModAction;
        let (alice, bob, _carol, id, old_invite) = three_members().await;
        alice
            .engine
            .moderate(id, ModAction::Ban, bob.root())
            .await
            .expect("alice bans bob");
        eventually("alice holds the rotated secret", || {
            alice.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;

        // Dave is not banned. He joins with the invite that predates the ban.
        let dave = Node::named("dave").await;
        dave.engine
            .join_community(&old_invite)
            .await
            .expect("dave joins with the old invite");
        eventually("dave learns the rotated secret", || {
            dave.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "hello dave".into())
            .await
            .expect("alice posts");
        eventually("dave reads it", || readable(&dave.engine, id, channel, "hello dave")).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_admins_rotating_at_once_converge_on_one_secret() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, general, readable, three_members};
        use crate::ModAction;
        use dissent_events::{ChannelId, Event, EventBody, Hlc, Payload, SignedEvent};
        let (alice, bob, carol, id, _invite) = three_members().await;
        alice
            .engine
            .moderate(id, ModAction::GrantAdmin, bob.root())
            .await
            .expect("alice makes bob an admin");
        eventually("bob is an admin everywhere", || {
            alice
                .engine
                .community_view(id)
                .ok()
                .and_then(|v| {
                    v.members
                        .iter()
                        .find(|m| m.root_id == bob.root().to_string())
                        .map(|m| m.role == "admin")
                })
                == Some(true)
        })
        .await;

        // Ban carol by a bare event, so no automatic rotation runs and the two below are the only ones.
        alice
            .engine
            .author_publish(id, ChannelId::CONTROL, Payload::Ban { user: carol.root() })
            .await
            .expect("ban");
        eventually("bob sees the ban", || {
            bob.engine
                .community_view(id)
                .ok()
                .is_some_and(|v| v.members.iter().any(|m| m.root_id == carol.root().to_string() && m.banned))
        })
        .await;

        // Both admins rotate from the same parents, neither having seen the other.
        let heads = alice.engine.read().communities[&id].heads();
        let mk = |node: &crate::membership::tests::Node, secret: [u8; 32]| -> SignedEvent {
            let sealed = node
                .engine
                .mls_encrypt(&id.0, &secret)
                .expect("seal");
            let body = EventBody {
                community: id,
                channel: ChannelId::CONTROL,
                author: node.root(),
                device: node.identity.device_id(),
                parents: heads.clone(),
                hlc: Hlc {
                    wall_ms: crate::engine::now_ms(),
                    counter: 0,
                },
                payload: Payload::SecretRotation {
                    generation: 1,
                    covers: vec![carol.root()],
                    sealed,
                },
            };
            SignedEvent {
                event: Event::sign(body, &node.identity.device),
                cert: node.identity.cert.clone(),
            }
        };
        let (secret_a, secret_b) = ([0xa1; 32], [0xb2; 32]);
        let ra = mk(&alice, secret_a);
        let rb = mk(&bob, secret_b);
        let (winner, winner_secret) = if ra.event.id < rb.event.id {
            (ra.event.id, secret_a)
        } else {
            (rb.event.id, secret_b)
        };

        // Each admin sees the other's rotation, in opposite orders.
        alice
            .engine
            .ingest(id, vec![ra.clone(), rb.clone()])
            .expect("alice ingests");
        bob.engine
            .ingest(id, vec![rb.clone(), ra.clone()])
            .expect("bob ingests");

        for node in [&alice, &bob] {
            eventually("the member holds the winning secret", || {
                node.engine
                    .access_keys(id)
                    .is_some_and(|k| k.secret == winner_secret && k.source == winner.0)
            })
            .await;
        }
        assert_eq!(
            alice.engine.access_keys(id).map(|k| k.secret),
            bob.engine.access_keys(id).map(|k| k.secret),
            "one secret everywhere"
        );

        // They share a topic now, so messages flow both ways.
        let channel = general(&alice.engine, id);
        alice
            .engine
            .send_message(id, channel, "from alice".into())
            .await
            .expect("alice posts");
        eventually("bob reads alice", || readable(&bob.engine, id, channel, "from alice")).await;
        bob.engine
            .send_message(id, channel, "from bob".into())
            .await
            .expect("bob posts");
        eventually("alice reads bob", || readable(&alice.engine, id, channel, "from bob")).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_who_joins_after_a_ban_reads_the_control_events_from_before_it() {
    crate::testutil::bounded(async {
        use crate::membership::tests::{eventually, three_members, Node};
        use crate::ModAction;
        // The "general" channel was created under generation 0, before the ban rotates to 1.
        let (alice, bob, _carol, id, _old_invite) = three_members().await;
        alice
            .engine
            .moderate(id, ModAction::Ban, bob.root())
            .await
            .expect("alice bans bob");
        eventually("alice holds the rotated secret", || {
            alice.engine.access_keys(id).map(|k| k.generation) == Some(1)
        })
        .await;

        // Dave joins with an invite from after the ban, so he starts with generation 1 only.
        let fresh = alice.engine.create_invite(id).await.expect("invite");
        let dave = Node::named("dave").await;
        dave.engine.join_community(&fresh).await.expect("dave joins");
        eventually("dave fetches the older secret", || {
            dave.engine.access_keys(id).is_some_and(|k| k.history.len() == 2)
        })
        .await;
        eventually("dave replays the channel sealed before the ban", || {
            dave.engine
                .community_view(id)
                .is_ok_and(|v| v.channels.iter().any(|c| c.name == "general"))
        })
        .await;
    })
    .await;
}

/// A rotation that arrives while its generation is unknown is locked. Its notice must come when
/// the key arrives, and only then, however often the key or the event is seen again.
#[tokio::test(flavor = "multi_thread")]
async fn a_locked_key_rotation_raises_its_notice_once_when_the_key_arrives() {
    crate::testutil::bounded(async {
        use crate::engine::now_ms;
        use crate::membership::tests::Node;
        use crate::views::EngineEvent;
        use dissent_events::{seal, ChannelId, Event, EventBody, Hlc, Payload, SignedEvent};
        use dissent_identity::{DeviceKey, KeyRotation, RootKey};

        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Rotating".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let mut events = alice.engine.subscribe();
        let notices = |events: &mut tokio::sync::broadcast::Receiver<EngineEvent>| {
            let mut n = 0;
            while let Ok(ev) = events.try_recv() {
                n += usize::from(matches!(ev, EngineEvent::IdentityChanged { .. }));
            }
            n
        };

        // Alice's root moves to a fresh root. The rotation is sealed under generation 7, which
        // alice does not hold yet.
        let now = now_ms();
        let old = alice.identity.root.as_ref().expect("alice holds her root key");
        let new = RootKey::generate();
        let rotation = KeyRotation::by_owner(old, &new, now);
        let device = DeviceKey::generate();
        let cert = new.certify(device.id(), 0);
        let body = EventBody {
            community: id,
            channel: ChannelId::CONTROL,
            author: new.id(),
            device: device.id(),
            parents: vec![],
            hlc: Hlc { wall_ms: now, counter: 0 },
            payload: Payload::KeyRotation(rotation),
        };
        let secret = [42u8; 32];
        let payload = seal(&secret, 7, &body).unwrap();
        let se = SignedEvent {
            event: Event::sign(EventBody { payload, ..body }, &device),
            cert,
        };

        alice.engine.ingest(id, vec![se.clone()]).expect("the locked rotation is stored");
        assert_eq!(notices(&mut events), 0, "locked: no notice yet");

        // The key arrives. The rotation unlocks and its notice fires once.
        alice.engine.remember_secrets(id, &[(7, secret)]).await;
        // The unlock can emit just after `remember_secrets` returns, so wait for it briefly.
        let mut seen = notices(&mut events);
        for _ in 0..500 {
            if seen > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            seen += notices(&mut events);
        }
        assert_eq!(seen, 1, "the notice fires when the key arrives");

        // Seeing the key again, or the event again, raises nothing more.
        alice.engine.remember_secrets(id, &[(7, secret)]).await;
        alice.engine.ingest(id, vec![se]).expect("duplicate is ignored");
        assert_eq!(notices(&mut events), 0, "no duplicate notice");
    })
    .await;
}

#[test]
fn keys_remember_every_generation_they_install() {
    use super::Keys;
    let mut keys = Keys::from_invite(0, [1; 32]);
    assert!(keys.install(1, [2; 32], [9; 32], 1_000));
    assert_eq!(keys.history, vec![(0, [1; 32]), (1, [2; 32])]);
    keys.remember(1, [2; 32]);
    assert_eq!(keys.history.len(), 2, "a pair is kept once");
}

/// Upgrades must never fail to start: every format an older build wrote still loads.
#[test]
fn access_files_from_older_versions_still_load() {
    use super::{decode_keys, Keys, Retired};

    // v0.1.1: the bare invite secret.
    let v1 = postcard::to_allocvec(&[7u8; 32]).unwrap();
    assert_eq!(decode_keys(&v1).unwrap(), Keys::from_invite(0, [7; 32]));

    // Before the rotation tie-break: Keys without `source`.
    #[derive(serde::Serialize)]
    struct KeysV2 {
        generation: u64,
        secret: [u8; 32],
        retired: Vec<Retired>,
    }
    let retired = vec![Retired { secret: [1; 32], until_ms: 99 }];
    let v2 = postcard::to_allocvec(&KeysV2 { generation: 3, secret: [8; 32], retired: retired.clone() }).unwrap();
    let keys = decode_keys(&v2).unwrap();
    assert_eq!((keys.generation, keys.secret, keys.retired, keys.source), (3, [8; 32], retired, [0; 32]));

    // Before sealed control events: Keys without `history`. The history is rebuilt from the
    // current secret, which is the only one such a file could have used.
    #[derive(serde::Serialize)]
    struct KeysV3 {
        generation: u64,
        secret: [u8; 32],
        retired: Vec<Retired>,
        source: [u8; 32],
    }
    let v3 = postcard::to_allocvec(&KeysV3 { generation: 4, secret: [6; 32], retired: vec![Retired { secret: [1; 32], until_ms: 99 }], source: [5; 32] }).unwrap();
    let keys = decode_keys(&v3).unwrap();
    assert_eq!((keys.generation, keys.secret, keys.source), (4, [6; 32], [5; 32]));
    assert_eq!(keys.history, vec![(4, [6; 32])]);

    // The current format round-trips unchanged.
    let mut now = Keys::from_invite(5, [9; 32]);
    now.source = [4; 32];
    now.remember(3, [7; 32]);
    assert_eq!(decode_keys(&postcard::to_allocvec(&now).unwrap()).unwrap(), now);

    // Garbage is an error, not a panic.
    assert!(decode_keys(&[1, 2, 3]).is_err());
}
