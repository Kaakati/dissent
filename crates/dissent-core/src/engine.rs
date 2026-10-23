//! The engine: start and stop, identity, contacts, and the shared state every other module uses.

use crate::error::{CoreError, Result};
use crate::net::{maintenance, receive_loop, ReceiverTask};
use crate::rpc::handler_for;
use crate::store::Paths;
use crate::views::{CommunitySummary, EngineEvent, MeView, NetStatusView, UserView};
use crate::world::World;
use dissent_discovery::{ContactCard, Discovery};
use dissent_events::{Hlc, HlcClock};
use dissent_identity::{CommunityId, DeviceId, Identity, RootId};
use dissent_net::{Node, NodeConfig, PeerAddr, RelayChoice, TopicReceiver};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

const EVENT_BUFFER: usize = 256;
/// How far ahead of our clock a remote HLC may be before we stop following it.
pub(crate) const MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;
pub(crate) const RETRY_EVERY: Duration = Duration::from_secs(30);
/// Display names are capped, like contact card names.
const MAX_NAME_CHARS: usize = 64;

/// Shared engine state. Tasks hold `Weak` references to it.
pub(crate) struct Inner {
    pub identity: Identity,
    pub me: RootId,
    pub world: RwLock<World>,
    pub clock: Mutex<HlcClock>,
    pub events: broadcast::Sender<EngineEvent>,
    pub node: OnceLock<Node>,
    pub tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Mainline DHT client for publishing community records and reading network stats. `None`
    /// when relays are disabled, so tests stay offline.
    pub discovery: Option<Arc<Discovery>>,
    /// Direct gossip neighbours per community, refreshed by each receive loop.
    pub neighbours: Mutex<BTreeMap<CommunityId, BTreeSet<DeviceId>>>,
}

/// Handle to the engine. Cheap to clone. All clones share one state.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    /// Opens the data directory, replays every community, and joins the network.
    pub async fn start(
        identity: Identity,
        data_dir: PathBuf,
        net: NodeConfig,
    ) -> Result<(Engine, broadcast::Receiver<EngineEvent>)> {
        let me = identity.root_id();
        let world = World::load(Paths::new(data_dir), &identity)?;
        let newest_stored = world.max_hlc();
        let discovery = if matches!(net.relay, RelayChoice::Disabled) {
            None
        } else {
            match Discovery::new() {
                Ok(d) => Some(Arc::new(d)),
                Err(e) => {
                    warn!(error = %e, "DHT unavailable; community records will not be published");
                    None
                }
            }
        };
        let (events, rx) = broadcast::channel(EVENT_BUFFER);
        let inner = Arc::new(Inner {
            identity,
            me,
            world: RwLock::new(world),
            clock: Mutex::new(HlcClock::new()),
            events,
            node: OnceLock::new(),
            tasks: Mutex::new(Vec::new()),
            discovery,
            neighbours: Mutex::new(BTreeMap::new()),
        });
        let config = NodeConfig {
            rpc: Some(handler_for(&inner)),
            ..net
        };
        let node = Node::spawn(&inner.identity.device, config).await?;
        let addr = node.addr().await;
        let engine = Engine { inner };
        let _ = engine.inner.node.set(node.clone());
        // Local stamps must sort after every stored event, so the clock starts past the newest.
        if let Some(newest) = newest_stored {
            engine.clock().observe(newest, now_ms());
        }

        let ids: Vec<CommunityId> = {
            let mut guard = engine.write();
            guard.my_addr = Some(addr);
            for known in guard.addresses.peers() {
                if let Err(e) = node.remember(&known) {
                    debug!(error = %e, "could not remember stored peer address");
                }
            }
            guard.communities.keys().copied().collect()
        };
        for id in ids {
            let bootstrap = engine.bootstrap_for(&id);
            let topic_rx = engine.subscribe_topic(id, bootstrap).await?;
            engine.spawn_receiver(id, topic_rx);
        }
        engine.spawn_addr_watch();
        engine.spawn_task(maintenance(Arc::downgrade(&engine.inner)));
        Ok((engine, rx))
    }

    /// Stops background tasks and the network node. Stored data stays on disk.
    pub async fn shutdown(self) -> Result<()> {
        {
            let mut guard = self.write();
            for c in guard.communities.values_mut() {
                if let Some(task) = c.task.take() {
                    task.handle.abort();
                }
                c.sender = None;
            }
        }
        let tasks = std::mem::take(&mut *self.lock_tasks());
        for task in tasks {
            task.abort();
        }
        self.node().shutdown().await?;
        Ok(())
    }

    /// A new receiver of engine notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn me(&self) -> MeView {
        let w = self.read();
        MeView {
            root_id: self.inner.me.to_string(),
            device_id: self.inner.identity.device_id().to_string(),
            suffix: self.inner.me.suffix(),
            display_name: w.my_name(),
        }
    }

    /// A `dissent://contact/...` link. It carries our cert, our name and our address.
    pub fn contact_link(&self) -> String {
        let w = self.read();
        let peers: Vec<String> = w.my_addr.iter().map(|a| a.to_string()).collect();
        let cert = self.inner.identity.cert.clone();
        let card = ContactCard::new(cert.clone(), &w.my_name(), peers.clone())
            .or_else(|_| ContactCard::new(cert, &w.anon_name(), peers));
        card.map(|c| c.to_url().to_string()).unwrap_or_default()
    }

    /// Adds (or replaces) a contact from a contact link, and learns its address.
    pub async fn add_contact(&self, link: &str) -> Result<UserView> {
        let card = ContactCard::parse(link)?;
        let user = card.cert.root;
        if user == self.inner.me {
            return Err(CoreError::Invalid("that is your own contact link".into()));
        }
        {
            let mut guard = self.write();
            let w = &mut *guard;
            w.directory.learn(&card.cert);
            for peer in card.peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()) {
                self.learn_address(w, peer);
            }
            w.contacts.retain(|c| c.cert.root != user);
            w.contacts.push(card);
            w.save_contacts()?;
        }
        Ok(self.read().user_view(&user))
    }

    /// Every community we belong to, sorted by name.
    pub fn communities(&self) -> Vec<CommunitySummary> {
        let w = self.read();
        let mut out: Vec<CommunitySummary> =
            w.communities.values().map(|c| w.summary(c)).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub fn user_profile(&self, user: RootId) -> UserView {
        self.read().user_view(&user)
    }

    /// DHT stats, peer count and home relay, for the status bar. Never fails: a DHT error
    /// leaves `dht` as `None`.
    pub async fn net_status(&self) -> NetStatusView {
        let dht = match &self.inner.discovery {
            Some(discovery) => discovery.stats().await.ok(),
            None => None,
        };
        let peers = self
            .inner
            .neighbours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .flatten()
            .collect::<BTreeSet<_>>()
            .len();
        let relay = self
            .read()
            .my_addr
            .as_ref()
            .and_then(|a| a.relay.as_ref())
            .and_then(|url| url.host_str().map(str::to_string));
        NetStatusView { dht, peers, relay }
    }

    pub(crate) fn read(&self) -> RwLockReadGuard<'_, World> {
        self.inner
            .world
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, World> {
        self.inner
            .world
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn clock(&self) -> MutexGuard<'_, HlcClock> {
        self.inner.clock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_tasks(&self) -> MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.inner.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn node(&self) -> Node {
        self.inner
            .node
            .get()
            .cloned()
            .expect("node is set during start")
    }

    pub(crate) fn emit(&self, event: EngineEvent) {
        let _ = self.inner.events.send(event);
    }

    /// A fresh local HLC stamp.
    pub(crate) fn stamp(&self) -> Hlc {
        self.clock().now(now_ms())
    }

    /// Folds a remote HLC into our clock so later local stamps sort after it. Stamps far in
    /// the future are ignored, so one peer cannot push every later stamp out of order.
    pub(crate) fn observe(&self, remote: Hlc) {
        let now = now_ms();
        if remote.wall_ms <= now.saturating_add(MAX_CLOCK_SKEW_MS) {
            self.clock().observe(remote, now);
        }
    }

    /// Runs `fut` on the runtime and keeps its handle. Finished handles are dropped first.
    pub(crate) fn spawn_task(&self, fut: impl Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(fut);
        let mut tasks = self.lock_tasks();
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
    }

    /// Sets the community's receive task, aborting any task it replaces.
    pub(crate) fn set_receiver_task(&self, id: CommunityId, task: ReceiverTask) {
        let mut guard = self.write();
        match guard.communities.get_mut(&id) {
            Some(c) => {
                if let Some(old) = c.task.replace(task) {
                    old.handle.abort();
                }
            }
            None => task.handle.abort(),
        }
    }

    /// Starts the receive loop for a community's topic.
    pub(crate) fn spawn_receiver(&self, id: CommunityId, rx: TopicReceiver) {
        let (stop, stop_rx) = oneshot::channel();
        let handle = tokio::spawn(receive_loop(
            Arc::downgrade(&self.inner),
            id,
            rx,
            stop_rx,
        ));
        self.set_receiver_task(id, ReceiverTask { handle, stop });
    }

    /// Devices of the members of `community`, used to dial it on startup and on join.
    pub(crate) fn bootstrap_for(&self, id: &CommunityId) -> Vec<DeviceId> {
        let w = self.read();
        w.communities
            .get(id)
            .map(|c| w.member_devices(c))
            .unwrap_or_default()
    }
}

/// Validates a display name: trimmed, non-empty, at most 64 characters.
pub(crate) fn clean_name(value: &str) -> Result<String> {
    let name = clean_text(value, "display name")?;
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CoreError::Invalid(format!(
            "display name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name)
}

/// Trims `value` and rejects it if nothing is left.
pub(crate) fn clean_text(value: &str, what: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(CoreError::Invalid(format!("{what} must not be empty")))
    } else {
        Ok(trimmed.to_string())
    }
}

pub(crate) fn not_found(id: &CommunityId) -> CoreError {
    CoreError::NotFound(format!("community {id}"))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
