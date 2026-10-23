//! iroh endpoint wrapper: dial by DeviceId, ping ALPN, gossip per channel.

mod addr;
mod error;
mod node;
mod ping;
mod rpc;
mod topic;

pub use addr::PeerAddr;
pub use error::{NetError, Result};
pub use node::{Node, NodeConfig, RelayChoice};
pub use rpc::RpcHandler;
pub use ping::{PathKind, PingReport};
pub use topic::{GossipMsg, TopicId, TopicReceiver, TopicSender};
