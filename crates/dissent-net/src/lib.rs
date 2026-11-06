//! iroh endpoint wrapper: dial by DeviceId, ping ALPN, gossip per channel.

mod addr;
mod error;
mod extra;
mod inbound;
mod lan;
mod node;
mod media;
mod ping;
mod relay_rank;
mod rpc;
pub mod socks_bridge;
mod source;
mod tls;
mod topic;

pub use addr::PeerAddr;
pub use error::{NetError, Result};
pub use node::{DEFAULT_GOSSIP_MAX_MESSAGE, Egress, Node, NodeConfig, RelayChoice, RelayReport};
pub use relay_rank::{merge as merge_relays, rank as rank_relays, Entry as RelayEntry, Source as RelaySource, MAX_LIST as MAX_RELAY_LIST};
pub use media::{LinkToken, MediaHandler, MediaLink, LINK_TOKEN_WAIT};
pub use inbound::InboundLimits;
pub use rpc::{RpcHandler, REQUEST_TIMEOUT};
pub use socks_bridge::SocksBridge;
pub use lan::community_key as lan_key;
pub use ping::{PathKind, PingReport};
pub use source::{CACHE, DHT, DialSample, DialStats, LAN, OPEN, PEER, SEEDER};
pub use topic::{GossipMsg, TopicId, TopicReceiver, TopicSender};
