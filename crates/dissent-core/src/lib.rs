//! Dissent engine: communities with channels, users across communities, and direct messages.
//!
//! The engine owns the disk layout, one gossip topic per community, the RPC handler and the
//! DM outbox. It has no Tauri dependency. See `docs/design/02-communities-and-dms.md`.

#[cfg(test)]
mod testutil;
pub mod test_support;
mod access;
mod addr_record;
mod announce;
mod avatars;
mod attachments;
mod call_layers;
mod call_video;
mod calls;
mod channels;
mod identity_flows;
mod local;
mod communities;
mod epoch_keys;
mod private_keys;
#[cfg(test)]
mod private_create_tests;
mod contact_times;
mod contacts;
mod conv_actions;
mod dht_probe;
mod community;
mod community_profile;
mod animated_banner;
pub mod boosts;
mod profile_banners;
mod profile_sync;
mod membership;
mod peer_exchange;
mod paging;
mod peer_lookup;
mod reseal;
mod sealed;
mod directory;
mod dm;
mod engine;
mod identity_watch;
mod error;
mod groups;
mod inbox;
mod dm_devices;
mod friends;
mod history;
mod mirror_hint;
mod invite_preview;
mod invites;
mod presence;
mod previews;
mod mention_reads;
mod messages;
mod net;
mod rail_order;
mod reactions;
mod relays;
pub use relays::HostedRelay;
mod rotation;
mod rpc;
mod saved;
mod server_prefs;
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
pub use dissent_net::{merge_relays, NodeConfig, RelayChoice, RelayEntry, RelayReport, RelaySource, MAX_RELAY_LIST};
pub use contacts::{ContactEntry, ContactRequestView, ContactsView, DmPolicy, MAX_NOTE_CHARS};
pub use calls::{
    CallQuality, CallRoute, CallScope, CallView, LiveCallView, LiveMember, LiveParticipant, LiveRoute,
    ParticipantView, PathView, Topology,
};
pub use attachments::{AttachmentView, Mentions, StoreStats, TransferSettings, TransferView};
pub use engine::Engine;
pub use calls::{EncodedSink, EncodedVideo};
pub use store::{sealed_data_present, SEALED_WITHOUT_IDENTITY};
pub use dissent_audio::SenderConfig;
pub use local::{NoteKind, CAPABILITIES};
pub use groups::{ConvId, MAX_GROUP_MEMBERS, MAX_GROUP_NAME_CHARS, MIN_GROUP_MEMBERS};
pub use inbox::{InboxStats, MAX_INBOXES};
pub use friends::FriendView;
pub use saved::{Bookmark, MessageRef, Reminder};
pub use server_prefs::{Notify, ServerPrefs, MUTED_FOREVER};
pub use invite_preview::{InviteStatus, InvitePreview, InvitePreviewView, SignedPreview};
pub use invites::{JoinRequestView, GROUP_INVITE_TTL_MS};
pub use previews::Embed;
pub use timings::Timings;
pub use presence::{Presence, Status};
pub use typing::TypingTarget;
pub use identity_flows::{start_device_link, DeviceView, GuardianEntry, GuardianView, PendingDevice};
pub use reseal::copy_profile;
pub use error::{CoreError, Result};
pub use peer_exchange::AddrAnswer;
pub use views::*;
pub use call_video::{list_share_sources, ShareSource, ShareTarget, TestCapture};
pub use channels::{parse_invite_id, ChannelPatch};
pub use dissent_events::{ChannelVisibility, HistorySetting, Role, Trait};
pub use animated_banner::{CropRect, EncodedBanner};
pub use epoch_keys::MemberOnlyStatus;
