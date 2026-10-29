//! Message history across versions. While an older member is in the community, messages keep the older form,
//! which joiners cannot read. A message sent before that member joined uses the history form.

use std::sync::atomic::Ordering;

use crate::membership::tests::{eventually, general, readable, Node};
use crate::rpc::Response;
use crate::testutil::on_own_thread;

#[test]
fn a_message_sent_while_an_older_member_is_present_stays_locked_for_joiners() {
    on_own_thread(|| async {
        let alice = Node::named("alice-mixed").await;
        let id = alice.engine.create_community("Mixed".into()).await.expect("create").id.parse().unwrap();
        alice
            .engine
            .set_history_visibility(id, dissent_events::HistorySetting::All)
            .await
            .expect("owner chooses all");
        let channel = general(&alice.engine, id);
        let first = alice.engine.send_message(id, channel, "first".into()).await.expect("send first");
        assert!(alice.engine.history_seal(id, "probe").is_some(), "a community of one uses the history form");

        // Bob is on a version that predates history messages.
        let bob = Node::named("bob-mixed").await;
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        let invite = alice.engine.create_invite(id).await.expect("invite");
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;

        let second = alice.engine.send_message(id, channel, "second".into()).await.expect("send second");
        assert!(alice.engine.history_seal(id, "probe").is_none(), "an older member means the older form");

        let carol = Node::named("carol-mixed").await;
        carol.engine.join_community(&invite).await.expect("carol joins");
        eventually("carol reads the first message in the history form", || {
            readable(&carol.engine, id, channel, "first")
        })
        .await;
        eventually("carol sees the second message locked", || {
            carol
                .engine
                .channel_messages(id, channel, 50)
                .iter()
                .any(|m| m.id == second.id && m.locked && m.text.is_empty())
        })
        .await;
        assert!(first.id != second.id);
    });
}

#[test]
fn a_send_with_every_other_member_offline_returns_quickly() {
    on_own_thread(|| async {
        let alice = Node::named("alice-offline").await;
        let id = alice.engine.create_community("Quiet".into()).await.expect("create").id.parse().unwrap();
        let channel = general(&alice.engine, id);
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-offline").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        // Bob goes offline, so no check of his device can answer.
        let _ = bob.stop().await;

        let started = std::time::Instant::now();
        alice.engine.send_message(id, channel, "into the void".into()).await.expect("send");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "a send took {:?}", started.elapsed());
    });
}

#[test]
fn the_history_form_switches_on_once_the_flag_is_set() {
    on_own_thread(|| async {
        let alice = Node::named("alice-flag").await;
        let id = alice.engine.create_community("Flag".into()).await.expect("create").id.parse().unwrap();
        // Nobody else is a member, so there is nothing to ask.
        assert!(alice.engine.history_seal(id, "solo").is_some());

        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-flag").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        // Alice admitted bob, and checked his devices before the welcome, so the answer is already known.
        assert!(alice.engine.history_seal(id, "admitted").is_some());
        assert!(alice.engine.refresh_history_ready(id).await, "bob supports history messages");
        assert!(alice.engine.history_seal(id, "supported").is_some());

        // Bob turns support off, and the next check flips the flag back.
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        assert!(!alice.engine.refresh_history_ready(id).await);
        assert!(alice.engine.history_seal(id, "unsupported").is_none());
    });
}

#[test]
fn a_thousand_sends_read_the_key_file_at_most_once() {
    on_own_thread(|| async {
        let alice = Node::named("alice-reads").await;
        let id = alice.engine.create_community("Reads".into()).await.expect("create").id.parse().unwrap();
        let before = alice.engine.inner.history_file_reads.load(Ordering::Relaxed);
        let started = std::time::Instant::now();
        for n in 0..1000 {
            assert!(alice.engine.history_seal(id, &format!("message {n}")).is_some(), "send {n}");
        }
        let reads = alice.engine.inner.history_file_reads.load(Ordering::Relaxed) - before;
        assert!(reads <= 1, "the key file was read {reads} times");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "1000 sends took {:?}", started.elapsed());
    });
}

#[test]
fn the_new_format_switches_on_within_two_seconds_of_the_last_unsupported_member_updating() {
    on_own_thread(|| async {
        let alice = Node::named("alice-wake").await;
        let id = alice.engine.create_community("Wake".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-wake").await;
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(!alice.engine.refresh_history_ready(id).await, "bob does not support it yet");
        assert!(alice.engine.history_seal(id, "before").is_none());

        // Bob updates, and his presence beacon reaches Alice.
        bob.engine.inner.history_support.store(true, Ordering::Relaxed);
        alice.engine.history_member_seen(bob.identity.root_id());
        let started = std::time::Instant::now();
        while alice.engine.history_seal(id, "after").is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(2), "still the older form after {:?}", started.elapsed());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
}

#[test]
fn a_restart_starts_from_the_last_readiness_answer() {
    on_own_thread(|| async {
        let alice = Node::named("alice-restart-ready").await;
        let id = alice.engine.create_community("Restart".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-restart-ready").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(alice.engine.refresh_history_ready(id).await, "bob supports it");

        let (identity, dir, port) = alice.stop().await;
        let again = Node::boot(identity, dir, port).await;
        // No check runs after the restart: the saved "yes" is the answer, so the first send is in the new form.
        assert!(again.engine.history_seal(id, "right after the restart").is_some());
    });
}

#[test]
fn a_member_admitted_by_us_is_counted_before_the_welcome_goes_out() {
    on_own_thread(|| async {
        let alice = Node::named("alice-admit").await;
        let id = alice.engine.create_community("Admit".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-admit").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        // Alice has admitted bob, so the answer for his devices is already known, with no check left to run.
        eventually("alice has admitted bob", || {
            alice.engine.community_view(id).map(|v| v.members.len() == 2).unwrap_or(false)
        })
        .await;
        assert!(alice.engine.history_seal(id, "no check to wait for").is_some());
    });
}

/// A member who left gets no history keys: no push reaches their device, and a fetch from them is refused.
#[test]
fn a_member_who_left_gets_no_history_keys() {
    on_own_thread(|| async {
        let alice = Node::named("alice-left-keys").await;
        let id = alice.engine.create_community("Left".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-left-keys").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        let bob_device = bob.identity.device_id();
        eventually("alice admits bob's device", || alice.engine.person_device(id, bob_device)).await;

        bob.engine.leave_community(id).await.expect("bob leaves");
        eventually("alice sees bob leave", || !alice.engine.person_device(id, bob_device)).await;
        assert!(
            matches!(alice.engine.serve_history_keys(bob_device, id), Response::Error(_)),
            "keys are not served to a departed member"
        );
    });
}

/// Whether `engine`'s replay of `id` holds a `HistorySupport` yes from `node`'s device.
fn declared(engine: &crate::Engine, id: dissent_identity::CommunityId, node: &Node) -> bool {
    let device = node.engine.inner.identity.device_id();
    engine.read().communities.get(&id).is_some_and(|c| c.state().declares_history(&device))
}

/// Whether `engine` lists `node`'s device among the member devices of `id`.
fn lists_device(engine: &crate::Engine, id: dissent_identity::CommunityId, node: &Node) -> bool {
    let device = node.engine.inner.identity.device_id();
    let w = engine.read();
    w.communities.get(&id).is_some_and(|c| w.member_devices(c).contains(&device))
}

#[test]
fn a_member_that_did_not_admit_a_joiner_counts_its_declared_support_without_asking() {
    on_own_thread(|| async {
        let alice = Node::named("alice-declared").await;
        let id = alice.engine.create_community("Declared".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-declared").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(bob.engine.refresh_history_ready(id).await, "alice supports history messages");

        // From here bob asks nobody, so only the events can tell him about a newcomer.
        bob.engine.inner.history_probes.store(false, Ordering::Relaxed);
        let carol = Node::named("carol-declared").await;
        carol.engine.join_community(&invite).await.expect("carol joins through alice");
        eventually("bob replays carol's join and her history support", || {
            lists_device(&bob.engine, id, &carol) && declared(&bob.engine, id, &carol)
        })
        .await;
        assert!(bob.engine.history_seal(id, "for carol too").is_some(), "bob keeps the history form for carol");
        // A check that reaches nobody still counts the devices that declared: the founder did too.
        assert!(declared(&bob.engine, id, &alice));
        assert!(bob.engine.refresh_history_ready(id).await);
        assert!(bob.engine.history_seal(id, "after the check").is_some());
    });
}

#[test]
fn a_joiner_that_does_not_declare_support_still_needs_an_answer() {
    on_own_thread(|| async {
        let alice = Node::named("alice-undeclared").await;
        let id = alice.engine.create_community("Undeclared".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-undeclared").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert!(bob.engine.refresh_history_ready(id).await);

        bob.engine.inner.history_probes.store(false, Ordering::Relaxed);
        // Carol is on a version from before history messages, so her join says nothing about them.
        let carol = Node::named("carol-undeclared").await;
        carol.engine.inner.history_support.store(false, Ordering::Relaxed);
        carol.engine.join_community(&invite).await.expect("carol joins");
        eventually("bob lists carol's device", || lists_device(&bob.engine, id, &carol)).await;
        assert!(!declared(&bob.engine, id, &carol));
        assert!(bob.engine.history_seal(id, "older form").is_none(), "an undeclared joiner means the older form");
    });
}

#[test]
fn a_device_that_answers_no_overrides_its_earlier_declaration() {
    on_own_thread(|| async {
        let alice = Node::named("alice-override").await;
        let id = alice.engine.create_community("Override".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-override").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("alice replays bob's declaration", || declared(&alice.engine, id, &bob)).await;
        assert!(alice.engine.refresh_history_ready(id).await);
        // Bob goes back to a version without history messages. His own answer now wins over the old event.
        bob.engine.inner.history_support.store(false, Ordering::Relaxed);
        assert!(!alice.engine.refresh_history_ready(id).await);
        assert!(alice.engine.history_seal(id, "older form").is_none());
    });
}

/// How many `HistorySupport` events from `node`'s device `engine` stores in `id`.
fn declarations(engine: &crate::Engine, id: dissent_identity::CommunityId, node: &Node) -> usize {
    let device = node.engine.inner.identity.device_id();
    engine.read().communities.get(&id).map_or(0, |c| c.history_declarations_by(&device))
}

#[test]
fn an_existing_member_declares_once_after_it_upgrades_and_again_only_when_its_setting_changes() {
    on_own_thread(|| async {
        // Alice founds the server on a version from before declarations: support off, so she says nothing.
        let alice = Node::named("alice-upgrade").await;
        alice.engine.inner.history_support.store(false, Ordering::Relaxed);
        let id = alice.engine.create_community("Upgrade".into()).await.expect("create").id.parse().unwrap();
        let invite = alice.engine.create_invite(id).await.expect("invite");
        let bob = Node::named("bob-upgrade").await;
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;
        assert_eq!(declarations(&alice.engine, id, &alice), 0, "the older version declares nothing");

        // Alice upgrades: the restart declares her support once.
        let (identity, dir, port) = alice.stop().await;
        let alice = Node::boot(identity, dir, port).await;
        eventually("alice declares after the upgrade", || declarations(&alice.engine, id, &alice) == 1).await;
        eventually("bob replays alice's declaration", || declared(&bob.engine, id, &alice)).await;

        // Another restart with the same setting sends nothing new, nor does fresh traffic.
        let (identity, dir, port) = alice.stop().await;
        let alice = Node::boot(identity, dir, port).await;
        let channel = general(&bob.engine, id);
        bob.engine.send_message(id, channel, "after the restart".into()).await.expect("send");
        eventually("alice reads bob", || readable(&alice.engine, id, channel, "after the restart")).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(declarations(&alice.engine, id, &alice), 1, "no repeat with an unchanged setting");

        // Turning support off says so once, and turning it off again says nothing.
        alice.engine.set_history_support(false);
        eventually("alice declares the change", || declarations(&alice.engine, id, &alice) == 2).await;
        eventually("bob replays the no", || !declared(&bob.engine, id, &alice)).await;
        alice.engine.set_history_support(false);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(declarations(&alice.engine, id, &alice), 2);
    });
}

#[test]
fn a_newly_linked_device_declares_once_in_the_communities_it_syncs() {
    on_own_thread(|| async {
        let alice = Node::named("alice-linked").await;
        let id = alice.engine.create_community("Linked".into()).await.expect("create").id.parse().unwrap();
        let bob = Node::named("bob-linked").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        bob.engine.join_community(&invite).await.expect("bob joins");
        eventually("bob is in the community", || bob.engine.community_view(id).is_ok()).await;

        // Bob links a tablet, which joins the server under his root.
        let pending = crate::start_device_link("Tablet").expect("link request");
        let grant = bob.engine.approve_device(&pending.link().expect("link")).expect("bob approves the tablet");
        let identity = pending.finish(&grant).expect("tablet identity");
        let tablet = Node::boot(identity, crate::membership::tests::scratch("bob-tablet"), crate::membership::tests::free_port()).await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        tablet.engine.join_community(&invite).await.expect("the tablet joins");

        eventually("alice replays the tablet's declaration", || declared(&alice.engine, id, &tablet)).await;
        // The join, the first sync and the startup each could declare. One declaration goes out.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        assert_eq!(declarations(&tablet.engine, id, &tablet), 1, "the tablet declares once");
        assert_eq!(declarations(&alice.engine, id, &tablet), 1);
        // Bob's own device declared at its join, and is not counted twice either.
        assert_eq!(declarations(&alice.engine, id, &bob), 1);
    });
}

#[test]
fn a_send_right_after_the_epoch_moves_derives_its_key_rather_than_falling_back() {
    on_own_thread(|| async {
        let alice = Node::named("alice-epoch-key").await;
        let id = alice.engine.create_community("Epoch".into()).await.expect("create").id.parse().unwrap();
        // A commit has moved the group, and `note_epoch` has not recorded the new epoch's key yet. Modelled by
        // dropping the recorded keys: the group still exports them.
        let paths = alice.engine.read().paths.clone();
        alice.engine.store_keys(&paths, id, |keys| keys.clear());
        let epoch = alice.engine.mls_epoch(id).expect("in the group");
        assert!(alice.engine.history_key(id, epoch).is_none(), "no key recorded for the epoch");
        let (sealed_in, ciphertext) = alice.engine.history_seal(id, "right after the commit").expect("the history form");
        assert_eq!(sealed_in, epoch);
        assert!(alice.engine.open_history_text(id, epoch, &ciphertext).is_some(), "it opens with the derived key");
        // An epoch the group is not in is never derived.
        assert!(alice.engine.open_history_text(id, epoch + 1, &ciphertext).is_none());
    });
}
