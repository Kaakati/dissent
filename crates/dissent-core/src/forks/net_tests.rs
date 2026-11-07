//! Fork detection on loopback: a real fork reported by every device, a device that was away noticing
//! it from a check, no report without a fork, and checks that must not count.

use std::time::Duration;

use dissent_identity::{DeviceKey, Identity};

use crate::membership::tests::{eventually, Node};
use crate::test_support::fast_timings;
use crate::wire::SignedEpochCheck;
use crate::{CommunityId, ModAction};

/// alice (the owner) and `names` in one community, all on one epoch.
async fn members(names: &[&str]) -> (Node, Vec<Node>, CommunityId) {
    let alice = Node::named_with("alice", fast_timings()).await;
    let id: CommunityId = alice.engine.create_community("Forks".into()).await.expect("create").id.parse().unwrap();
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let mut others = Vec::new();
    for name in names {
        let node = Node::named_with(name, fast_timings()).await;
        node.engine.join_community(&invite).await.expect("joins");
        others.push(node);
    }
    let want = names.len() + 1;
    for node in std::iter::once(&alice).chain(&others) {
        eventually("everyone is a member", || node.engine.community_view(id).map(|v| v.members.len()).ok() == Some(want)).await;
    }
    for node in &others {
        node.engine.mls_catch_up(id, alice.identity.device_id()).await;
    }
    eventually("one epoch", || {
        let epoch = alice.engine.mls_epoch(id);
        epoch.is_some() && others.iter().all(|n| n.engine.mls_epoch(id) == epoch)
    })
    .await;
    (alice, others, id)
}

/// dave joins and is banned, so a commit may add him once more and two members may each remove him again: the
/// one commit two members can make at one epoch. Returns dave, out of the group.
async fn dave_banned(alice: &Node, others: &[Node], id: CommunityId) -> Node {
    let invite = alice.engine.create_invite(id).await.expect("invite");
    let dave = Node::named_with("dave", fast_timings()).await;
    dave.engine.join_community(&invite).await.expect("dave joins");
    let device = dave.identity.device_id();
    let in_group = |n: &Node, want: bool| {
        n.engine.with_mls_read(|mls| mls.members(&id.0)).is_ok_and(|m| m.iter().any(|c| c.device == device) == want)
    };
    eventually("dave is in the group", || in_group(alice, true) && others.iter().all(|n| in_group(n, true))).await;
    alice.engine.moderate(id, ModAction::Ban, dave.root()).await.expect("ban");
    eventually("dave is out", || in_group(alice, false) && others.iter().all(|n| in_group(n, false))).await;
    eventually("one epoch", || others.iter().all(|n| n.engine.mls_epoch(id) == alice.engine.mls_epoch(id))).await;
    dave
}

fn add_commit(by: &Node, dave: &Node, id: CommunityId) -> Vec<u8> {
    let package = dave.engine.with_mls(|mls| mls.take_key_packages(1)).expect("key package").remove(0);
    by.engine.with_mls(|mls| mls.add_members(&id.0, &[package])).expect("the add").commit
}

fn removal(by: &Node, dave: &Node, id: CommunityId) -> Vec<u8> {
    by.engine.with_mls(|mls| mls.remove_members(&id.0, &[dave.identity.device_id()])).expect("the removal")
}

/// Whether `dave`'s device is in `node`'s group.
fn in_group(node: &Node, dave: &Node, id: CommunityId) -> bool {
    let device = dave.identity.device_id();
    node.engine.with_mls_read(|mls| mls.members(&id.0)).is_ok_and(|m| m.iter().any(|c| c.device == device))
}

fn forked_at(node: &Node, id: CommunityId) -> Option<u64> {
    crate::test_support::fork_state(&node.engine, id).map(|(epoch, _)| epoch)
}

/// bob adds dave once more. alice and carol take the add and each remove him at the same epoch, before
/// hearing of the other: the group forks. Returns the forked epoch.
async fn fork(alice: &Node, bob: &Node, carol: &Node, dave: &Node, id: CommunityId) -> u64 {
    let add = add_commit(bob, dave, id);
    let added = bob.engine.mls_epoch(id).expect("bob is in the group");
    for node in [alice, carol] {
        // Once alice has the add, carol's own catch-up can fetch it from her before the test hands it over, and a
        // commit for an epoch already left is refused. Either way she has taken it.
        let outcome = node.engine.apply_commit(id, &add);
        let has_it = node.engine.mls_epoch(id) == Some(added) && in_group(node, dave, id);
        assert!(matches!(outcome, Ok(true)) || has_it, "takes the add: {outcome:?}, epoch {:?}", node.engine.mls_epoch(id));
    }
    let (by_alice, by_carol) = (removal(alice, dave, id), removal(carol, dave, id));
    let epoch = alice.engine.mls_epoch(id).expect("in the group");
    assert_eq!(carol.engine.mls_epoch(id), Some(epoch));
    alice.engine.publish_commit(id, by_alice).await;
    carol.engine.publish_commit(id, by_carol).await;
    epoch
}

#[tokio::test(flavor = "multi_thread")]
async fn two_members_committing_at_one_epoch_is_reported_by_everyone() {
    crate::testutil::bounded(async {
        let (alice, others, id) = members(&["bob", "carol"]).await;
        let (bob, carol) = (&others[0], &others[1]);
        let dave = dave_banned(&alice, &others, id).await;
        let epoch = fork(&alice, bob, carol, &dave, id).await;
        for node in [&alice, bob, carol] {
            eventually("the fork is reported", || forked_at(node, id) == Some(epoch)).await;
        }
        // A device reports the fork once it hears of the other commit, and names who is on the other branch as their
        // checks reach it, which can be a moment later: each name is waited for.
        let other_side = |n: &Node| crate::test_support::fork_state(&n.engine, id).map(|(_, names)| names).unwrap_or_default();
        let (alice_dev, carol_dev) = (alice.identity.device_id(), carol.identity.device_id());
        eventually("alice names carol", || other_side(&alice).contains(&carol_dev)).await;
        eventually("carol names alice", || other_side(carol).contains(&alice_dev)).await;
        // bob took one of the two removals. He names the device of the other.
        eventually("bob names the other branch", || {
            let names = other_side(bob);
            names.contains(&alice_dev) || names.contains(&carol_dev)
        })
        .await;
        let bob_names = other_side(bob);
        let carol_root = carol.root().to_string();
        let shown = || alice.engine.community_view(id).ok().and_then(|v| v.mls_fork);
        eventually("alice's view names carol", || shown().is_some_and(|f| f.roots.contains(&carol_root))).await;
        let view = shown().expect("the view shows the fork");
        assert_eq!(view.epoch, epoch);
        // alice's view names the people heard on the other branch: carol, and bob as well when the removal he took was
        // carol's (he then names alice), once alice has heard him there. Nobody else.
        let bob_with_carol = bob_names.contains(&alice_dev);
        let bob_root = bob.root().to_string();
        assert!(
            view.roots.iter().all(|r| *r == carol_root || (bob_with_carol && *r == bob_root)),
            "alice's view names only the other branch: {:?}, bob names {bob_names:?}",
            view.roots
        );
    })
    .await;
}

/// erin is off while the group forks. Back, she catches up on one branch, hears nothing of either commit
/// being announced, and notices the fork from a check sent by the other branch.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_was_away_notices_the_fork_from_a_check() {
    crate::testutil::bounded(async {
        let (alice, mut others, id) = members(&["bob", "carol", "erin"]).await;
        let erin = others.pop().expect("erin");
        let dave = dave_banned(&alice, &others, id).await;
        eventually("erin is on the epoch", || erin.engine.mls_epoch(id) == alice.engine.mls_epoch(id)).await;
        let (identity, dir, port) = erin.stop().await;
        let (bob, carol) = (&others[0], &others[1]);
        let epoch = fork(&alice, bob, carol, &dave, id).await;
        eventually("alice reports it", || forked_at(&alice, id) == Some(epoch)).await;
        let erin = Node::boot_with(identity, dir, port, fast_timings()).await;
        eventually("erin caught up on a branch", || erin.engine.mls_epoch(id) == Some(epoch)).await;
        eventually("erin reports the fork", || forked_at(&erin, id) == Some(epoch)).await;
    })
    .await;
}

/// Joins, a ban and its removal, a banned add with the removal that follows it, a restart, and a quiet spell:
/// nobody reports a fork, though every device sends checks throughout.
#[tokio::test(flavor = "multi_thread")]
async fn no_fork_is_reported_without_a_fork() {
    crate::testutil::bounded(async {
        let (alice, mut others, id) = members(&["bob", "carol"]).await;
        let dave = dave_banned(&alice, &others, id).await;
        {
            let (bob, carol) = (&others[0], &others[1]);
            let add = add_commit(bob, &dave, id);
            let removal = removal(bob, &dave, id);
            // A device's own catch-up can take the removal from a neighbour between the two, and then refuses it here as
            // for a past epoch. The group it ends in is what counts.
            for node in [carol, &alice] {
                let _ = node.engine.apply_commit(id, &add);
                let _ = node.engine.apply_commit(id, &removal);
            }
            eventually("one group again", || {
                let check = alice.engine.own_epoch_check(id);
                check.is_some() && others.iter().all(|n| n.engine.own_epoch_check(id) == check)
            })
            .await;
        }
        let carol = others.pop().expect("carol");
        let (identity, dir, port) = carol.stop().await;
        let carol = Node::boot_with(identity, dir, port, fast_timings()).await;
        others.push(carol);
        eventually("one epoch", || others.iter().all(|n| n.engine.mls_epoch(id) == alice.engine.mls_epoch(id))).await;
        tokio::time::sleep(Duration::from_secs(8)).await;
        for node in std::iter::once(&alice).chain(&others) {
            assert!(crate::test_support::epoch_checks_sent(&node.engine, id) > 0, "checks went out");
            assert_eq!(forked_at(node, id), None, "no fork");
            assert!(node.engine.community_view(id).expect("view").mls_fork.is_none());
        }
    })
    .await;
}

/// A check from a device outside the group, a removed one included, and a check whose signature does not hold,
/// report nothing, whatever value they carry.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_or_a_forged_check_reports_nothing() {
    crate::testutil::bounded(async {
        let (alice, others, id) = members(&["bob"]).await;
        let dave = dave_banned(&alice, &others, id).await;
        let epoch = alice.engine.mls_epoch(id).expect("epoch");
        let from_dave = SignedEpochCheck::sign(id, epoch, [9; 16], 1, &dave.identity.device).expect("sign");
        assert!(alice.engine.accept_epoch_check(id, from_dave).await.is_ok(), "dropped quietly");
        let stranger = Identity::create(1);
        let from_stranger = SignedEpochCheck::sign(id, epoch, [9; 16], 1, &stranger.device).expect("sign");
        assert!(alice.engine.accept_epoch_check(id, from_stranger).await.is_ok());
        let mut forged = SignedEpochCheck::sign(id, epoch, [9; 16], 1, &DeviceKey::generate()).expect("sign");
        forged.device = others[0].identity.device_id();
        assert!(alice.engine.accept_epoch_check(id, forged).await.is_err(), "the signature does not hold");
        assert_eq!(forked_at(&alice, id), None);
    })
    .await;
}

/// Ten checks asked for at once send one, and one more when the gap ends, with the epoch then current.
#[tokio::test(flavor = "multi_thread")]
async fn epoch_checks_are_rate_limited() {
    crate::testutil::bounded(async {
        let (alice, _others, id) = members(&[]).await;
        let before = crate::test_support::epoch_checks_sent(&alice.engine, id);
        tokio::time::sleep(fast_timings().epoch_check_gap).await;
        for _ in 0..10 {
            alice.engine.send_epoch_check(id).await;
        }
        let at_once = crate::test_support::epoch_checks_sent(&alice.engine, id) - before;
        assert!(at_once <= 1, "{at_once} at once");
        tokio::time::sleep(fast_timings().epoch_check_gap * 2).await;
        let after = crate::test_support::epoch_checks_sent(&alice.engine, id) - before;
        assert!((1..=2).contains(&after), "{after} in all");
    })
    .await;
}

/// A released client drops an epoch check as undecodable and goes on with the rest of the topic. alice sends a
/// check that disagrees with everyone, then a message. carol, on this build, reports a fork from it; bob, acting
/// as a released client, reports nothing and still reads the message after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_released_client_drops_only_the_epoch_check_and_reads_on() {
    crate::testutil::bounded(async {
        let (alice, others, id) = members(&["bob", "carol"]).await;
        let (bob, carol) = (&others[0], &others[1]);
        crate::test_support::act_as_released_gossip(&bob.engine);
        let channel = crate::membership::tests::general(&alice.engine, id);
        let epoch = alice.engine.mls_epoch(id).expect("epoch");
        let odd = SignedEpochCheck::sign(id, epoch, [9; 16], crate::engine::now_ms(), &alice.identity.device).expect("sign");
        let sender = alice.engine.read().communities.get(&id).and_then(|c| c.sender.clone()).expect("on the topic");
        let bytes = crate::wire::encode_gossip(&crate::wire::GossipMsg::EpochCheck(Box::new(odd))).expect("encode");
        sender.broadcast(bytes).await.expect("sent");
        alice.engine.send_message(id, channel, "after the check".into()).await.expect("alice sends");
        eventually("carol, on this build, reports it", || forked_at(carol, id) == Some(epoch)).await;
        eventually("bob reads the message after the check", || {
            crate::membership::tests::readable(&bob.engine, id, channel, "after the check")
        })
        .await;
        assert_eq!(forked_at(bob, id), None, "bob dropped the check");
    })
    .await;
}
