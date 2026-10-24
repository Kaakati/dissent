//! Two `Call`s on 127.0.0.1: signalling, ICE and DTLS, then SFrame-protected audio both ways.
//!
//! Every socket binds to loopback only, so Windows Firewall shows no prompt.

use std::net::SocketAddr;
use std::time::Duration;

use dissent_media::{Call, MediaError, MediaKey, Signal};
use tokio::time::{interval, timeout};

const FRAMES: u64 = 50;
const FRAME_LEN: usize = 160;

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn key(byte: u8) -> MediaKey {
    MediaKey::new([byte; 32])
}

/// A synthetic 20 ms audio frame: a little-endian sequence counter, then a pattern derived from it.
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

/// Runs the offer and answer through their byte encoding, as a real transport would.
async fn connect_pair(caller_key: MediaKey, callee_key: MediaKey) -> (Call, Call) {
    let (mut caller, offer) = Call::start_offer(loopback(), caller_key, 1).await.unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();

    let (callee, answer) = Call::accept(loopback(), callee_key, 2, offer)
        .await
        .unwrap();
    let answer = Signal::from_bytes(&answer.to_bytes().unwrap()).unwrap();

    caller.apply(answer).unwrap();
    (caller, callee)
}

/// Waits for the connection, then sends `send` frames at 20 ms cadence while collecting frames
/// until `expect` have arrived. It keeps driving the session for `settle` afterwards, so late
/// packets are still processed.
async fn run_side(
    mut call: Call,
    send: u64,
    expect: usize,
    settle: Duration,
) -> (Call, Vec<(u64, Vec<u8>)>) {
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

    let _ = timeout(settle, async {
        while let Ok(Some(item)) = call.next_frame().await {
            received.push(item);
        }
    })
    .await;

    (call, received)
}

#[tokio::test]
async fn two_calls_exchange_50_encrypted_frames_over_loopback() {
    let (caller, callee) = connect_pair(key(7), key(7)).await;

    let ((caller, from_callee), (callee, from_caller)) = timeout(Duration::from_secs(30), async {
        tokio::join!(
            run_side(caller, FRAMES, FRAMES as usize, Duration::ZERO),
            run_side(callee, FRAMES, FRAMES as usize, Duration::ZERO),
        )
    })
    .await
    .expect("the exchange did not finish in time");

    // Each side delivers every frame, decrypted, attributed to the other sender.
    assert_received(&from_callee, 2);
    assert_received(&from_caller, 1);

    for call in [&caller, &callee] {
        let stats = call.stats();
        assert_eq!(stats.frames_sent, FRAMES);
        assert_eq!(stats.frames_received, FRAMES);
        assert_eq!(stats.frames_undecryptable, 0);
        assert!(stats.packets_received > 0);
    }
}

/// Checks that every frame came from `sender`, decrypts to its own payload, and that all
/// sequence numbers arrived. Arrival order may differ.
fn assert_received(received: &[(u64, Vec<u8>)], sender: u64) {
    assert_eq!(received.len(), FRAMES as usize, "frame count");
    let mut seqs = Vec::with_capacity(received.len());
    for (from, payload) in received {
        assert_eq!(*from, sender, "sender id");
        assert_eq!(payload.len(), FRAME_LEN, "frame length");
        let seq = seq_of(payload);
        assert_eq!(payload, &frame(seq), "payload decrypted incorrectly");
        seqs.push(seq);
    }
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        (0..FRAMES).collect::<Vec<_>>(),
        "missing or duplicated frames"
    );
}

#[tokio::test]
async fn peer_with_wrong_media_key_counts_frames_as_undecryptable() {
    let (caller, callee) = connect_pair(key(1), key(2)).await;

    let ((_caller, _), (callee, delivered)) = timeout(Duration::from_secs(30), async {
        tokio::join!(
            run_side(caller, FRAMES, 0, Duration::from_secs(3)),
            run_side(callee, 0, 0, Duration::from_secs(3)),
        )
    })
    .await
    .expect("the exchange did not finish in time");

    assert!(
        delivered.is_empty(),
        "a frame decrypted under the wrong key"
    );
    let stats = callee.stats();
    assert!(stats.packets_received > 0, "the packets did arrive");
    assert_eq!(stats.frames_received, 0);
    assert_eq!(stats.frames_undecryptable, FRAMES);
}

#[tokio::test]
async fn sending_before_the_connection_completes_is_refused() {
    let (mut caller, _callee) = connect_pair(key(3), key(3)).await;
    let err = caller.send_audio_frame(&frame(0)).await.unwrap_err();
    assert!(matches!(err, MediaError::NotConnected), "got {err:?}");
}
