//! iroh endpoint wrapper: dial by DeviceId, ping ALPN, gossip per channel.

mod addr;
mod error;
mod extra;
mod lan;
mod node;
mod media;
mod ping;
mod rpc;
mod source;
mod tls;
mod topic;

pub use addr::PeerAddr;
pub use error::{NetError, Result};
pub use node::{DEFAULT_GOSSIP_MAX_MESSAGE, Node, NodeConfig, RelayChoice};
pub use media::{MediaHandler, MediaLink};
pub use rpc::RpcHandler;
pub use lan::community_key as lan_key;
pub use ping::{PathKind, PingReport};
pub use source::{CACHE, DHT, DialSample, DialStats, LAN, OPEN, PEER, SEEDER};
pub use topic::{GossipMsg, TopicId, TopicReceiver, TopicSender};
