use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use dissent_identity::{CommunityKey, DeviceId, DeviceKey};
use dissent_net::{
    Node, NetError, NodeConfig, PathKind, PeerAddr, RelayChoice, RpcHandler, TopicId, TopicReceiver,
};
use futures_lite::{StreamExt, future::Boxed};

const MAX_FRAME: usize = 16 * 1024 * 1024;

fn cfg(rpc: Option<Arc<dyn RpcHandler>>) -> NodeConfig {
    NodeConfig { relay: RelayChoice::Disabled, rpc, loopback_only: true, ..Default::default() }
}

/// Polls `rx` until `peer` is (or is no longer) a neighbour. Neighbour state only moves while polled.
async fn wait_neighbor(rx: &mut TopicReceiver, peer: DeviceId, present: bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while rx.neighbors().contains(&peer) != present {
            let _ = tokio::time::timeout(Duration::from_millis(100), rx.next()).await;
        }
    })
    .await
    .expect("neighbour state did not change within 10 s");
}

#[tokio::test]
async fn ping_then_gossip_over_loopback() {
    let key_a = DeviceKey::generate();
    let key_b = DeviceKey::generate();
    let a = Node::spawn(&key_a, cfg(None)).await.expect("spawn a");
    let b = Node::spawn(&key_b, cfg(None)).await.expect("spawn b");
    assert_eq!(a.id(), key_a.id());

    let addr_a = a.addr().await;
    let report = b.ping(&addr_a).await.expect("ping a");
    assert_eq!(report.path, PathKind::Direct);

    b.remember(&addr_a).expect("remember a");
    let topic = TopicId::for_channel(&CommunityKey::generate().id(), &[7; 32]);
    let (tx_a, _rx_a) = a.subscribe(topic, vec![]).await.expect("subscribe a");
    let (_tx_b, mut rx_b) = b.subscribe(topic, vec![a.id()]).await.expect("subscribe b");

    // Gossip drops sends made before a neighbour is up, so resend until B hears one.
    let sender = tokio::spawn(async move {
        loop {
            let _ = tx_a.broadcast(b"hello".to_vec()).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let msg = tokio::time::timeout(Duration::from_secs(10), rx_b.next())
        .await
        .expect("no gossip within 10 s")
        .expect("topic closed");
    sender.abort();
    assert_eq!(msg.bytes, b"hello");
    assert_eq!(msg.from, a.id());

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn leaving_a_topic_drops_the_neighbour() {
    let a = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn b");
    b.remember(&a.addr().await).expect("remember a");
    let topic = TopicId::for_channel(&CommunityKey::generate().id(), &[9; 32]);

    let (_tx_a, mut rx_a) = a.subscribe(topic, vec![]).await.expect("subscribe a");
    let (tx_b, rx_b) = b.subscribe(topic, vec![a.id()]).await.expect("subscribe b");
    wait_neighbor(&mut rx_a, b.id(), true).await;

    tx_b.leave(rx_b);
    wait_neighbor(&mut rx_a, b.id(), false).await;
    assert!(rx_a.neighbors().is_empty());

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn watch_addr_yields_current_address_first() {
    let a = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn a");
    let mut watch = a.watch_addr();
    let first = tokio::time::timeout(Duration::from_secs(5), watch.next())
        .await
        .expect("no address within 5 s")
        .expect("watch ended");
    assert_eq!(first, a.addr().await);
    assert!(!first.direct.is_empty(), "loopback node has no direct address");
    assert!(first.direct.iter().all(|d| d.ip().is_loopback()), "non-loopback bind: {:?}", first.direct);
    a.shutdown().await.expect("shutdown a");
}

/// Uppercases the request and records who asked.
struct Upper(Arc<Mutex<Vec<DeviceId>>>);

impl RpcHandler for Upper {
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>> {
        self.0.lock().unwrap().push(from);
        Box::pin(async move { req.to_ascii_uppercase() })
    }
}

/// Replies with one byte more than the frame limit.
struct Oversize;

impl RpcHandler for Oversize {
    fn handle(&self, _from: DeviceId, _req: Vec<u8>) -> Boxed<Vec<u8>> {
        Box::pin(async { vec![0u8; MAX_FRAME + 1] })
    }
}

#[tokio::test]
async fn request_roundtrip_over_loopback() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let a = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Upper(seen.clone())))))
        .await
        .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn b");
    b.remember(&a.addr().await).expect("remember a");

    let reply = b.request(a.id(), b"hello rpc".to_vec()).await.expect("request");
    assert_eq!(reply, b"HELLO RPC");
    assert_eq!(*seen.lock().unwrap(), vec![b.id()]);

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn oversize_frames_fail_cleanly() {
    let a = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Oversize))))
        .await
        .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), cfg(None)).await.expect("spawn b");
    b.remember(&a.addr().await).expect("remember a");

    // Oversize request: rejected before anything reaches the wire.
    let err = b.request(a.id(), vec![0u8; MAX_FRAME + 1]).await.unwrap_err();
    assert!(matches!(err, NetError::FrameTooLarge { .. }), "{err:?}");

    // Oversize reply: A refuses to send it, so B gets an error instead of waiting out the timeout.
    let err = b.request(a.id(), b"x".to_vec()).await.unwrap_err();
    assert!(!matches!(err, NetError::Timeout), "{err:?}");

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

/// Dials by device id alone, with no shared memory lookup. Relays are the n0 defaults, but n0's
/// DNS lookup is not configured (`Custom` uses the minimal preset), so the DHT is the only way to
/// find A. The DHT publishes relay URLs only, which is enough to reach A. Needs internet access.
#[tokio::test]
#[ignore = "needs internet access to the Mainline DHT"]
async fn dial_by_id_via_mainline_dht() {
    let relays: Vec<url::Url> = iroh::defaults::prod::default_relay_map().urls::<Vec<_>>().into_iter().map(url::Url::from).collect();
    let echo = NodeConfig {
        relay: RelayChoice::Custom(relays.clone()),
        dht_lookup: true,
        rpc: Some(Arc::new(Upper(Arc::new(Mutex::new(Vec::new()))))),
        ..Default::default()
    };
    let client = NodeConfig { relay: RelayChoice::Custom(relays), dht_lookup: true, ..Default::default() };
    let a = Node::spawn(&DeviceKey::generate(), echo).await.expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), client).await.expect("spawn b");

    // A publishes to the DHT in the background, so retry with backoff until B can resolve it.
    let started = std::time::Instant::now();
    let by_id = PeerAddr { id: a.id(), relay: None, direct: vec![] };
    let report = tokio::time::timeout(Duration::from_secs(120), async {
        let mut delay = Duration::from_secs(2);
        loop {
            match b.ping(&by_id).await {
                Ok(report) => break report,
                Err(e) => {
                    eprintln!("dht lookup not ready after {:?}: {e}", started.elapsed());
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(15));
                }
            }
        }
    })
    .await
    .expect("DHT lookup did not resolve within 120 s");
    eprintln!("dht dial by id took {:?}, ping rtt {} ms, path {:?}", started.elapsed(), report.rtt_ms, report.path);

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}
