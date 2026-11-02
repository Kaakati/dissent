//! A call's frames must never repeat a (key, key id, counter) under AES-GCM. The ways a device starts sending again
//! within one MLS epoch: a tier move, leaving and rejoining, and restarting the app.

use crate::e2ee::{peek_counter, peek_key_id};
use crate::{Mesh, MediaKey, Mode, Participant};
use common_transport::NoTransport;

mod common_transport {
    use crate::transport::PacketTransport;
    use std::net::SocketAddr;

    /// Never carries anything. The frames here are only encrypted.
    pub(super) struct NoTransport;

    impl PacketTransport for NoTransport {
        fn local_addr(&self) -> SocketAddr {
            "127.0.0.1:1".parse().unwrap()
        }
        async fn send(&self, _to: SocketAddr, _bytes: &[u8]) -> crate::Result<()> {
            Ok(())
        }
        async fn recv(&self) -> crate::Result<(SocketAddr, Vec<u8>)> {
            std::future::pending().await
        }
    }
}

fn key() -> MediaKey {
    MediaKey::new([9; 32])
}

fn mesh() -> Mesh<NoTransport> {
    Mesh::new(key(), 3).unwrap()
}

fn leaf() -> Participant<NoTransport> {
    Participant::new(key(), 3, Vec::new(), 7, Mode::AudioTop3).unwrap()
}

/// What each start of sending puts on the wire first, for two different frames of the same length.
fn firsts() -> Vec<(&'static str, Vec<u8>)> {
    let (a, b) = (vec![0x11u8; 160], vec![0x22u8; 160]);
    vec![
        ("the first mesh", mesh().wire_for_test(&a)),
        ("the tier move to a forwarded leaf", leaf().wire_for_test(&b)),
        ("the tier move back to a mesh", mesh().wire_for_test(&b)),
        ("a rejoin, or a restart and a rejoin", mesh().wire_for_test(&b)),
    ]
}

#[test]
fn no_start_of_sending_repeats_a_key_id_and_counter() {
    let all = firsts();
    for (i, (what, wire)) in all.iter().enumerate() {
        for (other, second) in &all[..i] {
            let same = (peek_key_id(wire), peek_counter(wire)) == (peek_key_id(second), peek_counter(second));
            println!("{what} against {other}: key id {:x?} against {:x?}, counter {:?} against {:?}", peek_key_id(wire), peek_key_id(second), peek_counter(wire), peek_counter(second));
            assert!(!same, "{what} repeats the key id and counter of {other}, under the same key");
        }
    }
}

/// With a repeat, the two ciphertexts of different frames XOR to what the two plaintexts XOR to, so anyone who sees both
/// learns that without any key. A relay or a forwarder sees both.
#[test]
fn the_keystream_of_one_start_says_nothing_about_another() {
    let (a, b) = (vec![0x11u8; 160], vec![0x22u8; 160]);
    let (one, two) = (mesh().wire_for_test(&a), leaf().wire_for_test(&b));
    // A random context can be shorter on the wire, so these line up from the end. The tag at the end is not keystream.
    let body = |wire: &[u8]| wire[wire.len() - 16 - 160..wire.len() - 16].to_vec();
    let repeats = body(&one).iter().zip(&body(&two)).all(|(x, y)| x ^ y == 0x11 ^ 0x22);
    println!("first bytes {:02x?} and {:02x?}, keystream repeats: {repeats}", &one[..12], &two[..12]);
    assert!(!repeats, "the two frames share a keystream");
}
