use std::{fmt, net::SocketAddr, sync::Arc};

use dissent_identity::{DeviceId, DeviceKey};
use iroh::{
    Endpoint, EndpointId, RelayMode, RelayUrl, SecretKey, address_lookup::MemoryLookup,
    endpoint::presets, protocol::Router,
};
use futures_lite::{Stream, StreamExt};
use iroh::Watcher;
use iroh_gossip::{ALPN as GOSSIP_ALPN, Gossip};
use iroh_mainline_address_lookup::DhtAddressLookup;
use url::Url;

use crate::{
    addr::PeerAddr,
    error::{NetError, Result},
    ping::{PING_ALPN, PingHandler, PingReport, run_ping},
    rpc::{RPC_ALPN, RpcHandler, RpcProtocol, run_request},
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
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self { relay: RelayChoice::Default, bind_port: 0, rpc: None, dht_lookup: true, loopback_only: false }
    }
}

impl fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeConfig")
            .field("relay", &self.relay)
            .field("bind_port", &self.bind_port)
            .field("rpc", &self.rpc.as_ref().map(|_| "RpcHandler"))
            .field("dht_lookup", &self.dht_lookup)
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
    endpoint: Endpoint,
    router: Router,
    gossip: Gossip,
    lookup: MemoryLookup,
}

impl Node {
    /// Starts the endpoint with the device key as its iroh secret key, so the node id equals `DeviceId`.
    pub async fn spawn(device: &DeviceKey, cfg: NodeConfig) -> Result<Node> {
        let lookup = MemoryLookup::new();
        let use_dht = cfg.dht_lookup && !matches!(cfg.relay, RelayChoice::Disabled);
        let preset = match cfg.relay {
            RelayChoice::Default => Endpoint::builder(presets::N0),
            RelayChoice::Custom(_) | RelayChoice::Disabled => Endpoint::builder(presets::Minimal),
        };
        let mut builder = preset
            .secret_key(SecretKey::from_bytes(&device.0.seed()))
            .address_lookup(lookup.clone());
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
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let mut router = Router::builder(endpoint.clone())
            .accept(PING_ALPN, PingHandler)
            .accept(GOSSIP_ALPN, gossip.clone());
        if let Some(handler) = cfg.rpc {
            router = router.accept(RPC_ALPN, RpcProtocol(handler));
        }
        let router = router.spawn();
        Ok(Node {
            inner: Arc::new(Inner { id: device.id(), endpoint, router, gossip, lookup }),
        })
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
        Ok((TopicSender(tx), TopicReceiver(rx)))
    }

    pub async fn shutdown(self) -> Result<()> {
        self.inner.gossip.shutdown().await.map_err(|e| NetError::Shutdown(e.to_string()))?;
        // The router also closes the endpoint.
        self.inner.router.shutdown().await.map_err(|e| NetError::Shutdown(e.to_string()))
    }
}
