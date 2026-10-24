//! A 1:1 call with sender authentication. Both sides sign their frames and check the other's
//! digests, so every frame verifies over the in-memory link.

mod common;

use std::time::Duration;

use common::{assert_received, auth_set, frame, memory_pair, MemoryTransport, FRAMES};
use dissent_media::{Call, MediaKey, Signal};
use tokio::time::{interval, timeout};

async fn run_call(
    mut call: Call<MemoryTransport>,
    send: u64,
    expect: usize,
) -> (Call<MemoryTransport>, Vec<(u64, Vec<u8>)>) {
    call.wait_connected().await.unwrap();
    let mut ticker = interval(Duration::from_millis(20));
    let mut sent = 0u64;
    let mut received = Vec::new();
    while sent < send || received.len() < expect {
        tokio::select! {
            _ = ticker.tick(), if sent < send => {
                call.send_audio_frame(&frame(sent)).await.unwrap();
                sent += 1;
            }
            next = call.next_frame() => {
                received.push(next.unwrap().expect("call closed early"));
            }
        }
    }
    let _ = timeout(Duration::from_millis(250), async {
        while let Ok(Some(item)) = call.next_frame().await {
            received.push(item);
        }
    })
    .await;
    (call, received)
}

#[tokio::test]
async fn authenticated_one_to_one_call_verifies_every_frame() {
    let auth = auth_set(&[1, 2]);
    let key = MediaKey::new([13; 32]);
    let (caller_end, callee_end) = memory_pair();

    let (caller, offer) = Call::offer_authenticated(caller_end, key.clone(), 1, auth[&1].clone())
        .await
        .unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) = Call::accept_authenticated(callee_end, key, 2, auth[&2].clone(), offer)
        .await
        .unwrap();
    let mut caller = caller;
    caller.apply(answer).unwrap();

    let ((caller, from_callee), (callee, from_caller)) = timeout(Duration::from_secs(30), async {
        tokio::join!(
            run_call(caller, FRAMES, FRAMES as usize),
            run_call(callee, FRAMES, FRAMES as usize),
        )
    })
    .await
    .expect("the call did not finish in time");

    assert_received(&from_callee, 2);
    assert_received(&from_caller, 1);
    for call in [&caller, &callee] {
        let stats = call.stats();
        assert_eq!(stats.frames_unauthenticated, 0, "honest frames must verify");
        assert_eq!(stats.impersonation_detected, 0);
        assert_eq!(stats.frames_replayed, 0);
        assert_eq!(stats.frames_undecryptable, 0);
    }
}
