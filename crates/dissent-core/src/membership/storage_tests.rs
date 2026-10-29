//! Storage-only seeders on loopback. A storage member keeps and serves the community's history,
//! but holds no MLS group state, so it cannot read messages, and it cannot post.


use super::tests::{eventually, general, Node};
use crate::CommunityId;


#[tokio::test(flavor = "multi_thread")]
async fn storage_seeder_serves_history_and_is_never_in_the_group() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Stored".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let channel = general(&alice.engine, id);
        let invite = alice.engine.create_invite(id).await.expect("invite");

        // The seeder joins as storage: it syncs the history but is never added to the group.
        let seeder = Node::named("seeder").await;
        seeder
            .engine
            .join_as_storage(&invite)
            .await
            .expect("seeder joins as storage");
        for text in ["one", "two", "three"] {
            alice
                .engine
                .send_message(id, channel, text.into())
                .await
                .expect("alice posts");
        }
        eventually("the seeder holds the history", || {
            seeder.engine.channel_messages(id, channel, 100).len() == 3
        })
        .await;

        // It cannot read any of it: the text is locked, and it has no group state.
        assert!(seeder
            .engine
            .channel_messages(id, channel, 100)
            .iter()
            .all(|m| m.locked && m.text.is_empty()));
        assert_eq!(seeder.engine.mls_epoch(id), None);

        // It cannot post, either.
        assert!(seeder
            .engine
            .send_message(id, channel, "from storage".into())
            .await
            .is_err());

        // The owner goes offline. A later joiner gets the history from the seeder, still locked.
        let (alice_id, alice_dir, alice_port) = alice.stop().await;
        let seeder_invite = seeder.engine.create_invite(id).await.expect("storage invites");
        let carol = Node::named("carol").await;
        carol
            .engine
            .join_community(&seeder_invite)
            .await
            .expect("carol joins through the seeder");
        eventually("carol holds the history", || {
            carol.engine.channel_messages(id, channel, 100).len() == 3
        })
        .await;
        assert!(carol
            .engine
            .channel_messages(id, channel, 100)
            .iter()
            .all(|m| m.locked));
        assert_eq!(carol.engine.mls_epoch(id), None, "no person was online to add her");

        // A person comes back. Carol is added to the group, and reads what is posted from now on.
        let alice = Node::boot(alice_id, alice_dir, alice_port).await;
        eventually("alice knows carol", || {
            alice.engine.community_view(id).map(|v| v.members.len()).ok() == Some(3)
        })
        .await;
        carol.engine.mls_join_missing(id).await;
        eventually("carol is in the group", || carol.engine.mls_epoch(id).is_some()).await;
        alice
            .engine
            .send_message(id, channel, "after carol".into())
            .await
            .expect("alice posts");
        eventually("carol reads it", || {
            carol
                .engine
                .channel_messages(id, channel, 100)
                .iter()
                .any(|m| m.text == "after carol" && !m.locked)
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seeder_refuses_member_sealed_events_without_a_valid_tag() {
    use dissent_events::{ChannelId, Event, EventBody, Hlc, SignedEvent};

    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Tagged".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let seeder = Node::named("seeder").await;
        seeder
            .engine
            .join_as_storage(&invite)
            .await
            .expect("seeder joins as storage");
        eventually("the seeder sees member-only control", || {
            seeder
                .engine
                .community_view(id)
                .ok()
                .is_some_and(|v| v.member_only)
        })
        .await;

        // A channel made under the switch carries a valid tag, so the seeder keeps it.
        let channel = alice
            .engine
            .create_channel(id, "tagged".into())
            .await
            .expect("channel");
        let channel_id: ChannelId = channel.id.parse().unwrap();
        eventually("the seeder keeps the event with a valid tag", || {
            let c = seeder.engine.read();
            c.communities
                .get(&id)
                .is_some_and(|c| !c.signed_for(&[dissent_events::EventId(channel_id.0)]).is_empty())
        })
        .await;

        // A forgery: alice's device signs a member-sealed event whose tag is junk. The seeder holds
        // the write key, but cannot make the tag match this ciphertext, so it must not keep the event.
        let heads = alice.engine.read().communities[&id].heads();
        let body = EventBody {
            community: id,
            channel: ChannelId::CONTROL,
            author: alice.identity.root_id(),
            device: alice.identity.device_id(),
            parents: heads,
            hlc: Hlc {
                wall_ms: crate::engine::now_ms(),
                counter: 0,
            },
            payload: dissent_events::member_sealed_payload(0, vec![7; 64], [0; 32]).unwrap(),
        };
        let forged = Event::sign(body, &alice.identity.device);
        let forged_id = forged.id;
        let se = SignedEvent {
            event: forged,
            cert: alice.identity.cert.clone(),
        };
        seeder
            .engine
            .ingest(id, vec![se])
            .expect("the batch is handled, junk is dropped");
        let kept = seeder.engine.read().communities[&id].signed_for(&[forged_id]);
        assert!(kept.is_empty(), "the seeder refuses an event with a bad tag");
        assert!(seeder.engine.mls_epoch(id).is_none(), "the seeder is never in the group");
    })
    .await;
}

/// A seeder is named by its storage join everywhere a name is shown: the member list, a profile, a
/// mention. All of them read `World::display_name`.
#[tokio::test(flavor = "multi_thread")]
async fn a_seeder_is_shown_by_the_name_it_joined_with() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Named".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let seeder = Node::named("Home seeder").await;
        seeder.engine.join_as_storage(&invite).await.expect("seeder joins as storage");
        let seeder_root = seeder.engine.me().root_id;
        eventually("alice lists the seeder by its name", || {
            alice.engine.community_view(id).is_ok_and(|v| {
                v.members.iter().any(|m| m.root_id == seeder_root && m.display_name == "Home seeder")
            })
        })
        .await;
        let root: crate::RootId = seeder_root.parse().unwrap();
        assert_eq!(alice.engine.read().display_name(&root), "Home seeder", "the shared name lookup");
    })
    .await;
}

/// Storage grants, phase 1: a seeder asks the owner for a grant when it joins, the owner sees the ask
/// and grants it, and both see the grant. A person's ask is refused. See `storage_grants`.
#[tokio::test(flavor = "multi_thread")]
async fn a_seeder_asks_for_a_storage_grant_and_the_owner_grants_it() {
    crate::testutil::bounded(async {
        let alice = Node::named("alice").await;
        let id: CommunityId = alice
            .engine
            .create_community("Granted".into())
            .await
            .expect("create")
            .id
            .parse()
            .unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let mut events = alice.engine.subscribe();
        let seeder = Node::named("seeder").await;
        seeder.engine.join_as_storage(&invite).await.expect("seeder joins as storage");
        let seeder_root: crate::RootId = seeder.engine.me().root_id.parse().unwrap();
        eventually("the owner holds the seeder's ask", || {
            alice.engine.storage_grant_asks(id) == vec![seeder_root.to_string()]
        })
        .await;
        // The seeder asks again after 5 s. The owner is told once per ask, not once per message.
        tokio::time::sleep(std::time::Duration::from_secs(7)).await;
        let mut told = Vec::new();
        while let Ok(ev) = events.try_recv() {
            if let crate::EngineEvent::StorageGrantAsked { root_id, display_name, .. } = ev {
                told.push((root_id, display_name));
            }
        }
        assert_eq!(told, vec![(seeder_root.to_string(), "seeder".to_string())], "one notice for the ask");
        assert!(!alice.engine.storage_granted(id, &seeder_root), "no grant yet: phase 1 does not need one");

        alice.engine.grant_storage(id, seeder_root, true).await.expect("the owner grants storage");
        eventually("the seeder sees its grant", || seeder.engine.storage_granted(id, &seeder_root)).await;
        assert!(alice.engine.storage_grant_asks(id).is_empty(), "a granted ask is no longer pending");
        let listed = alice.engine.community_view(id).expect("view");
        assert!(listed
            .members
            .iter()
            .any(|m| m.root_id == seeder_root.to_string() && m.storage && m.storage_granted));

        // A person is not a seeder, so its ask is refused.
        let bob = Node::named("bob").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("alice sees bob", || {
            alice.engine.community_view(id).is_ok_and(|v| v.members.iter().any(|m| m.display_name == "bob"))
        })
        .await;
        let reply = bob
            .engine
            .call(alice.engine.inner.identity.device_id(), &crate::rpc::Request::StorageGrantAsk { community: id })
            .await
            .expect("delivered");
        assert!(matches!(reply, crate::rpc::Response::Error(_)), "a person's ask is refused: {reply:?}");

        // A withdrawn grant reads as no grant.
        alice.engine.grant_storage(id, seeder_root, false).await.expect("withdraw");
        eventually("the grant is withdrawn", || !seeder.engine.storage_granted(id, &seeder_root)).await;
    })
    .await;
}
