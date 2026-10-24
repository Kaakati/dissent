//! A two-peer mesh over an iroh media link between two dissent-net nodes on loopback, relays
//! disabled. Every datagram crosses a real QUIC connection.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{assert_from_each, run_mesh_peer, FRAMES};
use dissent_identity::{DeviceId, DeviceKey};
use dissent_media::{LinkTransport, MediaKey, Mesh, Side, Signal};
use dissent_net::{MediaHandler, MediaLink, Node, NodeConfig, PathKind, RelayChoice};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Hands every inbound media link to the test over a channel.
struct Inbound(mpsc::UnboundedSender<MediaLink>);

impl MediaHandler for Inbound {
    fn incoming(&self, _from: DeviceId, link: MediaLink) {
        let _ = self.0.send(link);
    }
}

fn cfg(media: Option<Arc<dyn MediaHandler>>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        media,
        ..Default::default()
    }
}

#[tokio::test]
async fn two_peer_mesh_over_an_iroh_link_delivers_every_frame() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let a = Node::spawn(&DeviceKey::generate(), cfg(None))
        .await
        .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Inbound(tx)))))
        .await
        .expect("spawn b");
    a.remember(&b.addr().await).expect("remember b");

    let link_a = a.open_media(b.id()).await.expect("open media");
    let link_b = timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("no inbound media link within 10 s")
        .expect("handler channel closed");
    assert_eq!(link_a.path(), PathKind::Direct);

    let media_key = MediaKey::new([4; 32]);
    let mut alice: Mesh<LinkTransport> = Mesh::new(media_key.clone(), 1).unwrap();
    let mut bob: Mesh<LinkTransport> = Mesh::new(media_key, 2).unwrap();

    // Alice has the lower id, so she offers and Bob answers.
    let offer = alice
        .offer(2, LinkTransport::new(link_a, Side::Caller))
        .await
        .expect("offer");
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let answer = bob
        .accept(1, LinkTransport::new(link_b, Side::Callee), offer)
        .await
        .expect("accept");
    let answer = Signal::from_bytes(&answer.to_bytes().unwrap()).unwrap();
    alice.apply(2, answer).expect("apply answer");

    let ((alice, from_bob), (bob, from_alice)) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_mesh_peer(alice, FRAMES, FRAMES as usize),
            run_mesh_peer(bob, FRAMES, FRAMES as usize),
        )
    })
    .await
    .expect("the mesh did not finish in time");

    assert_from_each(&from_bob, &[2], FRAMES);
    assert_from_each(&from_alice, &[1], FRAMES);
    for mesh in [&alice, &bob] {
        for stats in mesh.stats().values() {
            assert_eq!(stats.frames_undecryptable, 0);
        }
    }
}
