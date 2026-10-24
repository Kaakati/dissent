//! Dissent engine: communities with channels, users across communities, and direct messages.
//!
//! The engine owns the disk layout, one gossip topic per community, the RPC handler and the
//! DM outbox. It has no Tauri dependency. See `docs/design/02-communities-and-dms.md`.

#[cfg(test)]
mod testutil;
mod access;
mod announce;
mod calls;
mod identity_flows;
mod local;
mod communities;
mod contacts;
mod community;
mod membership;
mod sealed;
mod directory;
mod dm;
mod engine;
mod identity_watch;
mod error;
mod groups;
mod inbox;
mod messages;
mod net;
mod reactions;
mod rpc;
mod store;
mod sync;
mod views;
mod warm;
mod wire;
mod world;

pub use dissent_discovery::DhtStats;
pub use dissent_events::{ChannelId, EventId};
pub use dissent_identity::{CommunityId, DeviceId, Identity, RootId};
pub use dissent_net::{NodeConfig, RelayChoice};
pub use contacts::{ContactEntry, ContactRequestView, ContactsView, DmPolicy, MAX_NOTE_CHARS};
pub use calls::{CallQuality, CallRoute, CallScope, CallView, ParticipantView, PathView, Topology};
pub use engine::Engine;
pub use dissent_audio::SenderConfig;
pub use local::{NoteKind, CAPABILITIES};
pub use groups::{ConvId, MAX_GROUP_MEMBERS, MAX_GROUP_NAME_CHARS, MIN_GROUP_MEMBERS};
pub use inbox::{InboxStats, MAX_INBOXES};
pub use identity_flows::{start_device_link, DeviceView, GuardianEntry, GuardianView, PendingDevice};
pub use error::{CoreError, Result};
pub use views::*;
