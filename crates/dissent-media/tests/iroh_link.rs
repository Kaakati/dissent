//! A full call over an iroh media link between two dissent-net nodes on loopback, with relays
//! disabled. Every datagram crosses a real QUIC connection.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{assert_received, run_side, FRAMES};
use dissent_identity::{DeviceId, DeviceKey};
use dissent_media::{Call, LinkTransport, MediaKey, Side, Signal};
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
async fn call_over_iroh_media_link_delivers_encrypted_frames_both_ways() {
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

    let key = MediaKey::new([9; 32]);
    let (mut caller, offer) =
        Call::offer_over(LinkTransport::new(link_a, Side::Caller), key.clone(), 1)
            .await
            .expect("offer");
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) =
        Call::accept_over(LinkTransport::new(link_b, Side::Callee), key, 2, offer)
            .await
            .expect("accept");
    let answer = Signal::from_bytes(&answer.to_bytes().unwrap()).unwrap();
    caller.apply(answer).expect("apply answer");

    let ((caller, from_callee), (callee, from_caller)) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_side(caller, FRAMES, FRAMES as usize),
            run_side(callee, FRAMES, FRAMES as usize),
        )
    })
    .await
    .expect("the call did not finish in time");

    assert_received(&from_callee, 2);
    assert_received(&from_caller, 1);

    // Both ends used the iroh link directly, with no relay in the path.
    for call in [&caller, &callee] {
        assert_eq!(call.transport().link().path(), PathKind::Direct);
        assert_eq!(call.stats().frames_undecryptable, 0);
    }
}
