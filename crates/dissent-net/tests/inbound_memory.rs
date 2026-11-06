//! A request that declares a large frame and sends little makes the server hold little. Its own test binary, since
//! it counts every allocation in the process and other tests would add to the count.

use std::{
    alloc::{GlobalAlloc, Layout, System},
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

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes straight to the system allocator; only the byte counts are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout is passed through unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` with this layout.
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

struct Echo;

impl RpcHandler for Echo {
    fn handle(&self, _from: dissent_identity::DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>> {
        Box::pin(async move { req })
    }
}

fn cfg(rpc: Option<Arc<dyn RpcHandler>>) -> NodeConfig {
    let inbound = InboundLimits { deadline_floor: Duration::from_secs(2), min_rate: 64 * 1024 * 1024, ..InboundLimits::default() };
    NodeConfig { relay: RelayChoice::Disabled, rpc, loopback_only: true, inbound, ..Default::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn declaring_sixteen_mib_and_sending_a_little_allocates_little() {
    let server = Node::spawn(&DeviceKey::generate(), cfg(Some(Arc::new(Echo)))).await.unwrap();
    let abuser = Node::spawn(&DeviceKey::generate(), cfg(None)).await.unwrap();
    abuser.remember(&server.addr().await).unwrap();
    let conn = abuser.endpoint().connect(EndpointId::from_bytes(&server.id().0).unwrap(), b"dissent/rpc/0").await.unwrap();
    let (mut send, _recv) = conn.open_bi().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    send.write_all(&(16u32 * 1024 * 1024).to_be_bytes()).await.unwrap();
    for _ in 0..10 {
        send.write_all(&[7u8; 1024]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::timeout(Duration::from_secs(6), conn.closed()).await.expect("cut off at the deadline");
    let grew = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    assert!(grew < 8 * 1024 * 1024, "the server held {grew} bytes for a frame that sent 10 KiB of 16 MiB");

    abuser.shutdown().await.unwrap();
    server.shutdown().await.unwrap();
}
