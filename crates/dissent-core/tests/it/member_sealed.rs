//! Member-only control: what a storage seeder, a banned member and a later joiner can read. See
//! `docs/design/05-encrypted-servers.md` and `docs/design/06-threat-model.md`, finding 2.
//! Networked, so run through `scripts/fwtest.sh dissent-core it --ignored member_sealed`.

use crate::common::*;
use dissent_core::{ChannelId, CommunityId};
use dissent_events::HistorySetting;
use std::time::Duration;

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn has_channel(engine: &dissent_core::Engine, id: CommunityId, name: &str) -> bool {
    engine
        .community_view(id)
        .ok()
        .is_some_and(|v| v.channels.iter().any(|c| c.name == name))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn storage_seeder_stores_member_sealed_events_without_names() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("owner turns member-only control on");
        eventually("bob sees member-only control", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.member_only)
                .map(|_| ())
        })
        .await;

        // The seeder joins as storage, so it holds the community secret but no MLS group.
        let seeder = peer("seeder").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        seeder
            .engine
            .join_as_storage(&invite)
            .await
            .expect("seeder joins as storage");

        // Carol joins after the switch, and a channel is made after it.
        let carol = peer("carol-quiet").await;
        let carol_invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &carol_invite).await;
        let channel = alice
            .engine
            .create_channel(id, "secret-room".into())
            .await
            .expect("channel");
        let channel_id: ChannelId = channel.id.parse().expect("channel id");

        // The seeder stores the channel's event. Its log holds the event id, and the sealed text.
        let log = seeder
            .dir
            .join("communities")
            .join(id.to_string())
            .join("events.log");
        eventually("the seeder stores the channel event", || {
            let bytes = std::fs::read(&log).ok()?;
            contains(&bytes, &channel_id.0).then_some(())
        })
        .await;

        let bytes = std::fs::read(&log).expect("seeder log");
        assert!(
            !contains(&bytes, b"secret-room"),
            "the seeder's log holds no channel name"
        );
        assert!(
            !contains(&bytes, b"carol-quiet"),
            "the seeder's log holds no name of a member who joined after the switch"
        );
        let view = format!("{:?}", seeder.engine.community_view(id).expect("seeder view"));
        assert!(!view.contains("secret-room"), "the seeder names no channel");
        assert!(!view.contains("carol-quiet"), "the seeder names no member");

        finish(vec![alice, bob, carol, seeder]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn banned_member_cannot_read_control_after_the_ban() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let carol = peer("carol").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        eventually("carol sees member-only control", || {
            carol
                .engine
                .community_view(id)
                .ok()
                .filter(|v| v.member_only)
                .map(|_| ())
        })
        .await;

        alice
            .engine
            .moderate(id, dissent_core::ModAction::Ban, bob.identity.root_id())
            .await
            .expect("alice bans bob");
        // The removal commit moves the epoch. A channel made after the ban is in the new epoch.
        alice
            .engine
            .create_channel(id, "after-ban".into())
            .await
            .expect("channel after the ban");
        eventually("carol sees the channel made after the ban", || {
            has_channel(&carol.engine, id, "after-ban").then_some(())
        })
        .await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !has_channel(&bob.engine, id, "after-ban"),
            "a banned member cannot read control events made after the ban"
        );

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn joiner_under_since_join_sees_no_older_control_and_all_sees_everything() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        alice
            .engine
            .create_channel(id, "before-dave".into())
            .await
            .expect("channel before the joiner");
        alice
            .engine
            .set_history_visibility(id, HistorySetting::SinceJoin)
            .await
            .expect("history since join");

        let dave = peer("dave").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &dave, &invite).await;
        eventually("dave sees that history is locked", || {
            dave.engine
                .community_view(id)
                .ok()
                .filter(|v| v.history_locked)
                .map(|_| ())
        })
        .await;
        assert!(
            !has_channel(&dave.engine, id, "before-dave"),
            "a joiner under since-join sees no older control content"
        );
        alice
            .engine
            .create_channel(id, "after-dave".into())
            .await
            .expect("channel after the joiner");
        eventually("dave sees the channel made after it joined", || {
            has_channel(&dave.engine, id, "after-dave").then_some(())
        })
        .await;

        // Under `all`, a joiner gets every epoch's key, so it reads what came before it.
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("history of all");
        let eve = peer("eve").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &eve, &invite).await;
        eventually("eve sees the channel made before dave", || {
            has_channel(&eve.engine, id, "before-dave").then_some(())
        })
        .await;
        assert!(has_channel(&eve.engine, id, "after-dave"));
        // The keys arrive in the background after the join. Until they do, eve's history is locked,
        // and the banner clears once they have.
        eventually("eve's history unlocks, and the banner clears", || {
            eve.engine
                .community_view(id)
                .ok()
                .filter(|v| !v.history_locked)
                .map(|_| ())
        })
        .await;

        finish(vec![alice, bob, dave, eve]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn member_only_waits_until_every_member_device_answers() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;

        let status = alice
            .engine
            .member_only_status(id)
            .await
            .expect("status with every member online");
        assert!(status.can_enable, "every member device answers");
        assert!(!status.on);

        // Bob goes offline, so his device cannot say it supports member-only control.
        let (_, bob_dir) = stop(bob).await;
        let status = alice.engine.member_only_status(id).await.expect("status");
        assert!(!status.can_enable, "an offline member blocks the switch");
        assert_eq!(status.waiting, 1);
        assert!(
            alice.engine.set_member_only(id, true).await.is_err(),
            "the switch stays off while a member device is unaccounted for"
        );
        let _ = std::fs::remove_dir_all(bob_dir);

        finish(vec![alice]).await;
    })
    .await;
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create copy");
    for entry in std::fs::read_dir(src).expect("read dir") {
        let entry = entry.expect("entry");
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy file");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_member_who_left_cannot_read_later_control_events() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let carol = peer("carol").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        eventually("bob sees member-only control", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.member_only)
                .map(|_| ())
        })
        .await;

        // Bob's data as it is before he leaves, with the keys of every epoch he was in. Windows locks
        // open files, so the copy is taken after a clean shutdown, and Bob restarts to leave.
        let (bob_identity, bob_dir) = stop(bob).await;
        let snapshot = scratch("bob-before-leave");
        copy_dir(&bob_dir, &snapshot);
        let bob = start(bob_identity, bob_dir.clone()).await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let epoch_before = alice.engine.mls_epoch(id);
        bob.engine.leave_community(id).await.expect("bob leaves");
        eventually("carol sees bob has left", || {
            carol
                .engine
                .community_view(id)
                .ok()
                .filter(|v| !v.members.iter().any(|m| m.root_id == bob.identity.root_id().to_string()))
                .map(|_| ())
        })
        .await;
        // The removal commit moves the epoch. Events made after that are in an epoch Bob is not in.
        eventually("the removal commit moves the epoch", || {
            (alice.engine.mls_epoch(id) != epoch_before).then_some(())
        })
        .await;

        // A channel and a role change, both after the departure.
        alice
            .engine
            .create_channel(id, "after-leave".into())
            .await
            .expect("channel after the departure");
        alice
            .engine
            .moderate(id, dissent_core::ModAction::GrantModerator, carol.identity.root_id())
            .await
            .expect("role change after the departure");
        eventually("carol reads the channel and the role", || {
            let v = carol.engine.community_view(id).ok()?;
            let role = v
                .members
                .iter()
                .find(|m| m.root_id == carol.identity.root_id().to_string())?
                .role
                .clone();
            (v.channels.iter().any(|c| c.name == "after-leave") && role == "moderator").then_some(())
        })
        .await;

        // Bob comes back on the snapshot he kept. He holds every key he had before leaving, and
        // none for the epochs after it.
        let (bob_identity, _) = stop(bob).await;
        let _ = std::fs::remove_dir_all(&bob_dir);
        copy_dir(&snapshot, &bob_dir);
        let bob = start(bob_identity, bob_dir).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let view = bob.engine.community_view(id).ok();
        assert!(
            !view.as_ref().is_some_and(|v| v.channels.iter().any(|c| c.name == "after-leave")),
            "a member who left cannot read the channel made after it left"
        );
        assert!(
            view.as_ref().is_none_or(|v| {
                v.members
                    .iter()
                    .find(|m| m.root_id == carol.identity.root_id().to_string())
                    .is_none_or(|m| m.role != "moderator")
            }),
            "a member who left cannot read the role change made after it left"
        );

        // The remaining members still read everything.
        assert!(has_channel(&alice.engine, id, "after-leave"));
        finish(vec![alice, bob, carol]).await;
        let _ = std::fs::remove_dir_all(&snapshot);
    })
    .await;
}

/// A joiner whose first epoch-key fetch waits 20 s after the join, long enough for the test to take the
/// only member down first. Its first attempt then fails, and a member's beacon starts the next.
async fn slow_joiner(name: &str) -> Peer {
    let identity = dissent_core::Identity::create(now());
    let dir = scratch(name);
    let timings = dissent_core::Timings {
        history_first_retry: Duration::from_secs(20),
        ..dissent_core::test_support::fast_timings()
    };
    let (engine, events) = dissent_core::Engine::start_with_timings(
        identity.clone(),
        dir.clone(),
        offline_on(0),
        dissent_core::test_support::vault_for(&dir),
        timings,
    )
    .await
    .expect("engine starts");
    engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_joiner_whose_members_were_offline_unlocks_once_one_comes_back() {
    bounded(async {
        // Alice keeps her port across a restart, so the joiner still reaches her address.
        let alice_identity = dissent_core::Identity::create(now());
        let alice_dir = scratch("alice-restart");
        let port = free_port();
        let alice = start_on(alice_identity.clone(), alice_dir.clone(), port).await;
        alice
            .engine
            .set_display_name("alice".to_string())
            .await
            .expect("name set");
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        // New servers default to since-join. Carol must receive the channel created before she joins.
        alice
            .engine
            .set_history_visibility(id, HistorySetting::All)
            .await
            .expect("history of all");
        alice
            .engine
            .create_channel(id, "before-carol".into())
            .await
            .expect("channel before the joiner");
        // Bob is offline, so only alice could answer for history.
        let (_, bob_dir) = stop(bob).await;

        let carol = slow_joiner("carol").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        // Alice's own view of carol is not waited for: under load that can take longer than the delay.
        carol.engine.join_community(&invite).await.expect("carol joins");
        let joined = std::time::Instant::now();
        // Carol has to hold the locked channel before alice goes down. Her history stays locked while
        // her fetch is held back, so this wait cannot be satisfied by keys. It is satisfied once her
        // sync has delivered the channel's event.
        eventually("carol's history is locked", || {
            carol
                .engine
                .community_view(id)
                .ok()
                .filter(|v| v.history_locked)
                .map(|_| ())
        })
        .await;
        // Alice goes down once carol holds the event.
        let (_, alice_dir) = stop(alice).await;
        // Carol's first attempt runs once the delay has passed, while alice is still down, and fails.
        tokio::time::sleep(Duration::from_secs(25).saturating_sub(joined.elapsed())).await;
        assert!(
            !has_channel(&carol.engine, id, "before-carol"),
            "with no member online, carol stays locked"
        );

        // Alice comes back on the same port. Her beacon wakes carol's retry, and the banner clears
        // without restarting carol.
        let alice = start_on(alice_identity, alice_dir, port).await;
        eventually_within(Duration::from_secs(60), "carol's history unlocks", || {
            carol
                .engine
                .community_view(id)
                .ok()
                .filter(|v| !v.history_locked && has_channel(&carol.engine, id, "before-carol"))
                .map(|_| ())
        })
        .await;

        finish(vec![alice, carol]).await;
        let _ = std::fs::remove_dir_all(bob_dir);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_member_only_channel_tells_members_the_community_changed() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        eventually("bob sees member-only control", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.member_only)
                .map(|_| ())
        })
        .await;

        // A plain event makes bob's engine emit `CommunityUpdated`, which is what refreshes the UI. A
        // member-sealed event has to do the same, or new channels stay unseen until something else moves.
        let mut events = bob._events.resubscribe();
        alice
            .engine
            .create_channel(id, "notified-room".into())
            .await
            .expect("channel");
        eventually("bob sees the channel", || {
            has_channel(&bob.engine, id, "notified-room").then_some(())
        })
        .await;
        let id_text = id.to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut told = false;
        while !told && std::time::Instant::now() < deadline {
            if let Ok(Ok(dissent_core::EngineEvent::CommunityUpdated { community_id })) =
                tokio::time::timeout(Duration::from_millis(200), events.recv()).await
            {
                told = community_id == id_text;
            }
        }
        assert!(told, "bob's engine tells the UI that the community changed");

        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_server_icon_reaches_members_in_a_member_only_server() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        eventually("bob sees member-only control", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.member_only)
                .map(|_| ())
        })
        .await;

        // The icon event is member-sealed, so bob's picture fetch depends on its side effects running.
        let png = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src-tauri/icons/128x128.png"
        ))
        .expect("the test icon");
        alice
            .engine
            .set_community_icon(id, png)
            .await
            .expect("alice sets the icon");
        eventually_within(Duration::from_secs(60), "bob has the icon", || {
            bob.engine
                .community_view(id)
                .ok()
                .filter(|v| v.icon.is_some())
                .map(|_| ())
        })
        .await;

        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_relay_set_in_a_member_only_server_is_adopted_by_members() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        alice
            .engine
            .set_member_only(id, true)
            .await
            .expect("member-only control on");
        let relay = "https://relay.member-only.example.test/";
        assert!(
            !dissent_core::test_support::adopted_relays(&bob.engine)
                .iter()
                .any(|r| r.contains("member-only.example")),
            "bob has not adopted the relay yet"
        );
        alice
            .engine
            .set_community_relays(id, vec![relay.to_string()])
            .await
            .expect("alice sets the relay");
        // SetRelays is member-sealed here, so bob adopts it only if its side effects run.
        eventually("bob adopts the relay", || {
            dissent_core::test_support::adopted_relays(&bob.engine)
                .iter()
                .any(|r| r.contains("member-only.example"))
                .then_some(())
        })
        .await;

        finish(vec![alice, bob]).await;
    })
    .await;
}
