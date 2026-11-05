//! A peer that leaves and comes back in one epoch starts sending again from counter 0. The others must hear it.

mod common;

use std::time::Duration;

use common::{auth_set, memory_pair, MemoryTransport};
use dissent_media::{MediaKey, Mesh, Auth};
use tokio::time::{interval, timeout};

type Peer = Mesh<MemoryTransport>;

fn peer(id: u64, auth: &Auth) -> Peer {
    Mesh::new_authenticated(MediaKey::new([4; 32]), id, auth.clone()).unwrap()
}

async fn link(a: &mut Peer, a_id: u64, b: &mut Peer, b_id: u64) {
    let (a_end, b_end) = memory_pair();
    let (offer, token) = a.offer(b_id, a_end).await.unwrap();
    let answer = b.accept(a_id, b_end, offer).await.unwrap();
    a.apply(b_id, token, answer).unwrap();
    let (a_up, b_up) = tokio::join!(a.wait_connected(), b.wait_connected());
    a_up.unwrap();
    b_up.unwrap();
}

/// Sends `frames` frames from `from`, and returns how many `to` got, and how long after the first frame it first played.
async fn talk(from: &mut Peer, to: &mut Peer, frames: u32) -> (u32, Option<Duration>) {
    let start = tokio::time::Instant::now();
    let mut first = None;
    let mut got = 0;
    let mut tick = interval(Duration::from_millis(20));
    let mut sent = 0;
    let quiet = tokio::time::Instant::now() + Duration::from_millis(u64::from(frames) * 20 + 800);
    while tokio::time::Instant::now() < quiet {
        tokio::select! {
            _ = tick.tick() => {
                if sent < frames {
                    from.send_audio_frame(&[0x55; 160]).await.unwrap();
                    sent += 1;
                }
            }
            frame = to.next_received_frame() => {
                if matches!(frame, Ok(Some(_))) {
                    got += 1;
                    first.get_or_insert(start.elapsed());
                }
            }
            _ = from.next_received_frame() => {}
        }
    }
    (got, first)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_comes_back_in_one_epoch_is_heard() {
    timeout(Duration::from_secs(60), async {
        let auth = auth_set(&[1, 2]);
        let mut stays = peer(1, &auth[&1]);
        let mut comes_back = peer(2, &auth[&2]);
        link(&mut stays, 1, &mut comes_back, 2).await;
        let (before, _) = talk(&mut comes_back, &mut stays, 100).await;
        assert!(before >= 90, "heard {before} of 100 before it left");

        stays.remove_peer(2);
        drop(comes_back);
        let mut again = peer(2, &auth[&2]);
        link(&mut stays, 1, &mut again, 2).await;
        let (after, first) = talk(&mut again, &mut stays, 50).await;
        let unheard = first.expect("it was heard");
        println!("unheard for {unheard:?} after it came back");
        // A digest closes after 25 frames, 500 ms, and takes the session over. One lost digest adds another 500 ms.
        assert!(unheard < Duration::from_millis(1_200), "unheard for {unheard:?}");
        assert!(after >= 40, "heard {after} of 50 after it came back, stats {:?}", stays.stats());
    })
    .await
    .expect("in time");
}
