//! A call link that dies mid-call is made again by the side that offers for it (`calls::relink`). Networked, so run
//! through `scripts/fwtest.sh dissent-core it --ignored relink::`.

use crate::calls::{all_see, boosted_call, call_summary, root, simulcast_call};
use crate::common::*;
use dissent_core::test_support::{
    act_as_released_calls, close_media_link, drop_leaf_media, drop_mesh_media_to, forwarder_downlinks, leaf_link_live, mesh_call_live,
    relink_attempts,
};
use dissent_core::Topology;
use std::time::{Duration, Instant};

/// Whether `a`'s mesh call to `b` works: connected, and heard lately.
async fn live(a: &Peer, b: &Peer) -> bool {
    mesh_call_live(&a.engine, b.identity.device_id()).await
}

/// Waits until every pair of `peers` has a working call both ways.
async fn all_linked(peers: &[&Peer], within: Duration, what: &str) {
    let deadline = Instant::now() + scaled(within);
    loop {
        let mut down = Vec::new();
        for a in peers {
            for b in peers {
                if root(a) != root(b) && !live(a, b).await {
                    down.push(format!("{}->{}", &root(a)[..6], &root(b)[..6]));
                }
            }
        }
        if down.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "{what}: still down {down:?}\
{}", peers.iter().map(|p| call_summary(p)).collect::<Vec<_>>().join("\
"));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Kills the link between `a` and `b` both ways (`percent` 100) or restores it (0).
fn cut(a: &Peer, b: &Peer, percent: u32) {
    assert!(drop_mesh_media_to(&a.engine, &root(b), percent), "a has b in its call");
    assert!(drop_mesh_media_to(&b.engine, &root(a), percent), "b has a in its call");
}

/// `(offerer, answerer)` of the pair: the lower sender id offers, which is the one that makes relink offers.
fn offerer_of<'a>(a: &'a Peer, b: &'a Peer) -> (&'a Peer, &'a Peer) {
    let ka = dissent_core::test_support::call_key_state(&a.engine).expect("a in a call").2;
    let kb = dissent_core::test_support::call_key_state(&b.engine).expect("b in a call").2;
    if ka < kb {
        (a, b)
    } else {
        (b, a)
    }
}

async fn mesh_of_three() -> Vec<Peer> {
    let (everyone, community, channel) = boosted_call(3, 0, 5_000).await;
    for p in &everyone {
        p.engine.join_call(community, channel).await.expect("join");
    }
    all_see(&everyone, Topology::Mesh, "a mesh of three").await;
    let all: Vec<&Peer> = everyone.iter().collect();
    all_linked(&all, Duration::from_secs(30), "the mesh links up").await;
    everyone
}

/// a, d. A link that goes dead for 8 s comes back on its own once the network does, and only the lower sender id
/// offers for it. The third device keeps hearing both the whole time, and the call stays a mesh.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_link_dead_for_seconds_comes_back_once_the_network_does() {
    heavy(3, async {
        let everyone = mesh_of_three().await;
        let (a, b, c) = (&everyone[0], &everyone[1], &everyone[2]);
        cut(a, b, 100);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(8) {
            assert!(live(c, a).await && live(c, b).await && live(a, c).await && live(b, c).await, "the third device keeps both links");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert!(!live(a, b).await && !live(b, a).await, "the cut link is dead");
        cut(a, b, 0);
        let restored = Instant::now();
        all_linked(&[a, b, c], Duration::from_secs(20), "the cut link comes back").await;
        println!("RELINK back {} ms after the network", restored.elapsed().as_millis());
        let (offerer, answerer) = offerer_of(a, b);
        assert!(relink_attempts(&offerer.engine, &root(answerer)) >= 1, "the lower id offered again");
        assert_eq!(relink_attempts(&answerer.engine, &root(offerer)), 0, "the higher id never offers for the pair");
        // A new call starts a new digest signer: its frames must still verify on the other side. The last batch sent
        // before the cut lost its digest with the cut and counts as unauthenticated once; after that the count stays.
        let unauthenticated = |p: &Peer| -> u64 {
            let debug = dissent_core::test_support::call_debug(&p.engine);
            debug.split("unauth=").skip(1).filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok()).sum()
        };
        tokio::time::sleep(Duration::from_secs(3)).await;
        let settled = (unauthenticated(a), unauthenticated(b));
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!((unauthenticated(a), unauthenticated(b)), settled, "frames on the new call verify: {}", dissent_core::test_support::call_debug(&a.engine));
        all_see(&everyone, Topology::Mesh, "still a mesh").await;
        finish(everyone).await;
    })
    .await;
}

/// b. A link that stays dead is offered for with a growing wait, and the rest of the call is unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_link_that_stays_dead_is_offered_for_less_and_less_often() {
    heavy(3, async {
        let everyone = mesh_of_three().await;
        let (a, b, c) = (&everyone[0], &everyone[1], &everyone[2]);
        cut(a, b, 100);
        tokio::time::sleep(Duration::from_secs(40)).await;
        let (offerer, answerer) = offerer_of(a, b);
        let tries = relink_attempts(&offerer.engine, &root(answerer));
        println!("RELINK {tries} offers in 40 s for a dead link");
        // Fast timings: the first after 2 s, then 2, 4, 8, 8, 8 s apart at most, so six at most in 40 s. A new call is
        // replaced only once its handshake grace is over (`connect_grace`), which spaces them out further.
        assert!((1..=7).contains(&tries), "{tries} offers in 40 s");
        assert_eq!(relink_attempts(&answerer.engine, &root(offerer)), 0);
        assert!(live(c, a).await && live(c, b).await, "the third device still hears both");
        cut(a, b, 0);
        all_linked(&[a, b, c], Duration::from_secs(30), "the link comes back after all").await;
        finish(everyone).await;
    })
    .await;
}

/// c. A media link closed under the call, as by a network change, is made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_closed_media_link_is_made_again() {
    heavy(3, async {
        let everyone = mesh_of_three().await;
        let (a, b, c) = (&everyone[0], &everyone[1], &everyone[2]);
        let (offerer, answerer) = offerer_of(a, b);
        assert!(close_media_link(&offerer.engine, &root(answerer)), "the offerer had a link to close");
        let closed = Instant::now();
        until(Duration::from_secs(10), "the closed link counts as dead", || async { !live(offerer, answerer).await }).await;
        all_linked(&[a, b, c], Duration::from_secs(15), "the closed link is made again").await;
        println!("RELINK closed link back after {} ms", closed.elapsed().as_millis());
        finish(everyone).await;
    })
    .await;
}

/// e. A device that acts as a released build sent no relink feature, so nobody offers again to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn no_relink_offers_go_to_a_released_build() {
    heavy(3, async {
        let (everyone, community, channel) = boosted_call(3, 0, 5_000).await;
        act_as_released_calls(&everyone[2].engine);
        for p in &everyone {
            p.engine.join_call(community, channel).await.expect("join");
        }
        all_see(&everyone, Topology::Mesh, "a mesh of three").await;
        let all: Vec<&Peer> = everyone.iter().collect();
        all_linked(&all, Duration::from_secs(30), "the mesh links up").await;
        let (a, old) = (&everyone[0], &everyone[2]);
        cut(a, old, 100);
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(relink_attempts(&a.engine, &root(old)), 0, "no offer to the released build");
        assert_eq!(relink_attempts(&old.engine, &root(a)), 0, "the released build makes none");
        cut(a, old, 0);
        finish(everyone).await;
    })
    .await;
}

/// f. A leaf whose link to its forwarder closed, as by a network change, links to it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_leaf_whose_forwarder_link_closed_links_again() {
    heavy(7, async {
        let (everyone, forwarder) = simulcast_call(&[]).await;
        let leaf = everyone.iter().enumerate().find(|(i, _)| *i != forwarder).map(|(_, p)| p).expect("a leaf");
        let fwd = &everyone[forwarder];
        until(Duration::from_secs(30), "the leaf's link works", || leaf_link_live(&leaf.engine)).await;
        let before = forwarder_downlinks(&fwd.engine).await;
        assert!(close_media_link(&leaf.engine, &root(fwd)), "the leaf had a link to its forwarder");
        let closed = Instant::now();
        until(Duration::from_secs(10), "the closed link counts as dead", || async { !leaf_link_live(&leaf.engine).await }).await;
        until(Duration::from_secs(20), "the leaf links again", || async {
            leaf_link_live(&leaf.engine).await && forwarder_downlinks(&fwd.engine).await == before
        })
        .await;
        println!("RELINK leaf back after {} ms", closed.elapsed().as_millis());
        finish(everyone).await;
    })
    .await;
}

/// g. A forwarder drops a downlink that has gone silent, and the leaf links again once its network is back.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_forwarder_drops_a_silent_downlink_and_the_leaf_comes_back() {
    heavy(7, async {
        let (everyone, forwarder) = simulcast_call(&[]).await;
        let leaf = everyone.iter().enumerate().find(|(i, _)| *i != forwarder).map(|(_, p)| p).expect("a leaf");
        let fwd = &everyone[forwarder];
        until(Duration::from_secs(30), "the leaf's link works", || leaf_link_live(&leaf.engine)).await;
        let before = forwarder_downlinks(&fwd.engine).await;
        assert!(drop_leaf_media(&leaf.engine, 100));
        until(Duration::from_secs(20), "the forwarder drops the silent downlink", || async {
            forwarder_downlinks(&fwd.engine).await < before
        })
        .await;
        assert!(drop_leaf_media(&leaf.engine, 0));
        until(Duration::from_secs(30), "the leaf links again", || async {
            leaf_link_live(&leaf.engine).await && forwarder_downlinks(&fwd.engine).await == before
        })
        .await;
        finish(everyone).await;
    })
    .await;
}

/// Relink and rejoin together: a device whose link to one peer is dead also strays alone into a room of its own, as a
/// tier move that swapped in with no links does. Relink cannot move it, since its peers are in another room; rejoin
/// brings it back, and the dead pair comes back once the network does. The user's outcome is one mesh with every link
/// working.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_stray_device_with_a_dead_link_ends_up_in_the_majority_mesh() {
    heavy(4, async {
        let (everyone, community, channel) = boosted_call(4, 0, 5_000).await;
        for p in &everyone {
            p.engine.join_call(community, channel).await.expect("join");
        }
        all_see(&everyone, Topology::Mesh, "a mesh of four").await;
        let all: Vec<&Peer> = everyone.iter().collect();
        all_linked(&all, Duration::from_secs(30), "the mesh links up").await;
        let (stray, stayed) = (&everyone[0], &everyone[1]);
        cut(stray, stayed, 100);
        dissent_core::test_support::swap_without_links(&stray.engine, true);
        assert!(dissent_core::test_support::force_tier_move(&stray.engine, true, 0), "the stray's move starts");
        until(Duration::from_secs(60), "the stray swapped in a room of its own", || async {
            dissent_core::test_support::call_debug(&stray.engine).contains("swapped in Forwarded")
        })
        .await;
        dissent_core::test_support::swap_without_links(&stray.engine, false);
        // The stray's own drop went with the room it left; the other side's is still on.
        assert!(drop_mesh_media_to(&stayed.engine, &root(stray), 0));
        assert!(drop_mesh_media_to(&stray.engine, &root(stayed), 0));
        all_see(&everyone, Topology::Mesh, "everyone back in one mesh").await;
        all_linked(&all, Duration::from_secs(60), "every link works again").await;
        finish(everyone).await;
    })
    .await;
}

/// Polls the async `check` every 200 ms for up to `within` (scaled). Panics with `what` on timeout.
async fn until<F, Fut>(within: Duration, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + scaled(within);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
