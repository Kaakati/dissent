//! Adversarial membership tests: a member that breaks the commit rules is refused by everyone, a
//! welcome that lists an outsider is refused, and a seeder can admit a joiner while the owner is
//! offline.

use std::time::Duration;

use super::tests::{eventually, general, readable, three_members, Node};
use crate::CommunityId;

/// A key package of a fresh device that has never joined the community.
async fn outsider_key_package(name: &str) -> (Node, Vec<u8>) {
    let mallory = Node::named(name).await;
    let key_package = mallory
        .engine
        .with_mls(|mls| mls.take_key_packages(1))
        .expect("key package")
        .remove(0);
    (mallory, key_package)
}

#[tokio::test(flavor = "multi_thread")]
async fn member_commit_adding_an_outsider_is_rejected_by_everyone() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, _invite) = three_members().await;
    let (mallory, key_package) = outsider_key_package("mallory").await;
    let mallory_device = mallory.identity.device_id();

    // Bob is a legitimate member, but this add brings in a device of no member.
    let forged = bob
        .engine
        .with_mls(|mls| mls.add_members(&id.0, &[key_package]))
        .expect("bob builds the commit");

    for honest in [&alice.engine, &carol.engine] {
        let epoch = honest.mls_epoch(id);
        assert!(
            honest.apply_commit(id, &forged.commit).is_err(),
            "a commit adding an outsider must be refused"
        );
        assert_eq!(honest.mls_epoch(id), epoch, "a refused commit leaves the epoch alone");
        let members = honest
            .with_mls_read(|mls| mls.members(&id.0))
            .expect("members");
        assert!(
            members.iter().all(|c| c.device != mallory_device),
            "the outsider never enters an honest group"
        );
    }

    alice
        .engine
        .send_message(id, general(&alice.engine, id), "secret".into())
        .await
        .expect("alice sends");
    assert!(!readable(&mallory.engine, id, general(&alice.engine, id), "secret"));
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn member_commit_removing_a_current_member_is_rejected_by_everyone() {
    crate::testutil::bounded(async {
    let (alice, bob, carol, id, _invite) = three_members().await;
    let carol_device = carol.identity.device_id();

    // Bob may remove devices of banned or departed members, not of Carol, who is current.
    let forged = bob
        .engine
        .with_mls(|mls| mls.remove_members(&id.0, &[carol_device]))
        .expect("bob builds the removal");

    for honest in [&alice.engine, &bob.engine] {
        let epoch = honest.mls_epoch(id);
        assert!(
            honest.apply_commit(id, &forged).is_err(),
            "a removal of a current member must be refused"
        );
        assert_eq!(honest.mls_epoch(id), epoch);
    }
    let alice_members = alice
        .engine
        .with_mls_read(|mls| mls.members(&id.0))
        .expect("members");
    assert!(alice_members.iter().any(|c| c.device == carol_device));
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn welcome_listing_a_non_member_is_refused_and_a_real_join_still_works() {
    crate::testutil::bounded(async {
    let (_alice, bob, _carol, id, invite) = three_members().await;
    let (_mallory, key_package) = outsider_key_package("mallory").await;

    let dave = Node::named("dave").await;
    dave.engine
        .join_without_group(&invite)
        .await
        .expect("dave joins the community without a group");
    eventually("dave knows the members", || {
        dave.engine.community_view(id).map(|v| v.members.len()).ok() == Some(4)
    })
    .await;

    // Bob must have replayed Dave's Join before he will add Dave. Dave seeing the members is not enough.
    eventually("bob knows dave", || {
        bob.engine.community_view(id).map(|v| v.members.len()).ok() == Some(4)
    })
    .await;
    // An honest join through Bob is accepted, so the rule does not block real joiners.
    let device = bob.identity.device_id();
    assert!(
        dave.engine.mls_join_via(id, &[device]).await,
        "an honest welcome is accepted"
    );
    assert!(dave.engine.mls_epoch(id).is_some());

    // Bob then adds an outsider. The welcome for that add lists the outsider's device, so a new
    // joiner must refuse it. Bob's group has moved on, so this uses a fresh joiner.
    let eve = Node::named("eve").await;
    eve.engine
        .join_without_group(&invite)
        .await
        .expect("eve joins the community without a group");
    eventually("eve knows the members", || {
        eve.engine.community_view(id).map(|v| v.members.len()).ok() == Some(5)
    })
    .await;
    let forged = bob
        .engine
        .with_mls(|mls| mls.add_members(&id.0, &[key_package]))
        .expect("bob builds the add");
    assert!(
        eve.engine.accept_welcome(id, &forged.welcome).is_err(),
        "a welcome that lists an outsider must be refused"
    );
    assert_eq!(eve.engine.mls_epoch(id), None);
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seeder_admits_a_joiner_while_the_owner_is_offline() {
    crate::testutil::bounded(async {
    let alice = Node::named("alice").await;
    let id: CommunityId = alice
        .engine
        .create_community("Seeded".into())
        .await
        .expect("create")
        .id
        .parse()
        .unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let seeder = Node::named("seeder").await;
    seeder
        .engine
        .join_community(&invite)
        .await
        .expect("seeder joins");
    eventually("seeder has the group", || seeder.engine.mls_epoch(id).is_some()).await;

    // The owner goes offline. Only the plain member is left to admit anyone.
    let _ = alice.stop().await;

    let second_invite = seeder
        .engine
        .create_invite(id)
        .await
        .expect("a member can invite");
    let bob = Node::named("bob").await;
    bob.engine
        .join_community(&second_invite)
        .await
        .expect("bob joins through the seeder");
    eventually("bob has the group", || bob.engine.mls_epoch(id).is_some()).await;

    let channel = general(&seeder.engine, id);
    seeder
        .engine
        .send_message(id, channel, "after bob joined".into())
        .await
        .expect("seeder sends");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !readable(&bob.engine, id, channel, "after bob joined") {
        assert!(std::time::Instant::now() < deadline, "bob never read the message");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    }).await;
}

// ---- A commit that added someone since banned: see `CommitRules::allows_commit` ----

mod banned_add {
    use std::collections::BTreeSet;

    use dissent_identity::{DeviceCert, Identity};
    use dissent_mls::CommitInfo;

    use super::super::tests::{eventually, general, readable, three_members, Node};
    use super::super::CommitRules;
    use crate::{CommunityId, ModAction};

    fn person() -> DeviceCert {
        Identity::create(1).cert
    }

    fn info(committer: &DeviceCert, adds: &[&DeviceCert]) -> CommitInfo {
        CommitInfo { committer: committer.clone(), adds: adds.iter().map(|c| (*c).clone()).collect(), removes: Vec::new(), epoch_after: 2 }
    }

    #[test]
    fn only_a_current_person_may_add_only_a_real_member_since_banned() {
        let (current, banned, left, stranger) = (person(), person(), person(), person());
        let rules = CommitRules {
            active: BTreeSet::from([current.root]),
            departed: BTreeSet::from([banned.root, left.root]),
            persons: BTreeSet::from([current.root]),
            banned_persons: BTreeSet::from([banned.root]),
        };
        assert!(rules.allows_commit(&info(&current, &[&current])), "a current person, as ever");
        assert!(rules.allows_commit(&info(&current, &[&banned])), "someone since banned, from a current person");
        assert!(!rules.allows_commit(&info(&banned, &[&current])), "a banned committer is refused");
        assert!(!rules.allows_commit(&info(&left, &[&current])), "a committer who left is refused");
        assert!(!rules.allows_commit(&info(&stranger, &[&current])), "a committer who was never a member is refused");
        assert!(!rules.allows_commit(&info(&current, &[&stranger])), "a device of no member is refused");
        assert!(!rules.allows_commit(&info(&current, &[&left])), "someone who left is not let back this way");
        assert!(!rules.allows_commit(&info(&current, &[&current, &stranger])), "one bad add refuses the commit");
    }

    /// The group's three, and dave, who joined and was banned and is out of the group. Returns the fourth as well.
    async fn after_a_ban() -> (Node, Node, Node, Node, CommunityId) {
        let (alice, bob, carol, id, invite) = three_members().await;
        let dave = Node::named("dave").await;
        dave.engine.join_community(&invite).await.expect("dave joins");
        let dave_device = dave.identity.device_id();
        let in_groups = |want: bool| {
            [&alice, &bob, &carol].iter().all(|n| {
                n.engine.with_mls_read(|mls| mls.members(&id.0)).is_ok_and(|m| m.iter().any(|c| c.device == dave_device) == want)
            })
        };
        eventually("dave is in the group", || in_groups(true)).await;
        alice.engine.moderate(id, ModAction::Ban, dave.root()).await.expect("ban");
        eventually("dave is out of the group", || in_groups(false)).await;
        eventually("everyone is on one epoch", || {
            let epoch = alice.engine.mls_epoch(id);
            bob.engine.mls_epoch(id) == epoch && carol.engine.mls_epoch(id) == epoch
        })
        .await;
        (alice, bob, carol, dave, id)
    }

    fn add_commit(bob: &Node, dave: &Node, id: CommunityId) -> Vec<u8> {
        let package = dave.engine.with_mls(|mls| mls.take_key_packages(1)).expect("key package").remove(0);
        bob.engine.with_mls(|mls| mls.add_members(&id.0, &[package])).expect("bob builds the add").commit
    }

    /// Whether all three hold one group: the same epoch and the same secrets.
    fn one_group(nodes: [&Node; 3], id: CommunityId, dave: &Node) -> bool {
        let state = |n: &Node| (n.engine.mls_epoch(id), n.engine.with_mls_read(|m| m.export_secret(&id.0, "check", 16)).ok());
        let first = state(nodes[0]);
        let device = dave.identity.device_id();
        nodes.iter().all(|n| {
            state(n) == first && n.engine.with_mls_read(|m| m.members(&id.0)).is_ok_and(|m| m.iter().all(|c| c.device != device))
        })
    }

    /// As it happens for real: the add and the removal both exist, and carol reaches them after the ban.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_heard_of_the_ban_first_takes_the_add_and_the_removal() {
        crate::testutil::bounded(async {
            let (alice, bob, carol, dave, id) = after_a_ban().await;
            let channel = general(&alice.engine, id);
            let add = add_commit(&bob, &dave, id);
            bob.engine.publish_commit(id, add).await;
            let removal = bob.engine.with_mls(|mls| mls.remove_members(&id.0, &[dave.identity.device_id()])).expect("bob removes dave");
            bob.engine.publish_commit(id, removal).await;
            eventually("everyone converges with dave out", || one_group([&alice, &bob, &carol], id, &dave)).await;
            carol.engine.send_message(id, channel, "after the add".into()).await.expect("carol sends");
            eventually("alice reads it", || readable(&alice.engine, id, channel, "after the add")).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            assert!(!readable(&dave.engine, id, channel, "after the add"), "dave reads nothing new");
        })
        .await;
    }

    /// The add stands alone: a current member made it on purpose, or whoever would have removed the device is gone.
    /// The devices remove it in their turn.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_add_nobody_followed_is_undone_by_reconcile() {
        crate::testutil::bounded(async {
            let (alice, bob, carol, dave, id) = after_a_ban().await;
            let add = add_commit(&bob, &dave, id);
            bob.engine.publish_commit(id, add).await;
            eventually("everyone converges with dave out", || one_group([&alice, &bob, &carol], id, &dave)).await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nothing_is_sealed_while_a_banned_device_is_in_the_group() {
        crate::testutil::bounded(async {
            let (alice, bob, carol, dave, id) = after_a_ban().await;
            let channel = general(&alice.engine, id);
            let commit = add_commit(&bob, &dave, id);
            // Taken without the follow-up, so the window stays open for the checks.
            carol.engine.with_mls(|mls| mls.process_commit_with_policy(&id.0, &commit, |_| true)).expect("carol takes it");
            assert!(carol.engine.banned_device_in_group(id));
            assert!(carol.engine.seal(id, "secret").is_err(), "no sealed message");
            assert!(carol.engine.history_seal(id, "secret").is_none(), "no history message");
            // A send holds, removes the device in carol's turn, and then goes out.
            carol.engine.send_message(id, channel, "sealed again".into()).await.expect("carol's send waits for the removal");
            assert!(!carol.engine.banned_device_in_group(id));
        })
        .await;
    }

    /// The guard is for the others. The banned device's own send is refused as it always was, at once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_banned_sender_is_refused_at_once_with_the_usual_reason() {
        crate::testutil::bounded(async {
            let (alice, _bob, _carol, dave, id) = after_a_ban().await;
            let channel = general(&alice.engine, id);
            let started = std::time::Instant::now();
            let sent = dave.engine.send_message(id, channel, "after my ban".into()).await;
            assert!(matches!(sent, Err(crate::CoreError::Rejected(_))), "refused for the ban: {sent:?}");
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "not held by the guard: {:?}", started.elapsed());
            assert!(!dave.engine.banned_device_in_group(id));
        })
        .await;
    }

    /// A device that refuses the add, as a client from before this does, is modelled by one that is off. The rest go on.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_others_go_on_without_a_device_that_does_not_take_the_add() {
        crate::testutil::bounded(async {
            let (alice, bob, carol, dave, id) = after_a_ban().await;
            let channel = general(&alice.engine, id);
            let _ = carol.stop().await;
            let commit = add_commit(&bob, &dave, id);
            assert!(matches!(alice.engine.apply_commit(id, &commit), Ok(true)));
            eventually("alice and bob converge", || {
                let same = alice.engine.mls_epoch(id) == bob.engine.mls_epoch(id);
                same && !alice.engine.banned_device_in_group(id) && !bob.engine.banned_device_in_group(id)
            })
            .await;
            alice.engine.send_message(id, channel, "still talking".into()).await.expect("alice sends");
            eventually("bob reads it", || readable(&bob.engine, id, channel, "still talking")).await;
        })
        .await;
    }
}
