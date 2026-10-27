//! The devices a members-only relay admits follow the community: a joined member's devices are listed,
//! a stranger's never are, and a banned member's are dropped. Networked: run with scripts/fwtest.sh.

use crate::common::*;
use dissent_core::ModAction;
use std::time::Duration;

#[tokio::test]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn relay_members_follow_joins_and_bans() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let carol = peer("carol").await;
        let community = rustaceans(&alice, &bob).await;

        let bob_device = bob.identity.device_id();
        let carol_device = carol.identity.device_id();
        let alice_device = alice.identity.device_id();

        // Alice's own device and her joined member's device are listed; a stranger's is not.
        eventually("bob's device is listed after joining", || {
            alice.engine.relay_member_devices().contains(&bob_device).then_some(())
        })
        .await;
        assert!(alice.engine.relay_member_devices().contains(&alice_device));
        assert!(!alice.engine.relay_member_devices().contains(&carol_device));

        // A ban drops the banned member's devices from the list.
        alice
            .engine
            .moderate(community, ModAction::Ban, bob.identity.root_id())
            .await
            .expect("ban bob");
        eventually("bob's device is dropped after the ban", || {
            (!alice.engine.relay_member_devices().contains(&bob_device)).then_some(())
        })
        .await;

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

#[tokio::test]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_relay_check_is_answered_by_a_member_and_only_feeds_the_host() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;

        // No relays run in tests, so the member answers that it could not reach the relay. The point is
        // that a member answered at all: the reason is the member's, not "no member answered".
        let reason = alice
            .engine
            .relay_check(community, "https://relay.example.org")
            .await
            .expect_err("no relay runs in tests");
        assert_ne!(reason, "no member answered", "bob should have answered: {reason}");
        assert_ne!(reason, "no other member to run the check");

        finish(vec![alice, bob]).await;
    })
    .await;
}

/// A member in a call answers a relay check as busy, and the host is told to try again later. No probe
/// runs during the call, so a check cannot cause an audio gap.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_member_in_a_call_is_busy_for_a_relay_check() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let community = rustaceans(&alice, &bob).await;
        let url = "https://relay.busy.example";
        alice
            .engine
            .set_hosted_relays(community, vec![url.into()])
            .await
            .expect("publish the relay");

        dissent_core::test_support::pretend_in_call(&bob.engine, true);
        // The publication has to reach bob before he will check it. Until then he refuses it as unpublished.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let reason = loop {
            let reason = alice.engine.relay_check(community, url).await.expect_err("no relay runs in tests");
            if reason == "try again later" || std::time::Instant::now() >= deadline {
                break reason;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        assert_eq!(reason, "try again later", "a busy member is reported as busy, not as a failure");

        dissent_core::test_support::pretend_in_call(&bob.engine, false);
        finish(vec![alice, bob]).await;
    })
    .await;
}
