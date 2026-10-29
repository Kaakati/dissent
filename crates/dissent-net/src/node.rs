use std::{
    fmt,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use dissent_identity::{DeviceId, DeviceKey};
use iroh::{
    Endpoint, EndpointId, RelayConfig, RelayMode, RelayUrl, SecretKey,
    address_lookup::{
        AddressLookup, AddressLookupBuilder, DnsAddressLookup, MemoryLookup, PkarrPublisher, PkarrResolver,
    },
    endpoint::{default_relay_mode, presets},
    protocol::Router,
    tls::CaTlsConfig,
};
use futures_lite::{Stream, StreamExt};
use iroh::Watcher;
use std::time::Duration;
use iroh::protocol::DynProtocolHandler;
use iroh_gossip::{ALPN as GOSSIP_ALPN, Gossip};
use iroh_mainline_address_lookup::DhtAddressLookup;
use std::time::Instant;
use tokio::task::JoinHandle;
use tracing::warn;
use url::Url;

use crate::{
    addr::PeerAddr,
    extra::{self, Shared},
    lan,
    media::MEDIA_ALPN,
    error::{NetError, Result},
    media::{MediaHandler, MediaLink, MediaProtocol, dial_media},
    ping::{PING_ALPN, PingHandler, PingReport, run_ping},
    rpc::{RPC_ALPN, RpcHandler, RpcProtocol, run_request},
    source::{self, DialLog, DialStats, Recorded},
    tls::pq_preferred_provider,
    topic::{TopicId, TopicReceiver, TopicSender},
};

/// Relay servers used to hole-punch and fall back when no direct path works.
#[derive(Debug, Clone, Default)]
pub enum RelayChoice {
    /// iroh's public n0 relays.
    #[default]
    Default,
    Custom(Vec<Url>),
    /// No relays: direct IP paths only.
    Disabled,
}

/// What the net report says about the relays: the preferred one, and each relay's best measured latency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayReport {
    pub home: Option<Url>,
    pub latencies: Vec<(Url, Option<std::time::Duration>)>,
}

/// Default gossip frame limit: 128 KiB, enough for large MLS commits and long messages.
pub const DEFAULT_GOSSIP_MAX_MESSAGE: usize = 128 * 1024;

#[derive(Clone)]
pub struct NodeConfig {
    pub relay: RelayChoice,
    /// UDP port to bind on all IPv4 interfaces. `0` picks any free port.
    pub bind_port: u16,
    /// Handler for incoming requests on the RPC ALPN. `None` refuses RPC.
    pub rpc: Option<Arc<dyn RpcHandler>>,
    /// Publish addresses to, and resolve peers from, the Mainline DHT. Only used when `relay`
    /// is not `Disabled`, so relay-less configs never touch the network. By design only relay
    /// URLs are published, never direct IP addresses.
    pub dht_lookup: bool,
    /// Bind only to 127.0.0.1, so nothing listens on external interfaces. Used by tests and
    /// local multi-instance checks; avoids OS firewall prompts. Peers must be on this machine.
    pub loopback_only: bool,
    /// Give the endpoint no IP transport, so every connection goes through a relay and no direct
    /// path can form. Takes precedence over `loopback_only` and `bind_port`, and turns LAN discovery
    /// off. The DHT lookup keeps its own socket: turn `dht_lookup` off too for a node with no UDP
    /// socket at all. Needs a relay: an error with `RelayChoice::Disabled`. Off by default; used by
    /// tests that must keep a path relayed.
    pub relay_only: bool,
    /// Add the relays that communities name to this node. Off, the node uses only the relays in `relay`, so
    /// no community can have this device measured or dialled through its relay.
    pub community_relays: bool,
    /// Handler for inbound media links on the media ALPN. `None` refuses media.
    pub media: Option<Arc<dyn MediaHandler>>,
    /// Largest gossip frame in bytes, framing included (about 256 bytes of it). Every peer on a
    /// topic must use the same value or a larger one: a peer receiving a bigger frame closes the
    /// connection. `TopicSender::broadcast` rejects payloads that would not fit.
    pub gossip_max_message: usize,
    /// Extra ALPNs served by the node's router, for example a file-transfer handler. Each ALPN
    /// must be unique and must not be one the node registers itself. Handlers are shut down by
    /// `Node::shutdown`, before the endpoint closes.
    pub extra_protocols: Vec<(Vec<u8>, Arc<dyn DynProtocolHandler>)>,
    /// Trust roots for relay TLS. `None` uses iroh's built-in roots. Set it for a relay with a
    /// private CA, for example `CaTlsConfig::custom_roots` with that relay's certificate.
    pub relay_ca: Option<CaTlsConfig>,
    /// Find community members on the same local network, by multicast with rotating tokens and a
    /// challenge handshake. Off by default. Ignored when `loopback_only` is set, since it binds a
    /// multicast socket.
    pub lan_discovery: bool,
    /// This device answers address lookups as a seeder, so peers label those answers `seeder`.
    pub serves_lookups: bool,
    /// Publish this device's ID and addresses to number0's DNS directory, and resolve peers through it. It
    /// is faster to connect, but number0 sees the device ID and addresses. Used only with number0's relays
    /// (`RelayChoice::Default`), where iroh's N0 preset used to add it; custom relays never use it. On by
    /// default.
    pub n0_address_lookup: bool,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            relay: RelayChoice::Default,
            bind_port: 0,
            rpc: None,
            dht_lookup: true,
            loopback_only: false,
            relay_only: false,
            community_relays: true,
            media: None,
            gossip_max_message: DEFAULT_GOSSIP_MAX_MESSAGE,
            extra_protocols: Vec::new(),
            relay_ca: None,
            lan_discovery: false,
            serves_lookups: false,
            n0_address_lookup: true,
        }
    }
}

impl fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeConfig")
            .field("relay", &self.relay)
            .field("bind_port", &self.bind_port)
            .field("rpc", &self.rpc.as_ref().map(|_| "RpcHandler"))
            .field("dht_lookup", &self.dht_lookup)
            .field("n0_address_lookup", &self.n0_address_lookup)
            .field("media", &self.media.as_ref().map(|_| "MediaHandler"))
            .field("gossip_max_message", &self.gossip_max_message)
            .field(
                "extra_protocols",
                &self.extra_protocols.iter().map(|(alpn, _)| alpn).collect::<Vec<_>>(),
            )
            .field("loopback_only", &self.loopback_only)
            .field("relay_only", &self.relay_only)
            .field("lan_discovery", &self.lan_discovery)
            .field("serves_lookups", &self.serves_lookups)
            .finish()
    }
}

/// A networked device: owns the iroh endpoint, its router, and gossip. Cheap to clone.
#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
}

struct Inner {
    id: DeviceId,
    /// Relays this node knows: the configured ones plus any added at runtime.
    relays: Mutex<Vec<Url>>,
    endpoint: Endpoint,
    router: Router,
    gossip: Gossip,
    /// The address cache: addresses remembered with `remember`.
    lookup: MemoryLookup,
    gossip_max: usize,
    dials: Arc<DialLog>,
    /// When this node last dialled each peer, for `contacted_within`.
    contacted: Mutex<std::collections::HashMap<[u8; 32], Instant>>,
    /// Community keys the LAN handshake answers for. Set by `set_lan_keys`.
    lan_keys: Arc<Mutex<Vec<[u8; 32]>>>,
    serves_lookups: bool,
    lan_task: Mutex<Option<JoinHandle<()>>>,
    /// The CA the relays were checked against, kept for `ping_via_relay`. `None` is the public roots.
    relay_ca: Option<CaTlsConfig>,
    /// Relay probes bound and not yet closed or dropped.
    open_probes: std::sync::atomic::AtomicUsize,
}

impl Node {
    /// Starts the endpoint with the device key as its iroh secret key, so the node id equals `DeviceId`.
    ///
    /// Authentication: iroh uses raw public keys (RFC 7250) inside TLS 1.3. Each endpoint's
    /// certificate is its Ed25519 key, and a dialer checks the server's key against the id it
    /// dialled. QUIC is end to end between the two endpoints. A relay forwards QUIC packets it
    /// cannot decrypt, so it cannot terminate TLS or read payloads.
    ///
    /// ALPNs accepted: `dissent/ping/0` and the gossip ALPN always, `dissent/rpc/0` when `rpc` is
    /// set, and `dissent/media/0` when `media` is set.
    ///
    /// Key exchange prefers hybrid X25519 + ML-KEM-768 and falls back to classical X25519, P-256
    /// or P-384 for peers that lack it.
    pub async fn spawn(device: &DeviceKey, cfg: NodeConfig) -> Result<Node> {
        extra::validate(&cfg.extra_protocols)?;
        if cfg.gossip_max_message < iroh_gossip::proto::MIN_MAX_MESSAGE_SIZE {
            return Err(NetError::Config(format!(
                "gossip_max_message must be at least {} bytes",
                iroh_gossip::proto::MIN_MAX_MESSAGE_SIZE
            )));
        }
        if cfg.relay_only && matches!(cfg.relay, RelayChoice::Disabled) {
            return Err(NetError::Config("relay_only needs a relay, but relays are disabled".into()));
        }
        let gossip_max = cfg.gossip_max_message;
        let lookup = MemoryLookup::with_provenance(source::CACHE);
        let lan_lookup = MemoryLookup::with_provenance(source::LAN);
        let dials = Arc::new(DialLog::default());
        let use_dht = cfg.dht_lookup && !matches!(cfg.relay, RelayChoice::Disabled);
        // number0's directory is used only with number0's own relays, as iroh's N0 preset did, and only when
        // the setting is on. Custom relays and relay-less configs never reach it.
        let use_n0 = cfg.n0_address_lookup && matches!(cfg.relay, RelayChoice::Default);
        // Always the minimal preset: the directory is added below, separately, so it can be switched off.
        let preset = Endpoint::builder(presets::Minimal);
        let provider = pq_preferred_provider();
        tracing::debug!(
            suites = ?provider.cipher_suites.iter().map(|s| s.suite()).collect::<Vec<_>>(),
            groups = ?provider.kx_groups.iter().map(|g| g.name()).collect::<Vec<_>>(),
            "tls policy"
        );
        let initial_relays: Vec<Url> = match &cfg.relay {
            RelayChoice::Default => default_relay_mode()
                .relay_map()
                .urls::<Vec<RelayUrl>>()
                .into_iter()
                .map(Url::from)
                .collect(),
            RelayChoice::Custom(urls) => urls.clone(),
            RelayChoice::Disabled => Vec::new(),
        };
        let mut builder = preset
            .secret_key(SecretKey::from_bytes(&device.0.seed()))
            .address_lookup(Recorded::new(source::CACHE, lookup.clone(), dials.clone()))
            .crypto_provider(provider);
        builder = match cfg.relay {
            RelayChoice::Default => builder.relay_mode(RelayMode::Default),
            RelayChoice::Disabled => builder.relay_mode(RelayMode::Disabled),
            RelayChoice::Custom(urls) => {
                builder.relay_mode(RelayMode::custom(urls.into_iter().map(RelayUrl::from)))
            }
        };
        let relay_ca = cfg.relay_ca.clone();
        if let Some(ca) = cfg.relay_ca {
            builder = builder.ca_tls_config(ca);
        }
        if cfg.relay_only {
            builder = builder.clear_ip_transports();
        } else if cfg.loopback_only {
            let addr = SocketAddr::from(([127, 0, 0, 1], cfg.bind_port));
            builder = builder
                .clear_ip_transports()
                .bind_addr(addr)
                .map_err(|e| NetError::Config(e.to_string()))?;
        } else if cfg.bind_port != 0 {
            let addr = SocketAddr::from(([0, 0, 0, 0], cfg.bind_port));
            builder = builder.bind_addr(addr).map_err(|e| NetError::Config(e.to_string()))?;
        }
        let endpoint = builder.bind().await?;
        // Added after bind, so each service can be wrapped to record which one resolves a dial. The
        // DHT uses the endpoint's key to publish our address, as it did when it was a builder.
        let services = endpoint
            .address_lookup()
            .map_err(|e| NetError::Config(e.to_string()))?;
        services.add(Recorded::new(source::LAN, lan_lookup.clone(), dials.clone()));
        if use_dht {
            let dht = DhtAddressLookup::builder()
                .secret_key(endpoint.secret_key().clone())
                .build()
                .map_err(|e| NetError::Config(e.to_string()))?;
            services.add(Recorded::new(source::DHT, dht, dials.clone()));
        }
        if use_n0 {
            // The same three services the N0 preset added: publish our address, resolve peers by it, and
            // resolve them by DNS. Each one is wrapped with its own label, so the dial log still names it.
            let publisher = PkarrPublisher::n0_dns()
                .into_address_lookup(&endpoint)
                .map_err(|e| NetError::Config(e.to_string()))?;
            services.add(Recorded::new(source::N0_DNS, publisher, dials.clone()));
            let resolver = PkarrResolver::n0_dns()
                .into_address_lookup(&endpoint)
                .map_err(|e| NetError::Config(e.to_string()))?;
            services.add(Recorded::new(source::N0_DNS, resolver, dials.clone()));
            let dns = DnsAddressLookup::n0_dns()
                .into_address_lookup(&endpoint)
                .map_err(|e| NetError::Config(e.to_string()))?;
            services.add(Recorded::new(source::N0_DNS, dns, dials.clone()));
        }
        let lan_keys = Arc::new(Mutex::new(Vec::new()));
        let lan_task = if cfg.lan_discovery && !cfg.loopback_only && !cfg.relay_only {
            match lan::bind() {
                Ok(socket) => {
                    let port = endpoint.bound_sockets().first().map_or(0, |a| a.port());
                    let state = lan::Lan::new(device.id(), port);
                    Some(tokio::spawn(lan::run(socket, state, lan_keys.clone(), lan_lookup.clone())))
                }
                Err(e) => {
                    warn!(error = %e, "LAN discovery could not bind, continuing without it");
                    None
                }
            }
        } else {
            None
        };
        let gossip = Gossip::builder()
            .max_message_size(gossip_max)
            .spawn(endpoint.clone());
        let mut router = Router::builder(endpoint.clone())
            .accept(PING_ALPN, PingHandler)
            .accept(GOSSIP_ALPN, gossip.clone());
        if let Some(handler) = cfg.rpc {
            router = router.accept(RPC_ALPN, RpcProtocol(handler));
        }
        if let Some(handler) = cfg.media {
            router = router.accept(MEDIA_ALPN, MediaProtocol(handler));
        }
        for (alpn, handler) in cfg.extra_protocols {
            router = router.accept(alpn, Shared(handler));
        }
        let router = router.spawn();
        Ok(Node {
            inner: Arc::new(Inner {
                id: device.id(),
                relays: Mutex::new(initial_relays),
                endpoint,
                router,
                gossip,
                lookup,
                gossip_max,
                dials,
                contacted: Mutex::new(std::collections::HashMap::new()),
                lan_keys,
                serves_lookups: cfg.serves_lookups,
                lan_task: Mutex::new(lan_task),
                relay_ca,
                open_probes: std::sync::atomic::AtomicUsize::new(0),
            }),
        })
    }

    /// Read-only access to the iroh endpoint, for dialling custom ALPNs or passing to a crate's
    /// `fetch`. Do not close it directly. Use `Node::shutdown`, which stops the router and its
    /// extra handlers first.
    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    /// Relay servers this node knows: the configured ones plus any added with `add_relays`.
    pub fn relays(&self) -> Vec<Url> {
        self.inner.relays.lock().expect("relay list lock").clone()
    }

    /// Adds relay servers at runtime. Each one is used for dialling and for relaying, and this
    /// node advertises it in its address. Relays already known are skipped.
    pub async fn add_relays(&self, urls: Vec<Url>) {
        for url in urls {
            if self.relays().contains(&url) {
                continue;
            }
            let relay = RelayUrl::from(url.clone());
            let config = RelayConfig::new(relay.clone(), None);
            self.inner.endpoint.insert_relay(relay, Arc::new(config)).await;
            let mut known = self.inner.relays.lock().expect("relay list lock");
            if !known.contains(&url) {
                known.push(url);
            }
        }
    }

    /// Removes relay servers added with `add_relays`. This node stops dialling them, and stops
    /// advertising them in its address.
    pub async fn remove_relays(&self, urls: &[Url]) {
        for url in urls {
            self.inner.endpoint.remove_relay(&RelayUrl::from(url.clone())).await;
            self.inner.relays.lock().expect("relay list lock").retain(|known| known != url);
        }
    }

    /// Resolves once this node is connected to a relay. Returns at once if no relay is known,
    /// since waiting then would never finish.
    pub async fn online(&self) {
        if self.relays().is_empty() {
            return;
        }
        self.inner.endpoint.online().await;
    }

    pub fn id(&self) -> DeviceId {
        self.inner.id
    }

    /// The latest net report: the relay this node prefers, and the latency to each relay it measured. Each entry is
    /// the relay's URL and the best latency measured to it, `None` when no probe reached it yet.
    pub fn relay_report(&self) -> RelayReport {
        let mut latencies: Vec<(Url, Option<Duration>)> = self
            .relays()
            .into_iter()
            .map(|url| (url, None))
            .collect();
        let report = self.inner.endpoint.net_report().get();
        let home = report.as_ref().and_then(|r| r.preferred_relay.clone()).map(Url::from);
        if let Some(r) = report.as_ref() {
            for (_, url, latency) in r.relay_latency.iter() {
                let url = Url::from(url.clone());
                if let Some(slot) = latencies.iter_mut().find(|(u, _)| *u == url) {
                    slot.1 = Some(slot.1.map_or(latency, |best| best.min(latency)));
                }
            }
        }
        RelayReport { home, latencies }
    }

    /// Our dialable address as known now. Relay and direct addresses may still be filling in.
    pub async fn addr(&self) -> PeerAddr {
        PeerAddr::from_iroh(&self.inner.endpoint.addr())
    }

    /// Yields our address now, then again whenever it changes (new relay, new direct address).
    /// Re-announce on each item so peers can still reach us after a network change.
    pub fn watch_addr(&self) -> impl Stream<Item = PeerAddr> + Send + Unpin + 'static {
        self.inner.endpoint.watch_addr().stream().map(|a| PeerAddr::from_iroh(&a))
    }

    /// Records a peer's addresses so gossip can dial it by id alone.
    pub fn remember(&self, peer: &PeerAddr) -> Result<()> {
        self.inner.lookup.add_endpoint_info(peer.to_iroh()?);
        Ok(())
    }

    pub async fn ping(&self, peer: &PeerAddr) -> Result<PingReport> {
        let target = peer.to_iroh()?;
        self.inner.dials.begin(peer.id.0);
        let started = Instant::now();
        let report = run_ping(&self.inner.endpoint, target).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.inner.dials.finish(peer.id.0, elapsed_ms, report.is_ok());
        report
    }

    /// Relay probes bound and not yet closed. See `ping_via_relay`.
    pub fn open_relay_probes(&self) -> usize {
        self.inner.open_probes.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Pings `peer` through `relay` and nothing else: a short-lived endpoint with this node's key, the
    /// one relay in its relay map, no IP transport and no address lookup. The ping completes only if
    /// the relay admits this node's key and carries its packets to `peer`, which must be connected to
    /// that relay. Returns the round trip in ms. The relay keeps this node's main connection as an
    /// inactive duplicate while the probe runs, and makes it active again when the probe closes.
    pub async fn ping_via_relay(&self, relay: &Url, peer: DeviceId, timeout: Duration) -> Result<u64> {
        let relay_url = RelayUrl::from(relay.clone());
        // Everything that can fail before the probe exists is done first, so nothing returns between
        // binding the probe and closing it.
        let target = iroh::EndpointAddr::new(
            EndpointId::from_bytes(&peer.0).map_err(|e| NetError::BadPeer(e.to_string()))?,
        )
        .with_relay_url(relay_url.clone());
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(self.inner.endpoint.secret_key().clone())
            .crypto_provider(pq_preferred_provider())
            .relay_mode(RelayMode::custom([relay_url.clone()]))
            .clear_ip_transports();
        if let Some(ca) = self.inner.relay_ca.clone() {
            builder = builder.ca_tls_config(ca);
        }
        let probe = builder.bind().await?;
        // Counts the probe until it is closed, or dropped if the caller gives up on this future.
        // A dropped endpoint shuts down on its own.
        let _open = OpenProbe::new(&self.inner.open_probes);
        let result = tokio::time::timeout(timeout, run_ping(&probe, target)).await;
        probe.close().await;
        match result {
            Ok(Ok(report)) => Ok(report.rtt_ms),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(NetError::Protocol("no answer through the relay in time".into())),
        }
    }

    /// Sends `req` to `peer` and returns its reply. Dials by id, so addresses must be
    /// remembered or discoverable. Fails with `Timeout` after 15 s.
    pub async fn request(&self, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>> {
        self.note_contacted(peer);
        self.inner.dials.begin(peer.0);
        let started = Instant::now();
        let reply = run_request(&self.inner.endpoint, peer, req).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.inner.dials.finish(peer.0, elapsed_ms, reply.is_ok());
        reply
    }

    /// Counts of dials by the address source that resolved them, and the most recent dial. A
    /// source of `dht` means the Mainline DHT was the one that answered.
    pub fn dial_stats(&self) -> DialStats {
        self.inner.dials.snapshot()
    }

    /// Replaces the community keys used for LAN tokens and handshakes. Call it whenever the
    /// communities or their secrets change.
    pub fn set_lan_keys(&self, keys: Vec<[u8; 32]>) {
        *self.inner.lan_keys.lock().expect("LAN keys lock") = keys;
    }

    /// Whether this node answers address lookups as a seeder.
    pub fn serves_lookups(&self) -> bool {
        self.inner.serves_lookups
    }

    /// Adds an address lookup service, such as the peer lookup. Dials consult every service at
    /// once, and the first address any of them yields wins. Its dials are recorded under `label`
    /// unless the item names a known source of its own.
    pub fn add_lookup(&self, label: &'static str, service: impl AddressLookup + 'static) -> Result<()> {
        let services = self
            .inner
            .endpoint
            .address_lookup()
            .map_err(|e| NetError::Config(e.to_string()))?;
        services.add(Recorded::new(label, service, self.inner.dials.clone()));
        Ok(())
    }

    /// Dials `peer` and opens an unreliable datagram link for call media. The peer sees it via
    /// `NodeConfig.media`.
    pub async fn open_media(&self, peer: DeviceId) -> Result<MediaLink> {
        self.note_contacted(peer);
        dial_media(&self.inner.endpoint, peer).await
    }

    /// Most peers `contacted_within` remembers. The ones dialled longest ago are dropped first.
    const MAX_CONTACTED: usize = 4096;

    fn note_contacted(&self, peer: DeviceId) {
        let mut contacted = self.inner.contacted.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if contacted.len() >= Self::MAX_CONTACTED && !contacted.contains_key(&peer.0) {
            if let Some(oldest) = contacted.iter().min_by_key(|(_, at)| **at).map(|(id, _)| *id) {
                contacted.remove(&oldest);
            }
        }
        contacted.insert(peer.0, Instant::now());
    }

    /// Whether this node dialled `peer`, for a request or a media link, within `window`. Such a peer
    /// already knows this node is online.
    pub fn contacted_within(&self, peer: DeviceId, window: Duration) -> bool {
        self.inner
            .contacted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&peer.0)
            .is_some_and(|at| at.elapsed() < window)
    }

    /// Joins a gossip topic, dialling `bootstrap` peers. Peers must be known via `remember` or dialable.
    pub async fn subscribe(
        &self,
        topic: TopicId,
        bootstrap: Vec<DeviceId>,
    ) -> Result<(TopicSender, TopicReceiver)> {
        let peers = bootstrap
            .iter()
            .map(|d| EndpointId::from_bytes(&d.0).map_err(|e| NetError::BadPeer(e.to_string())))
            .collect::<Result<Vec<_>>>()?;
        let sub = self
            .inner
            .gossip
            .subscribe(iroh_gossip::TopicId::from_bytes(topic.0), peers)
            .await?;
        let (tx, rx) = sub.split();
        Ok((TopicSender::new(tx, self.inner.gossip_max), TopicReceiver(rx)))
    }

    pub async fn shutdown(self) -> Result<()> {
        if let Some(task) = self.inner.lan_task.lock().expect("LAN task lock").take() {
            task.abort();
        }
        // The router shuts each protocol handler down, gossip included, and then closes the endpoint.
        // Gossip must not be shut down twice, since the second call warns that its actor is gone.
        self.inner.router.shutdown().await.map_err(|e| NetError::Shutdown(e.to_string()))
    }
}

/// Counts one open relay probe while it lives.
struct OpenProbe<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> OpenProbe<'a> {
    fn new(count: &'a std::sync::atomic::AtomicUsize) -> Self {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for OpenProbe<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
