//! Guardian recovery end to end: the owner names two guardians, loses the device, and a new device asks them. One
//! declines, then approves; the other approves with the code read to them. The answers reach the new device by
//! themselves, with the guardian list and an invite, and the new device finishes the rotation.

use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn guardians_answer_a_recovery_and_the_new_device_finishes_it() {
    bounded(async {
        let alice = peer("alice").await;
        let gina = peer("gina").await;
        let id = rustaceans(&alice, &gina).await;
        let gus = peer("gus").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &gus, &invite).await;
        let alice_root = alice.identity.root_id();
        let (gina_root, gus_root) = (gina.identity.root_id(), gus.identity.root_id());

        alice.engine.set_guardians(vec![gina_root, gus_root], 2).await.expect("alice names her guardians");

        // Alice loses her device. A new one, with a fresh account, starts the recovery.
        let fresh = peer("alice-new").await;
        let started = fresh.engine.begin_recovery(&alice_root.to_string()).expect("recovery starts");
        assert!(started.request_link.contains("?via="), "the link names the new device");
        assert!(!started.set_known && started.approvals == 0 && !started.ready);

        // Gina holds Alice's list, so she sees she is on it, and the server they share.
        let seen = eventually("gina keeps alice's guardian list", || {
            gina.engine.inspect_recovery_request(&started.request_link).ok().filter(|v| v.listed == Some(true))
        })
        .await;
        assert_eq!(seen.shared_servers, vec!["Rustaceans".to_string()]);
        assert!(seen.reachable);

        // A wrong code is refused; the right one approves, and the answer arrives by itself.
        let wrong = gina.engine.answer_recovery_request(&started.request_link, true, Some("000 000 000"), true).await;
        assert!(wrong.is_err(), "an approval needs the code the person reads out");
        let sent = gina
            .engine
            .answer_recovery_request(&started.request_link, true, Some(&started.code), true)
            .await
            .expect("gina approves");
        assert!(sent.delivered, "{sent:?}");
        assert!(sent.approval_link.is_some_and(|l| l.starts_with("dissent://approve/")));
        let after_gina = eventually("the new device has gina's approval", || {
            fresh.engine.recovery().ok().flatten().filter(|v| v.approvals == 1)
        })
        .await;
        assert!(after_gina.set_known, "the list came with the approval");
        assert_eq!(after_gina.threshold, Some(2));
        assert!(after_gina.invites >= 1, "gina sent an invite to the server they share");
        assert!(!after_gina.ready, "one of two is not enough");

        // Gus declines first, signed, and the new device shows it. Then he approves.
        let declined = gus.engine.answer_recovery_request(&started.request_link, false, None, false).await.expect("gus declines");
        assert!(declined.delivered && declined.approval_link.is_none());
        eventually("the new device shows gus declined", || {
            fresh
                .engine
                .recovery()
                .ok()
                .flatten()
                .filter(|v| v.guardians.iter().any(|g| g.root_id == gus_root.to_string() && g.state == "declined"))
        })
        .await;
        gus.engine
            .answer_recovery_request(&started.request_link, true, Some(&started.code), false)
            .await
            .expect("gus approves");
        let ready = eventually("two of two approved", || fresh.engine.recovery().ok().flatten().filter(|v| v.ready)).await;
        assert!(ready.guardians.iter().all(|g| g.state == "approved"));

        let published = fresh.engine.finish_recovery().await.expect("the new device finishes the recovery");
        assert!(published >= 1, "the rotation went out in the shared server");
        assert!(fresh.engine.recovery().expect("status").is_none(), "nothing is pending afterwards");

        finish(vec![alice, gina, gus, fresh]).await;
    })
    .await;
}

// An answer for another request, or one that does not verify, is refused; a pasted approval link works the same as a
// delivered one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_pasted_approval_counts_and_a_wrong_request_is_refused() {
    bounded(async {
        let alice = peer("alice").await;
        let gina = peer("gina").await;
        rustaceans(&alice, &gina).await;
        let alice_root = alice.identity.root_id();
        alice.engine.set_guardians(vec![gina.identity.root_id()], 1).await.expect("guardians");
        let share = alice.engine.guardians().expect("view").share_link.expect("share link");

        let fresh = peer("alice-new").await;
        let started = fresh.engine.begin_recovery(&share).expect("start from the guardians link");
        assert!(started.set_known && started.threshold == Some(1));

        // An approval made for a link without the address goes back by hand.
        let bare = started.request_link.split("?via=").next().unwrap().to_string();
        let answer = gina.engine.answer_recovery_request(&bare, true, Some(&started.code), false).await.expect("approve");
        assert!(!answer.delivered);
        let link = answer.approval_link.expect("approval link");

        let other = peer("other-new").await;
        other.engine.begin_recovery(&alice_root.to_string()).expect("another recovery");
        assert!(other.engine.add_recovery_link(&link).is_err(), "an approval for another request is refused");

        let view = fresh.engine.add_recovery_link(&link).expect("the pasted approval counts");
        assert!(view.ready);
        fresh.engine.cancel_recovery().expect("cancel");
        assert!(fresh.engine.recovery().expect("status").is_none());

        finish(vec![alice, gina, fresh, other]).await;
    })
    .await;
}
