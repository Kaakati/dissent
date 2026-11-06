//! Inbound limits over loopback: a device that dials us and sends its request slowly, or not at all, is cut off, and
//! a crowd of them cannot keep an honest request out.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use dissent_identity::DeviceKey;
use dissent_net::{InboundLimits, Node, NodeConfig, RelayChoice, RpcHandler};
use futures_lite::future::Boxed;
use iroh::EndpointId;

const RPC_ALPN: &[u8] = b"dissent/rpc/0";

/// Echoes requests and counts them.
struct Echo(Arc<AtomicUsize>);

impl RpcHandler for Echo {
    fn handle(&self, _from: dissent_identity::DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { req })
    }
}

fn cfg(rpc: Option<Arc<dyn RpcHandler>>, inbound: InboundLimits) -> NodeConfig {
    NodeConfig { relay: RelayChoice::Disabled, rpc, loopback_only: true, inbound, ..Default::default() }
}

/// A node that knows `server`, so it can dial it by id.
async fn dialer(server: &Node) -> Node {
    let node = Node::spawn(&DeviceKey::generate(), cfg(None, InboundLimits::default())).await.expect("spawn dialer");
    node.remember(&server.addr().await).expect("remember the server");
    node
}

/// Opens an RPC stream to `server` and declares a frame of `len` bytes, then sends nothing more. The stream is `None`
/// when the server already refused the connection.
async fn declare(from: &Node, server: &Node, len: u32) -> (iroh::endpoint::Connection, Option<iroh::endpoint::SendStream>) {
    let id = EndpointId::from_bytes(&server.id().0).unwrap();
    let conn = from.endpoint().connect(id, RPC_ALPN).await.expect("dial");
    let send = match conn.open_bi().await {
        Ok((mut send, _recv)) => send.write_all(&len.to_be_bytes()).await.ok().map(|()| send),
        Err(_) => None,
    };
    (conn, send)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_sent_slowly_is_cut_off_at_its_deadline() {
    let served = Arc::new(AtomicUsize::new(0));
    let limits = InboundLimits {
        deadline_floor: Duration::from_secs(1),
        min_rate: 64 * 1024 * 1024,
        close_wait: Duration::from_secs(1),
        ..InboundLimits::default()
    };
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo(served.clone()))), limits)).await.unwrap();
    let abuser = dialer(&server).await;

    let (conn, send) = declare(&abuser, &server, 16 * 1024 * 1024).await;
    let mut send = send.expect("the first connection is admitted");
    let dribble = tokio::spawn(async move {
        for _ in 0..200 {
            if send.write_all(&[7u8; 1024]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let started = std::time::Instant::now();
    let closed = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await.expect("cut off within 5 s");
    let after = started.elapsed();
    assert!(after < Duration::from_secs(3), "cut off at the 1.25 s deadline, not later: {after:?}");
    assert!(format!("{closed:?}").contains("ApplicationClosed"), "the server closed it: {closed:?}");
    assert_eq!(served.load(Ordering::SeqCst), 0, "the handler never saw the request");
    dribble.abort();

    // The slot is free again: an honest request goes through.
    let honest = dialer(&server).await;
    assert_eq!(honest.request(server.id(), b"hello".to_vec()).await.expect("request"), b"hello");

    for node in [honest, abuser, server] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_never_opens_a_stream_is_cut_off() {
    let limits = InboundLimits { deadline_floor: Duration::from_secs(1), ..InboundLimits::default() };
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo(Arc::default()))), limits)).await.unwrap();
    let idle = dialer(&server).await;
    let conn = idle.endpoint().connect(EndpointId::from_bytes(&server.id().0).unwrap(), RPC_ALPN).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), conn.closed()).await.expect("an idle connection is closed");
    for node in [idle, server] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crowd_of_slow_senders_cannot_keep_an_honest_request_out() {
    let served = Arc::new(AtomicUsize::new(0));
    let limits = InboundLimits {
        max_concurrent: 4,
        per_device: 2,
        evict_after: Duration::from_millis(300),
        deadline_floor: Duration::from_secs(60),
        ..InboundLimits::default()
    };
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo(served.clone()))), limits)).await.unwrap();
    let mut abusers = Vec::new();
    let mut held = Vec::new();
    for _ in 0..3 {
        let abuser = dialer(&server).await;
        for _ in 0..2 {
            held.push(declare(&abuser, &server, 1000).await);
        }
        abusers.push(abuser);
    }

    // A third connection from one device is refused at once, whatever else is happening.
    let (extra, _send) = declare(&abusers[0], &server, 1000).await;
    tokio::time::timeout(Duration::from_secs(3), extra.closed()).await.expect("over its share, refused");

    tokio::time::sleep(Duration::from_millis(400)).await;
    let honest = dialer(&server).await;
    let reply = tokio::time::timeout(Duration::from_secs(5), honest.request(server.id(), b"ok".to_vec()))
        .await
        .expect("the honest request is not kept waiting")
        .expect("the honest request is answered");
    assert_eq!(reply, b"ok");
    assert_eq!(served.load(Ordering::SeqCst), 1, "only the honest request reached the handler");

    drop(held);
    honest.shutdown().await.unwrap();
    for node in abusers {
        node.shutdown().await.unwrap();
    }
    server.shutdown().await.unwrap();
}

/// 32 devices, 2 connections each: 64 slow senders for 32 slots, so some are always waiting to get in, each dialling
/// again as soon as it is turned away. Returns the devices and their tasks; set `stop` to end them.
async fn flood(server: &Node, stop: &Arc<std::sync::atomic::AtomicBool>) -> (Vec<Node>, Vec<tokio::task::JoinHandle<()>>) {
    let mut abusers = Vec::new();
    let mut tasks = Vec::new();
    for _ in 0..32 {
        let abuser = dialer(server).await;
        for _ in 0..2 {
            let (abuser, server, stop) = (abuser.clone(), server.clone(), stop.clone());
            tasks.push(tokio::spawn(async move {
                while !stop.load(Ordering::SeqCst) {
                    let (conn, send) = declare(&abuser, &server, 1024 * 1024).await;
                    if let Some(mut send) = send {
                        while !stop.load(Ordering::SeqCst) && send.write_all(&[7u8; 64]).await.is_ok() {
                            if conn.close_reason().is_some() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }));
        }
        abusers.push(abuser);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    (abusers, tasks)
}

fn flood_limits() -> InboundLimits {
    InboundLimits {
        max_concurrent: 32,
        per_device: 2,
        evict_after: Duration::from_millis(100),
        deadline_floor: Duration::from_secs(60),
        ..InboundLimits::default()
    }
}

async fn stop_flood(stop: &std::sync::atomic::AtomicBool, abusers: Vec<Node>, tasks: Vec<tokio::task::JoinHandle<()>>) {
    stop.store(true, Ordering::SeqCst);
    for task in tasks {
        task.abort();
    }
    for node in abusers {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_honest_request_completes_while_every_slot_is_taken_by_slow_senders() {
    let served = Arc::new(AtomicUsize::new(0));
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo(served.clone()))), flood_limits())).await.unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (abusers, tasks) = flood(&server, &stop).await;

    let honest = dialer(&server).await;
    let big = vec![42u8; 8 * 1024 * 1024];
    let reply = honest.request(server.id(), big.clone()).await.expect("the large honest request is answered");
    assert_eq!(reply.len(), big.len());
    assert_eq!(served.load(Ordering::SeqCst), 1, "only the honest request reached the handler");

    stop_flood(&stop, abusers, tasks).await;
    honest.shutdown().await.unwrap();
    server.shutdown().await.unwrap();
}

/// An honest request that takes long to arrive but arrives steadily, as a large one over a relay does, keeps its
/// slot while slow senders come and go around it, and is answered on its first try.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_steady_upload_keeps_its_slot_while_slow_senders_come_and_go() {
    let served = Arc::new(AtomicUsize::new(0));
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo(served.clone()))), flood_limits())).await.unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (abusers, tasks) = flood(&server, &stop).await;

    // 2 MiB in 64 KiB pieces, one every 60 ms: about 1 MiB/s for two seconds, twenty times `evict_after`. A full node
    // may turn the connection away when it opens, as `Node::request` would see and retry; it must never lose its slot
    // once the upload is under way.
    let honest = dialer(&server).await;
    let len = 2 * 1024 * 1024u32;
    let server_id = EndpointId::from_bytes(&server.id().0).unwrap();
    let mut lost_midway = 0;
    let attempt = async {
        loop {
            let conn = honest.endpoint().connect(server_id, RPC_ALPN).await.unwrap();
            let started = std::time::Instant::now();
            let sent = async {
                let (mut send, mut recv) = conn.open_bi().await.ok()?;
                send.write_all(&len.to_be_bytes()).await.ok()?;
                for _ in 0..(len / (64 * 1024)) {
                    send.write_all(&[42u8; 64 * 1024]).await.ok()?;
                    tokio::time::sleep(Duration::from_millis(60)).await;
                }
                send.finish().ok()?;
                recv.read_to_end(4 + len as usize).await.ok()
            };
            match sent.await {
                Some(reply) => break reply,
                None if started.elapsed() > Duration::from_millis(500) => lost_midway += 1,
                None => {}
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    let reply = tokio::time::timeout(Duration::from_secs(30), attempt).await.expect("the honest upload gets through");
    assert_eq!(lost_midway, 0, "an upload under way was never given up for a slow sender");
    assert_eq!(reply.len(), 4 + len as usize, "the whole echo came back");
    assert_eq!(served.load(Ordering::SeqCst), 1);

    stop_flood(&stop, abusers, tasks).await;
    honest.shutdown().await.unwrap();
    server.shutdown().await.unwrap();
}
