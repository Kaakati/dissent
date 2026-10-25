//! Opt-in discovery of community members on the same local network.
//!
//! A device multicasts a beacon every few seconds. The beacon carries a fresh random nonce and one
//! token per community key: a keyed hash of the ten-minute window and the nonce, under a key only
//! that community's members hold. Every beacon has its own nonce, so an observer cannot link two
//! beacons, even from one device, and cannot tell which community they belong to.
//!
//! A device learns another device's id only after a handshake with mutual proof of the community
//! key. Each side answers a fresh challenge from the other, so a recorded message cannot be
//! replayed to reveal an id:
//!
//! ```text
//! A -> multicast  Beacon { nonce, tokens }
//! B -> A          Challenge { nonce, token, nonce_b, mac(key, nonce_b) }   B proves the key
//! A -> B          Hello { device_a, port_a, nonce_a, mac(key, nonce_b, device_a, nonce_a) }
//! B -> A          Reply { device_b, port_b, mac(key, nonce_a, device_b) }
//! ```
//!
//! A learns B's id only after B's challenge verifies, and B learns A's only after A's Hello
//! verifies. Each side then adds the other to the LAN address lookup, so the endpoint dials the
//! IP address directly.
//!
//! Off by default. Never runs in loopback test mode, because it binds a multicast socket.

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dissent_identity::DeviceId;
use iroh::address_lookup::MemoryLookup;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::debug;

use crate::addr::PeerAddr;

/// Beacons go out this often.
const BEACON_EVERY: Duration = Duration::from_secs(5);

/// UDP port for beacons and handshakes.
pub const LAN_PORT: u16 = 47320;
/// Multicast group for beacons. Administratively scoped, so it stays on the local network.
pub const LAN_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
/// Tokens change every ten minutes. Peers accept the current and the previous window, so a
/// beacon sent just before a boundary still matches.
pub const WINDOW_MS: u64 = 10 * 60 * 1000;
/// Most tokens one beacon carries.
const MAX_TOKENS: usize = 16;
/// A handshake that has not finished within this many milliseconds is dropped.
const HANDSHAKE_TTL_MS: u64 = 10_000;
const TOKEN_DOMAIN: &[u8] = b"dissent/lan/v1";

/// A rotating token from a beacon.
pub type Token = [u8; 16];

/// The LAN key of a community, derived from its secret. Only members hold it, so only members
/// can make or check a token.
pub fn community_key(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("dissent/lan/v1 community key", secret)
}

/// The token for `key` in `window`, for a beacon that carries `nonce`.
pub(crate) fn token(key: &[u8; 32], window: u64, nonce: &[u8; 16]) -> Token {
    let mut msg = Vec::with_capacity(TOKEN_DOMAIN.len() + 8 + nonce.len());
    msg.extend_from_slice(TOKEN_DOMAIN);
    msg.extend_from_slice(&window.to_be_bytes());
    msg.extend_from_slice(nonce);
    let hash = blake3::keyed_hash(key, &msg);
    let mut out = [0u8; 16];
    out.copy_from_slice(&hash.as_bytes()[..16]);
    out
}

fn window_of(now_ms: u64) -> u64 {
    now_ms / WINDOW_MS
}

/// The key that makes `t` for a beacon with `nonce`, in the current or the previous window.
fn key_for(keys: &[[u8; 32]], t: &Token, nonce: &[u8; 16], now_ms: u64) -> Option<[u8; 32]> {
    let w = window_of(now_ms);
    let windows = [w, w.saturating_sub(1)];
    keys.iter()
        .find(|k| windows.iter().any(|w| token(k, *w, nonce) == *t))
        .copied()
}

/// What goes over the wire. Postcard encoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LanMsg {
    Beacon { nonce: [u8; 16], tokens: Vec<Token> },
    Challenge { beacon: [u8; 16], token: Token, nonce: [u8; 16], mac: [u8; 32] },
    Hello { device: DeviceId, port: u16, nonce: [u8; 16], mac: [u8; 32] },
    Reply { device: DeviceId, port: u16, mac: [u8; 32] },
}

/// A device that completed a handshake. `addr` is its IP address and the iroh endpoint's port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub device: DeviceId,
    pub addr: SocketAddr,
}

/// What handling one message produced: replies to send and a device that is now known.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub send: Vec<(SocketAddr, LanMsg)>,
    pub found: Option<Found>,
}

/// A handshake step waiting for its answer.
struct Pending {
    key: [u8; 32],
    nonce: [u8; 16],
    at_ms: u64,
}

/// The handshake state of one device. Pure: it takes messages and returns messages, so tests
/// drive two of them directly.
pub(crate) struct Lan {
    device: DeviceId,
    port: u16,
    keys: Vec<[u8; 32]>,
    /// Challenges we sent, awaiting the peer's Hello.
    issued: HashMap<SocketAddr, Pending>,
    /// Hellos we sent, awaiting the peer's Reply.
    sent: HashMap<SocketAddr, Pending>,
}

fn mac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

fn fresh_nonce() -> [u8; 16] {
    rand::random()
}

impl Lan {
    /// `device` is this device's id and `port` the port of its iroh endpoint.
    pub fn new(device: DeviceId, port: u16) -> Self {
        Self {
            device,
            port,
            keys: Vec::new(),
            issued: HashMap::new(),
            sent: HashMap::new(),
        }
    }

    /// Replaces the community keys this device answers for.
    pub fn set_keys(&mut self, keys: Vec<[u8; 32]>) {
        self.keys = keys;
    }

    /// A beacon for `now_ms`: a fresh nonce and one token per community key, capped at `MAX_TOKENS`.
    pub fn beacon(&self, now_ms: u64) -> LanMsg {
        let w = window_of(now_ms);
        let nonce = fresh_nonce();
        let tokens = self
            .keys
            .iter()
            .take(MAX_TOKENS)
            .map(|k| token(k, w, &nonce))
            .collect();
        LanMsg::Beacon { nonce, tokens }
    }

    /// Handles one message from `src`. Malformed or unverifiable messages are ignored.
    pub fn handle(&mut self, src: SocketAddr, msg: LanMsg, now_ms: u64) -> Outcome {
        self.expire(now_ms);
        let mut out = Outcome::default();
        match msg {
            LanMsg::Beacon { nonce: beacon, tokens } => {
                let matched = tokens
                    .iter()
                    .find_map(|t| key_for(&self.keys, t, &beacon, now_ms).map(|k| (*t, k)));
                if let Some((token, key)) = matched {
                    let nonce = fresh_nonce();
                    let proof = mac(&key, &[b"challenge", &token, &nonce]);
                    self.issued.insert(src, Pending { key, nonce, at_ms: now_ms });
                    out.send.push((
                        src,
                        LanMsg::Challenge {
                            beacon,
                            token,
                            nonce,
                            mac: proof,
                        },
                    ));
                }
            }
            LanMsg::Challenge { beacon, token, nonce, mac: proof } => {
                let Some(key) = key_for(&self.keys, &token, &beacon, now_ms) else {
                    return out;
                };
                if mac(&key, &[b"challenge", &token, &nonce]) != proof {
                    return out;
                }
                let ours = fresh_nonce();
                let hello_mac = mac(
                    &key,
                    &[b"hello", &nonce, self.device.0.as_slice(), &self.port.to_be_bytes(), &ours],
                );
                self.sent.insert(src, Pending { key, nonce: ours, at_ms: now_ms });
                out.send.push((
                    src,
                    LanMsg::Hello {
                        device: self.device,
                        port: self.port,
                        nonce: ours,
                        mac: hello_mac,
                    },
                ));
            }
            LanMsg::Hello { device, port, nonce, mac: proof } => {
                let Some(pending) = self.issued.remove(&src) else {
                    return out;
                };
                let expected = mac(
                    &pending.key,
                    &[b"hello", &pending.nonce, device.0.as_slice(), &port.to_be_bytes(), &nonce],
                );
                if expected != proof || device == self.device {
                    return out;
                }
                let reply = mac(&pending.key, &[b"reply", &nonce, self.device.0.as_slice(), &self.port.to_be_bytes()]);
                out.send.push((
                    src,
                    LanMsg::Reply {
                        device: self.device,
                        port: self.port,
                        mac: reply,
                    },
                ));
                out.found = Some(Found {
                    device,
                    addr: SocketAddr::new(src.ip(), port),
                });
            }
            LanMsg::Reply { device, port, mac: proof } => {
                let Some(pending) = self.sent.remove(&src) else {
                    return out;
                };
                let expected = mac(
                    &pending.key,
                    &[b"reply", &pending.nonce, device.0.as_slice(), &port.to_be_bytes()],
                );
                if expected != proof || device == self.device {
                    return out;
                }
                out.found = Some(Found {
                    device,
                    addr: SocketAddr::new(src.ip(), port),
                });
            }
        }
        out
    }

    /// Drops handshakes that did not finish in time.
    fn expire(&mut self, now_ms: u64) {
        let alive = |p: &Pending| now_ms.saturating_sub(p.at_ms) < HANDSHAKE_TTL_MS;
        self.issued.retain(|_, p| alive(p));
        self.sent.retain(|_, p| alive(p));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs the LAN side of a node until the socket fails. Each device that completes a handshake is
/// added to `found`, the LAN address lookup, so the endpoint can dial it.
pub(crate) async fn run(
    socket: UdpSocket,
    mut lan: Lan,
    keys: Arc<Mutex<Vec<[u8; 32]>>>,
    found: MemoryLookup,
) {
    let group = SocketAddr::from((LAN_GROUP, LAN_PORT));
    let mut tick = tokio::time::interval(BEACON_EVERY);
    let mut buf = vec![0u8; 2048];
    loop {
        let snapshot = keys.lock().unwrap_or_else(PoisonError::into_inner).clone();
        lan.set_keys(snapshot);
        tokio::select! {
            _ = tick.tick() => {
                send(&socket, group, &lan.beacon(now_ms())).await;
            }
            recv = socket.recv_from(&mut buf) => {
                let (n, src) = match recv {
                    Ok(received) => received,
                    Err(e) => {
                        debug!(error = %e, "LAN socket stopped");
                        break;
                    }
                };
                let Ok(msg) = postcard::from_bytes::<LanMsg>(&buf[..n]) else {
                    continue;
                };
                let out = lan.handle(src, msg, now_ms());
                for (dst, reply) in &out.send {
                    send(&socket, *dst, reply).await;
                }
                if let Some(Found { device, addr }) = out.found {
                    if let Ok(peer) = (PeerAddr { id: device, relay: None, direct: vec![addr] }).to_iroh() {
                        found.add_endpoint_info(peer);
                    }
                }
            }
        }
    }
}

async fn send(socket: &UdpSocket, dst: SocketAddr, msg: &LanMsg) {
    match postcard::to_allocvec(msg) {
        Ok(bytes) => {
            if let Err(e) = socket.send_to(&bytes, dst).await {
                debug!(error = %e, "LAN message not sent");
            }
        }
        Err(e) => debug!(error = %e, "LAN message not encoded"),
    }
}

/// Binds the LAN socket and joins the beacon group. The socket is not looped back, so a device
/// never sees its own beacon.
pub(crate) fn bind() -> std::io::Result<UdpSocket> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, LAN_PORT))?;
    socket.join_multicast_v4(&LAN_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(false)?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;

    /// Each device sits on its own host, so the two handshake sides never share an address.
    fn host(last: u8) -> SocketAddr {
        SocketAddr::from(([192, 168, 1, last], LAN_PORT))
    }

    fn device(seed: u8) -> DeviceId {
        DeviceId([seed; 32])
    }

    fn key(seed: u8) -> [u8; 32] {
        community_key(&[seed; 32])
    }

    /// Runs the whole handshake from `a`'s beacon until no message is left. Returns what each
    /// side learned: `(a learned, b learned)`.
    fn handshake(a: &mut Lan, a_addr: SocketAddr, b: &mut Lan, b_addr: SocketAddr) -> (Option<Found>, Option<Found>) {
        let mut found_a = None;
        let mut found_b = None;
        let mut queue: Vec<(SocketAddr, SocketAddr, LanMsg)> = vec![(a_addr, b_addr, a.beacon(NOW))];
        let mut hops = 0;
        while let Some((from, to, msg)) = queue.pop() {
            hops += 1;
            assert!(hops < 10, "handshake did not settle");
            let out = if to == b_addr {
                b.handle(from, msg, NOW)
            } else {
                a.handle(from, msg, NOW)
            };
            if to == b_addr {
                found_b = found_b.or(out.found.clone());
            } else {
                found_a = found_a.or(out.found.clone());
            }
            for (dst, reply) in out.send {
                queue.push((to, dst, reply));
            }
        }
        (found_a, found_b)
    }

    #[test]
    fn members_learn_each_others_device_and_address() {
        let k = key(1);
        let mut a = Lan::new(device(1), 4433);
        let mut b = Lan::new(device(2), 5544);
        a.set_keys(vec![k]);
        b.set_keys(vec![k]);
        let (found_a, found_b) = handshake(&mut a, host(10), &mut b, host(11));
        assert_eq!(found_b, Some(Found { device: device(1), addr: SocketAddr::new(host(10).ip(), 4433) }));
        assert_eq!(found_a, Some(Found { device: device(2), addr: SocketAddr::new(host(11).ip(), 5544) }));
    }

    #[test]
    fn a_stranger_with_another_community_learns_nothing() {
        let mut a = Lan::new(device(1), 4433);
        let mut stranger = Lan::new(device(3), 6655);
        a.set_keys(vec![key(1)]);
        stranger.set_keys(vec![key(9)]);
        let (found_a, found_stranger) = handshake(&mut a, host(10), &mut stranger, host(11));
        assert_eq!(found_a, None);
        assert_eq!(found_stranger, None);
    }

    #[test]
    fn beacon_tokens_change_with_the_window() {
        let k = key(1);
        let mut a = Lan::new(device(1), 4433);
        a.set_keys(vec![k]);
        let now = a.beacon(NOW);
        let later = a.beacon(NOW + WINDOW_MS);
        assert_ne!(now, later);
        // A beacon from the previous window still matches, so a boundary does not lose peers.
        let mut b = Lan::new(device(2), 5544);
        b.set_keys(vec![k]);
        let out = b.handle(host(10), now, NOW + WINDOW_MS);
        assert_eq!(out.send.len(), 1, "previous window is accepted");
    }

    #[test]
    fn tokens_depend_on_the_key_the_window_and_the_nonce() {
        let w = window_of(NOW);
        let n = [7u8; 16];
        assert_ne!(token(&key(1), w, &n), token(&key(2), w, &n));
        assert_ne!(token(&key(1), w, &n), token(&key(1), w + 2, &n));
        assert_ne!(token(&key(1), w, &n), token(&key(1), w, &[8u8; 16]));
    }

    #[test]
    fn beacons_do_not_repeat_their_tokens() {
        let mut a = Lan::new(device(1), 4433);
        a.set_keys(vec![key(1)]);
        let (LanMsg::Beacon { nonce: n1, tokens: t1 }, LanMsg::Beacon { nonce: n2, tokens: t2 }) =
            (a.beacon(NOW), a.beacon(NOW))
        else {
            panic!("expected beacons");
        };
        assert_ne!(n1, n2, "each beacon has a fresh nonce");
        assert_ne!(t1, t2, "so its tokens differ from the last beacon's");
    }

    #[test]
    fn a_recorded_hello_cannot_be_replayed_to_a_new_challenge() {
        let k = key(1);
        let mut a = Lan::new(device(1), 4433);
        let mut b = Lan::new(device(2), 5544);
        a.set_keys(vec![k]);
        b.set_keys(vec![k]);
        // First run: capture A's Hello to B.
        let beacon = a.beacon(NOW);
        let challenge = b.handle(host(10), beacon, NOW).send.remove(0).1;
        let hello = a.handle(host(10), challenge, NOW).send.remove(0).1;
        // An attacker replays that Hello to B after B issues a new challenge.
        let _new_challenge = b.handle(host(12), a.beacon(NOW), NOW);
        let replay = b.handle(host(12), hello, NOW);
        assert!(replay.found.is_none(), "a Hello with a stale nonce is refused");
        assert!(replay.send.is_empty());
    }

    #[test]
    fn a_wrong_mac_is_refused() {
        let k = key(1);
        let mut b = Lan::new(device(2), 5544);
        b.set_keys(vec![k]);
        let mut a = Lan::new(device(1), 4433);
        a.set_keys(vec![k]);
        let challenge = b.handle(host(10), a.beacon(NOW), NOW).send.remove(0).1;
        let LanMsg::Challenge { beacon, token, nonce, .. } = challenge else {
            panic!("expected a challenge");
        };
        let forged = LanMsg::Challenge { beacon, token, nonce, mac: [0; 32] };
        assert!(a.handle(host(10), forged, NOW).send.is_empty());
    }

    #[test]
    fn an_expired_handshake_is_dropped() {
        let k = key(1);
        let mut b = Lan::new(device(2), 5544);
        b.set_keys(vec![k]);
        let mut a = Lan::new(device(1), 4433);
        a.set_keys(vec![k]);
        let challenge = b.handle(host(10), a.beacon(NOW), NOW).send.remove(0).1;
        let hello = a.handle(host(10), challenge, NOW).send.remove(0).1;
        let late = NOW + HANDSHAKE_TTL_MS + 1;
        assert!(b.handle(host(10), hello, late).found.is_none());
    }
}
