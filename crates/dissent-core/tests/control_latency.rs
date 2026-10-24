//! Control events reach online members within seconds on loopback. They must not wait for the
//! next anti-entropy round, which runs every 30 s.

mod common;

use common::*;
use dissent_core::ModAction;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn role_grant_reaches_an_online_member_within_five_seconds() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;

    let id = rustaceans(&alice, &bob).await;
    let invite = alice
        .engine
        .create_invite(id)
        .await
        .expect("create invite for carol");
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");

    for p in [&alice, &bob, &carol] {
        eventually("three members everywhere", || {
            (p.engine.community_view(id).ok()?.members.len() == 3).then_some(())
        })
        .await;
    }

    let bob_root = bob.identity.root_id();
    let granted = Instant::now();
    alice
        .engine
        .moderate(id, ModAction::GrantAdmin, bob_root)
        .await
        .expect("alice grants admin");

    let bob_root_text = bob_root.to_string();
    eventually_within(Duration::from_secs(5), "carol sees bob as admin", || {
        carol
            .engine
            .community_view(id)
            .ok()?
            .members
            .iter()
            .find(|m| m.root_id == bob_root_text)
            .filter(|m| m.role == "admin")
            .map(|_| ())
    })
    .await;
    eprintln!("carol saw the grant after {:?}", granted.elapsed());

    finish(vec![alice, bob, carol]).await;
    }).await;
}

/// A grant issued straight after a join, before the new member's topic neighbours have settled.
#[tokio::test(flavor = "multi_thread")]
async fn role_grant_right_after_a_join_still_arrives_within_five_seconds() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let carol = peer("carol").await;

    let id = rustaceans(&alice, &bob).await;
    let invite = alice
        .engine
        .create_invite(id)
        .await
        .expect("create invite for carol");
    carol
        .engine
        .join_community(&invite)
        .await
        .expect("carol joins");

    let carol_root = carol.identity.root_id();
    let granted = Instant::now();
    alice
        .engine
        .moderate(id, ModAction::GrantAdmin, carol_root)
        .await
        .expect("alice grants admin");

    let carol_root_text = carol_root.to_string();
    eventually_within(Duration::from_secs(5), "bob sees carol as admin", || {
        bob.engine
            .community_view(id)
            .ok()?
            .members
            .iter()
            .find(|m| m.root_id == carol_root_text)
            .filter(|m| m.role == "admin")
            .map(|_| ())
    })
    .await;
    eprintln!("bob saw the grant after {:?}", granted.elapsed());

    finish(vec![alice, bob, carol]).await;
    }).await;
}
