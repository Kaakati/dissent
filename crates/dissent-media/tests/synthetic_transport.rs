//! A call over a transport with no IP path of its own. Datagrams travel through in-memory
//! channels, and every peer address is one of the synthetic addresses. This is the same shape
//! the iroh transport takes: str0m sees only the synthetic addresses.

use std::net::SocketAddr;
use std::time::Duration;

use dissent_media::{
    Call, MediaError, MediaKey, PacketTransport, Signal, SYNTHETIC_CALLEE, SYNTHETIC_CALLER,
};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, timeout};

const FRAMES: u64 = 50;
const FRAME_LEN: usize = 160;

/// One side of an in-memory datagram link. `local` is what str0m sees, and `peer` is the address
/// every received datagram is reported from.
struct MemoryTransport {
    local: SocketAddr,
    peer: SocketAddr,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
}

/// Two connected transports, one per side of the call.
fn memory_pair() -> (MemoryTransport, MemoryTransport) {
    let (a_tx, b_rx) = mpsc::unbounded_channel();
    let (b_tx, a_rx) = mpsc::unbounded_channel();
    let a = MemoryTransport {
        local: SYNTHETIC_CALLER,
        peer: SYNTHETIC_CALLEE,
        tx: a_tx,
        rx: Mutex::new(a_rx),
    };
    let b = MemoryTransport {
        local: SYNTHETIC_CALLEE,
        peer: SYNTHETIC_CALLER,
        tx: b_tx,
        rx: Mutex::new(b_rx),
    };
    (a, b)
}

impl PacketTransport for MemoryTransport {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> dissent_media::Result<()> {
        // A single-peer transport ignores the destination, but it must be the peer str0m was given.
        assert_eq!(
            to, self.peer,
            "str0m sent to an address that is not the peer"
        );
        self.tx.send(bytes.to_vec()).map_err(|_| MediaError::Closed)
    }

    async fn recv(&self) -> dissent_media::Result<(SocketAddr, Vec<u8>)> {
        let bytes = self
            .rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(MediaError::Closed)?;
        Ok((self.peer, bytes))
    }
}

fn key(byte: u8) -> MediaKey {
    MediaKey::new([byte; 32])
}

fn frame(seq: u64) -> Vec<u8> {
    let mut out = vec![0u8; FRAME_LEN];
    out[..8].copy_from_slice(&seq.to_le_bytes());
    for (i, byte) in out[8..].iter_mut().enumerate() {
        *byte = (seq as u8).wrapping_mul(31).wrapping_add(i as u8);
    }
    out
}

fn seq_of(frame: &[u8]) -> u64 {
    u64::from_le_bytes(frame[..8].try_into().unwrap())
}

async fn run_side(
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
                received.push(next.unwrap().expect("call closed before all frames arrived"));
            }
        }
    }
    let _ = timeout(Duration::from_millis(200), async {
        while let Ok(Some(item)) = call.next_frame().await {
            received.push(item);
        }
    })
    .await;
    (call, received)
}

#[tokio::test]
async fn call_over_synthetic_addresses_delivers_encrypted_frames_both_ways() {
    let (a_link, b_link) = memory_pair();

    let (caller, offer) = Call::offer_over(a_link, key(5), 1).await.unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) = Call::accept_over(b_link, key(5), 2, offer).await.unwrap();
    let answer = Signal::from_bytes(&answer.to_bytes().unwrap()).unwrap();
    let mut caller = caller;
    caller.apply(answer).unwrap();

    assert_eq!(caller.local_addr(), SYNTHETIC_CALLER);
    assert_eq!(callee.local_addr(), SYNTHETIC_CALLEE);

    let ((_caller, from_callee), (_callee, from_caller)) =
        timeout(Duration::from_secs(30), async {
            tokio::join!(
                run_side(caller, FRAMES, FRAMES as usize),
                run_side(callee, FRAMES, FRAMES as usize),
            )
        })
        .await
        .expect("the exchange did not finish in time");

    assert_received(&from_callee, 2);
    assert_received(&from_caller, 1);
}

fn assert_received(received: &[(u64, Vec<u8>)], sender: u64) {
    assert_eq!(received.len(), FRAMES as usize, "frame count");
    let mut seqs = Vec::with_capacity(received.len());
    for (from, payload) in received {
        assert_eq!(*from, sender, "sender id");
        assert_eq!(
            payload,
            &frame(seq_of(payload)),
            "payload decrypted incorrectly"
        );
        seqs.push(seq_of(payload));
    }
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        (0..FRAMES).collect::<Vec<_>>(),
        "missing or duplicated frames"
    );
}
