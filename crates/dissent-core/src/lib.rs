//! Dissent engine: communities with channels, users across communities, and direct messages.
//!
//! The engine owns the disk layout, one gossip topic per community, the RPC handler and the
//! DM outbox. It has no Tauri dependency. See `docs/design/02-communities-and-dms.md`.

mod announce;
mod communities;
mod community;
mod directory;
mod dm;
mod engine;
mod error;
mod messages;
mod net;
mod rpc;
mod store;
mod sync;
mod views;
mod wire;
mod world;

pub use dissent_discovery::DhtStats;
pub use dissent_events::{ChannelId, EventId};
pub use dissent_identity::{CommunityId, DeviceId, Identity, RootId};
pub use dissent_net::{NodeConfig, RelayChoice};
pub use engine::Engine;
pub use error::{CoreError, Result};
pub use views::*;
