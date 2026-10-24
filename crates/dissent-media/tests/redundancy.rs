//! Audio redundancy over a lossy link. The caller's transport drops every fifth packet once the
//! call is up, which is 20% loss. With redundancy on, each frame carries the one before it, so a
//! lost frame is usually recovered from the next packet.

mod common;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::{frame, memory_pair, seq_of, MemoryTransport};
use dissent_media::{Call, MediaKey, PacketTransport, Result, Signal};
use tokio::time::{interval, timeout};

/// Drops every fifth packet after the first 200, so the handshake completes first.
struct Lossy {
    inner: MemoryTransport,
    sends: AtomicU64,
}

impl PacketTransport for Lossy {
    fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr()
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> Result<()> {
        let n = self.sends.fetch_add(1, Ordering::Relaxed);
        if n > 200 && n.is_multiple_of(5) {
            return Ok(());
        }
        self.inner.send(to, bytes).await
    }

    async fn recv(&self) -> Result<(SocketAddr, Vec<u8>)> {
        self.inner.recv().await
    }
}

fn lossy(inner: MemoryTransport) -> Lossy {
    Lossy {
        inner,
        sends: AtomicU64::new(0),
    }
}

/// Sends `FRAMES`-odd frames, which is 250, then keeps the call up for a second.
async fn caller_side(mut call: Call<Lossy>) -> Call<Lossy> {
    call.wait_connected().await.unwrap();
    let mut ticker = interval(Duration::from_millis(20));
    let mut sent = 0u64;
    while sent < 250 {
        tokio::select! {
            _ = ticker.tick() => {
                call.send_audio_frame(&frame(sent)).await.unwrap();
                sent += 1;
            }
            _ = call.next_frame() => {}
        }
    }
    let _ = timeout(Duration::from_secs(1), async {
        loop {
            let _ = call.next_frame().await;
        }
    })
    .await;
    call
}

/// Collects the distinct frames the callee plays.
async fn callee_side(mut call: Call<Lossy>) -> BTreeSet<u64> {
    call.wait_connected().await.unwrap();
    let mut seen = BTreeSet::new();
    let _ = timeout(Duration::from_secs(6), async {
        while let Ok(Some((_, payload))) = call.next_frame().await {
            seen.insert(seq_of(&payload));
        }
    })
    .await;
    seen
}

async fn delivered_with(redundancy: Option<bool>) -> (usize, Call<Lossy>) {
    let (caller_end, callee_end) = memory_pair();
    let key = MediaKey::new([27; 32]);
    let (mut caller, offer) = Call::offer_over(lossy(caller_end), key.clone(), 1)
        .await
        .unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) = Call::accept_over(lossy(callee_end), key, 2, offer)
        .await
        .unwrap();
    caller.apply(answer).unwrap();
    caller.set_redundancy(redundancy);

    let (caller, seen) = timeout(Duration::from_secs(60), async {
        tokio::join!(caller_side(caller), callee_side(callee))
    })
    .await
    .expect("the call did not finish in time");
    (seen.len(), caller)
}

#[tokio::test]
async fn redundancy_recovers_frames_that_a_lossy_link_drops() {
    let (without, _) = delivered_with(Some(false)).await;
    let (with, _) = delivered_with(Some(true)).await;
    println!("lossy link (every fifth packet dropped): {without} of 250 frames without redundancy, {with} with it");
    assert!(
        without < 250,
        "the lossy link should drop some frames without redundancy"
    );
    assert!(
        with >= 248,
        "with redundancy only {with} of 250 frames arrived"
    );
    assert!(
        with >= without + 5,
        "redundancy recovered too little: {with} with, {without} without"
    );
}
