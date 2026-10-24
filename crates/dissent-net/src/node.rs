use std::{
    fmt,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use dissent_identity::{DeviceId, DeviceKey};
use iroh::{
    Endpoint, EndpointId, RelayConfig, RelayMode, RelayUrl, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{default_relay_mode, presets},
    protocol::Router,
    tls::CaTlsConfig,
};
use futures_lite::{Stream, StreamExt};
use iroh::Watcher;
use iroh::protocol::DynProtocolHandler;
use iroh_gossip::{ALPN as GOSSIP_ALPN, Gossip};
use iroh_mainline_address_lookup::DhtAddressLookup;
use url::Url;

use crate::{
    addr::PeerAddr,
    extra::{self, Shared},
    media::MEDIA_ALPN,
    error::{NetError, Result},
    media::{MediaHandler, MediaLink, MediaProtocol, dial_media},
    ping::{PING_ALPN, PingHandler, PingReport, run_ping},
    rpc::{RPC_ALPN, RpcHandler, RpcProtocol, run_request},
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
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            relay: RelayChoice::Default,
            bind_port: 0,
            rpc: None,
            dht_lookup: true,
            loopback_only: false,
            media: None,
            gossip_max_message: DEFAULT_GOSSIP_MAX_MESSAGE,
            extra_protocols: Vec::new(),
            relay_ca: None,
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
            .field("media", &self.media.as_ref().map(|_| "MediaHandler"))
            .field("gossip_max_message", &self.gossip_max_message)
            .field(
                "extra_protocols",
                &self.extra_protocols.iter().map(|(alpn, _)| alpn).collect::<Vec<_>>(),
            )
            .field("loopback_only", &self.loopback_only)
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
    lookup: MemoryLookup,
    gossip_max: usize,
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
        let gossip_max = cfg.gossip_max_message;
        let lookup = MemoryLookup::new();
        let use_dht = cfg.dht_lookup && !matches!(cfg.relay, RelayChoice::Disabled);
        let preset = match cfg.relay {
            RelayChoice::Default => Endpoint::builder(presets::N0),
            RelayChoice::Custom(_) | RelayChoice::Disabled => Endpoint::builder(presets::Minimal),
        };
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
            .address_lookup(lookup.clone())
            .crypto_provider(provider);
        if use_dht {
            builder = builder.address_lookup(DhtAddressLookup::builder());
        }
        builder = match cfg.relay {
            RelayChoice::Default => builder.relay_mode(RelayMode::Default),
            RelayChoice::Disabled => builder.relay_mode(RelayMode::Disabled),
            RelayChoice::Custom(urls) => {
                builder.relay_mode(RelayMode::custom(urls.into_iter().map(RelayUrl::from)))
            }
        };
        if let Some(ca) = cfg.relay_ca {
            builder = builder.ca_tls_config(ca);
        }
        if cfg.loopback_only {
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
        run_ping(&self.inner.endpoint, peer.to_iroh()?).await
    }

    /// Sends `req` to `peer` and returns its reply. Dials by id, so addresses must be
    /// remembered or discoverable. Fails with `Timeout` after 15 s.
    pub async fn request(&self, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>> {
        run_request(&self.inner.endpoint, peer, req).await
    }

    /// Dials `peer` and opens an unreliable datagram link for call media. The peer sees it via
    /// `NodeConfig.media`.
    pub async fn open_media(&self, peer: DeviceId) -> Result<MediaLink> {
        dial_media(&self.inner.endpoint, peer).await
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
        // The router shuts each protocol handler down, gossip included, and then closes the endpoint.
        // Gossip must not be shut down twice, since the second call warns that its actor is gone.
        self.inner.router.shutdown().await.map_err(|e| NetError::Shutdown(e.to_string()))
    }
}
