//! Dissent engine: communities with channels, users across communities, and direct messages.
//!
//! The engine owns the disk layout, one gossip topic per community, the RPC handler and the
//! DM outbox. It has no Tauri dependency. See `docs/design/02-communities-and-dms.md`.

#[cfg(test)]
mod testutil;
pub mod test_support;
mod access;
mod announce;
mod attachments;
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
mod invites;
mod presence;
mod previews;
mod messages;
mod net;
mod reactions;
mod relays;
mod rotation;
mod rpc;
mod store;
mod sync;
mod timings;
mod typing;
mod views;
mod warm;
mod wire;
mod world;

pub use dissent_discovery::DhtStats;
pub use dissent_events::{ChannelId, EventId};
pub use dissent_identity::{CommunityId, DeviceId, Identity, RootId};
pub use dissent_net::{NodeConfig, RelayChoice};
pub use contacts::{ContactEntry, ContactRequestView, ContactsView, DmPolicy, MAX_NOTE_CHARS};
pub use calls::{
    CallQuality, CallRoute, CallScope, CallView, LiveCallView, LiveMember, LiveParticipant, LiveRoute,
    ParticipantView, PathView, Topology,
};
pub use attachments::{AttachmentView, StoreStats, TransferSettings, TransferView};
pub use engine::Engine;
pub use dissent_audio::SenderConfig;
pub use local::{NoteKind, CAPABILITIES};
pub use groups::{ConvId, MAX_GROUP_MEMBERS, MAX_GROUP_NAME_CHARS, MIN_GROUP_MEMBERS};
pub use inbox::{InboxStats, MAX_INBOXES};
pub use invites::{JoinRequestView, GROUP_INVITE_TTL_MS};
pub use previews::Embed;
pub use timings::Timings;
pub use presence::{Presence, Status};
pub use typing::TypingTarget;
pub use identity_flows::{start_device_link, DeviceView, GuardianEntry, GuardianView, PendingDevice};
pub use error::{CoreError, Result};
pub use views::*;
