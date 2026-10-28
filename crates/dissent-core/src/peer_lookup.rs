//! The peer lookup: an address lookup service that asks connected, scoped devices for the record of
//! a device the endpoint cannot reach yet. Every dial consults it alongside the cache, the LAN and
//! the DHT, and the first verified address wins. Answers are signed records, so a device that
//! relays one cannot substitute its own address.

use std::sync::{Arc, Weak};
use std::time::Duration;

use dissent_identity::DeviceId;
use futures_lite::stream;
use iroh::{
    EndpointId,
    address_lookup::{AddressLookup, Error, Item},
};
use n0_future::{StreamExt, boxed::BoxStream};
use tokio::sync::mpsc;

use crate::engine::{Engine, Inner};

/// How long one live lookup waits for each device it asks.
const ASK_TIMEOUT: Duration = Duration::from_secs(3);

/// The lookup service the engine adds to its endpoint. Holds a weak reference, because the node
/// that owns it is stored in the engine.
#[derive(Debug)]
pub(crate) struct PeerLookup {
    inner: Weak<Inner>,
}

impl PeerLookup {
    pub(crate) fn new(inner: &Arc<Inner>) -> Self {
        Self {
            inner: Arc::downgrade(inner),
        }
    }
}

/// The engine for the length of one `resolve` call. iroh calls `resolve` while it holds the read
/// lock on its list of lookup services. If every other engine reference goes away meanwhile, this
/// one is the last, and dropping it drops the endpoint, whose `Drop` takes the write lock on that
/// list to clear it: the thread would wait on itself forever. So the last reference is dropped on
/// another thread, which waits until `resolve` has returned and the lock is free.
struct LookupEngine(Option<Engine>);

impl std::ops::Deref for LookupEngine {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        self.0.as_ref().expect("taken only on drop")
    }
}

impl Drop for LookupEngine {
    fn drop(&mut self) {
        let Some(inner) = self.0.take().and_then(|engine| Arc::into_inner(engine.inner)) else {
            return;
        };
        let runtime = tokio::runtime::Handle::try_current().ok();
        std::thread::spawn(move || {
            let _context = runtime.as_ref().map(|r| r.enter());
            drop(inner);
        });
    }
}

impl AddressLookup for PeerLookup {
    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        // Declared first so it drops last: any clone dropped inside this call is not the last.
        let engine = LookupEngine(Some(Engine {
            inner: self.inner.upgrade()?,
        }));
        let device = DeviceId(*endpoint_id.as_bytes());
        let peers = engine.lookup_targets(device);
        if peers.is_empty() || !engine.begin_lookup(device) {
            return None;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let task = engine.clone();
        engine.spawn_task(async move {
            let mut asks = Vec::with_capacity(peers.len());
            for peer in peers {
                let asker = task.clone();
                let tx = tx.clone();
                asks.push(tokio::spawn(async move {
                    let _ = tokio::time::timeout(ASK_TIMEOUT, asker.ask_for_record(peer, device, tx)).await;
                }));
            }
            // The stream ends once every ask has finished and dropped its sender.
            drop(tx);
            for ask in asks {
                let _ = ask.await;
            }
            task.end_lookup(device);
        });
        let items = stream::unfold(rx, |mut rx| async move {
            let item = rx.recv().await?;
            Some((item, rx))
        });
        Some(items.boxed())
    }
}

impl Engine {
    /// Marks `device` as being looked up. Returns false if a lookup for it is already running, so
    /// concurrent dials to one device share a single round of asks.
    fn begin_lookup(&self, device: DeviceId) -> bool {
        self.inner
            .asking
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(device)
    }

    fn end_lookup(&self, device: DeviceId) {
        self.inner
            .asking
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&device);
    }
}
