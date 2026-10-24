//! Helpers shared by the integration tests: synthetic frames and the send-and-collect driver.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use dissent_media::{Call, Mesh, PacketTransport};
use tokio::time::{interval, timeout};

pub const FRAMES: u64 = 50;
pub const FRAME_LEN: usize = 160;

/// A synthetic 20 ms audio frame: a little-endian sequence counter, then a pattern derived from it.
pub fn frame(seq: u64) -> Vec<u8> {
    let mut out = vec![0u8; FRAME_LEN];
    out[..8].copy_from_slice(&seq.to_le_bytes());
    for (i, byte) in out[8..].iter_mut().enumerate() {
        *byte = (seq as u8).wrapping_mul(31).wrapping_add(i as u8);
    }
    out
}

pub fn seq_of(frame: &[u8]) -> u64 {
    u64::from_le_bytes(frame[..8].try_into().unwrap())
}

/// Waits for the connection, then sends `send` frames at 20 ms cadence while collecting frames
/// until `expect` have arrived. It keeps driving the session for 200 ms afterwards, so late
/// packets are still processed.
pub async fn run_side<T: PacketTransport>(
    mut call: Call<T>,
    send: u64,
    expect: usize,
) -> (Call<T>, Vec<(u64, Vec<u8>)>) {
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

/// Checks that every frame came from `sender`, decrypts to its own payload, and that all
/// sequence numbers arrived. Arrival order may differ.
pub fn assert_received(received: &[(u64, Vec<u8>)], sender: u64) {
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

/// One end of an in-memory datagram link. `local` is what str0m sees, and `peer` is the address
/// every received datagram is reported from.
pub struct MemoryTransport {
    local: SocketAddr,
    peer: SocketAddr,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
}

/// Two connected in-memory transports, the caller's end first.
pub fn memory_pair() -> (MemoryTransport, MemoryTransport) {
    use dissent_media::{SYNTHETIC_CALLEE, SYNTHETIC_CALLER};
    let (a_tx, b_rx) = tokio::sync::mpsc::unbounded_channel();
    let (b_tx, a_rx) = tokio::sync::mpsc::unbounded_channel();
    let a = MemoryTransport {
        local: SYNTHETIC_CALLER,
        peer: SYNTHETIC_CALLEE,
        tx: a_tx,
        rx: tokio::sync::Mutex::new(a_rx),
    };
    let b = MemoryTransport {
        local: SYNTHETIC_CALLEE,
        peer: SYNTHETIC_CALLER,
        tx: b_tx,
        rx: tokio::sync::Mutex::new(b_rx),
    };
    (a, b)
}

impl PacketTransport for MemoryTransport {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    async fn send(&self, to: SocketAddr, bytes: &[u8]) -> dissent_media::Result<()> {
        assert_eq!(
            to, self.peer,
            "str0m sent to an address that is not the peer"
        );
        self.tx
            .send(bytes.to_vec())
            .map_err(|_| dissent_media::MediaError::Closed)
    }

    async fn recv(&self) -> dissent_media::Result<(SocketAddr, Vec<u8>)> {
        let bytes = self
            .rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(dissent_media::MediaError::Closed)?;
        Ok((self.peer, bytes))
    }
}

/// Drives one mesh participant. It waits for every peer to connect, then sends `send` frames at
/// 20 ms cadence and collects frames until `expect` have arrived. It keeps driving for 200 ms
/// afterwards, so late packets are still processed.
pub async fn run_mesh_peer<T: PacketTransport>(
    mut mesh: Mesh<T>,
    send: u64,
    expect: usize,
) -> (Mesh<T>, Vec<(u64, Vec<u8>)>) {
    mesh.wait_connected().await.unwrap();

    let mut ticker = interval(Duration::from_millis(20));
    let mut sent = 0u64;
    let mut received = Vec::new();
    while sent < send || received.len() < expect {
        tokio::select! {
            _ = ticker.tick(), if sent < send => {
                mesh.send_audio_frame(&frame(sent)).await.unwrap();
                sent += 1;
            }
            next = mesh.next_frame() => {
                received.push(next.unwrap().expect("mesh emptied before all frames arrived"));
            }
        }
    }
    let _ = timeout(Duration::from_millis(200), async {
        while let Ok(Some(item)) = mesh.next_frame().await {
            received.push(item);
        }
    })
    .await;
    (mesh, received)
}

/// Checks that frames came only from `senders`, that each sender delivered exactly `frames`
/// frames with sequence numbers `0..frames`, and that every payload decrypted correctly.
pub fn assert_from_each(received: &[(u64, Vec<u8>)], senders: &[u64], frames: u64) {
    let mut by_sender: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (from, payload) in received {
        assert_eq!(payload.len(), FRAME_LEN, "frame length");
        let seq = seq_of(payload);
        assert_eq!(payload, &frame(seq), "payload decrypted incorrectly");
        by_sender.entry(*from).or_default().push(seq);
    }
    let got: Vec<u64> = by_sender.keys().copied().collect();
    assert_eq!(got, senders, "frames came from unexpected senders");
    for sender in senders {
        let mut seqs = by_sender[sender].clone();
        seqs.sort_unstable();
        assert_eq!(seqs, (0..frames).collect::<Vec<_>>(), "sender {sender}");
    }
}

/// Marks the plaintext of a sentinel frame. A forwarder that sees this in its input has seen
/// plaintext.
pub const SENTINEL: &[u8; 16] = b"PLAINTEXT-LEAK!!";

/// A 160-byte frame with a sequence number, the sender id, and a repeated sentinel. The sentinel
/// is what the leak check looks for.
pub fn sentinel_frame(seq: u64, sender: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_LEN);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&sender.to_le_bytes());
    while out.len() < FRAME_LEN {
        out.extend_from_slice(SENTINEL);
    }
    out.truncate(FRAME_LEN);
    out
}

/// One frame a participant received.
#[derive(Debug, Clone, Copy)]
pub struct Arrival {
    pub at: std::time::Instant,
    pub sender: u64,
    /// Whether the payload matches the sender's frame byte for byte.
    pub intact: bool,
}

/// Runs one forwarded participant. It sends a sentinel frame every 20 ms at `level` for
/// `run_for`, and records every frame it receives.
pub async fn run_participant<T: PacketTransport>(
    mut participant: dissent_media::Participant<T>,
    id: u64,
    level: u8,
    run_for: Duration,
) -> (dissent_media::Participant<T>, Vec<Arrival>) {
    participant.wait_connected().await.unwrap();

    let end = tokio::time::sleep(run_for);
    tokio::pin!(end);
    let mut ticker = interval(Duration::from_millis(20));
    let mut seq = 0u64;
    let mut arrivals = Vec::new();
    loop {
        tokio::select! {
            _ = &mut end => break,
            _ = ticker.tick() => {
                participant.send_audio_frame(&sentinel_frame(seq, id), level).await.unwrap();
                seq += 1;
            }
            next = participant.next_frame() => match next.unwrap() {
                Some((sender, payload)) => arrivals.push(Arrival {
                    at: std::time::Instant::now(),
                    sender,
                    intact: payload == sentinel_frame(seq_of(&payload), sender),
                }),
                None => break,
            },
        }
    }
    // Keep driving for a moment, so the digest that closes the last batch goes out.
    let _ = timeout(Duration::from_millis(250), async {
        while let Ok(Some((sender, payload))) = participant.next_frame().await {
            arrivals.push(Arrival {
                at: std::time::Instant::now(),
                sender,
                intact: payload == sentinel_frame(seq_of(&payload), sender),
            });
        }
    })
    .await;
    (participant, arrivals)
}

/// One device key and certificate per member, certified by one root. Each member's `Auth` holds
/// its own device key and every member's certificate.
pub fn auth_set(ids: &[u64]) -> BTreeMap<u64, dissent_media::Auth> {
    let root = dissent_identity::RootKey::generate();
    let devices: BTreeMap<u64, dissent_identity::DeviceKey> = ids
        .iter()
        .map(|id| (*id, dissent_identity::DeviceKey::generate()))
        .collect();
    let certs: std::collections::HashMap<u64, dissent_identity::DeviceCert> = devices
        .iter()
        .map(|(id, device)| (*id, root.certify(device.id(), 0)))
        .collect();
    devices
        .into_iter()
        .map(|(id, device)| (id, dissent_media::Auth::new(device, certs.clone())))
        .collect()
}
