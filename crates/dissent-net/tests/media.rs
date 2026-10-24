use std::{sync::Arc, time::Duration};

use dissent_identity::{DeviceId, DeviceKey};
use dissent_net::{MediaHandler, MediaLink, Node, NetError, NodeConfig, PathKind, RelayChoice};
use tokio::sync::mpsc;

const PAYLOAD: usize = 1000;
const COUNT: u16 = 200;

/// Hands every inbound link to the test over a channel.
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

/// Spawns A (dialer) and B (accepting side) on loopback, and returns A's link to B
/// together with B's link, as delivered by the media handler.
async fn linked_pair() -> (Node, Node, MediaLink, MediaLink) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let a = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Inbound(tx)))))
        .await
        .expect("spawn b");
    a.remember(&b.addr().await).expect("remember b");

    let link_a = a.open_media(b.id()).await.expect("open media");
    let link_b = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("no inbound link within 10 s")
        .expect("handler channel closed");
    assert_eq!(link_a.peer, b.id());
    assert_eq!(link_b.peer, a.id());
    (a, b, link_a, link_b)
}

#[tokio::test]
async fn datagrams_flow_over_loopback() {
    let (a, b, link_a, link_b) = linked_pair().await;

    let max = link_a.max_datagram_size().expect("datagrams enabled");
    assert!(max >= PAYLOAD, "max datagram size {max} is below {PAYLOAD}");
    assert_eq!(link_a.path(), PathKind::Direct);

    for i in 0..COUNT {
        let mut payload = vec![0u8; PAYLOAD];
        payload[..2].copy_from_slice(&i.to_be_bytes());
        link_a.send(payload).expect("send");
    }

    let mut seen = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while seen.len() < COUNT as usize {
        let Ok(Ok(datagram)) = tokio::time::timeout_at(deadline, link_b.recv()).await else {
            break;
        };
        assert_eq!(datagram.len(), PAYLOAD);
        seen.insert(u16::from_be_bytes([datagram[0], datagram[1]]));
    }
    // Datagrams are unreliable. Require 95 % delivery; loopback normally delivers all.
    assert!(seen.len() * 100 >= COUNT as usize * 95, "only {} of {COUNT} arrived", seen.len());
    assert!(link_a.rtt_ms().is_some());

    link_a.close();
    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn oversize_datagram_is_rejected() {
    let (a, b, link_a, _link_b) = linked_pair().await;

    let max = link_a.max_datagram_size().expect("datagrams enabled");
    let err = link_a.send(vec![0u8; max + 1]).unwrap_err();
    assert!(matches!(err, NetError::FrameTooLarge { len, max: m } if len == max + 1 && m == max), "{err:?}");

    link_a.close();
    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn close_ends_recv() {
    let (a, b, link_a, link_b) = linked_pair().await;

    link_a.close();
    let result = tokio::time::timeout(Duration::from_secs(10), link_b.recv())
        .await
        .expect("recv did not end within 10 s");
    assert!(result.is_err(), "recv returned data after close");

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}
