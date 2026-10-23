//! Serialisable views handed to the UI. Ids are base32 strings and fields are snake_case.

use dissent_discovery::DhtStats;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct MeView {
    pub root_id: String,
    pub device_id: String,
    pub suffix: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommunitySummary {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// One of `owner`, `admin`, `moderator`, `member`.
    pub my_role: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommunityView {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub my_role: String,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemberInfo {
    pub root_id: String,
    pub suffix: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub role: String,
    pub banned: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MessageItem {
    pub id: String,
    pub author: String,
    pub author_name: String,
    pub text: String,
    pub ms: u64,
    pub edited: bool,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserView {
    pub root_id: String,
    pub suffix: String,
    pub display_name: String,
    pub is_contact: bool,
    pub handles: Vec<HandleView>,
    pub mutual_communities: Vec<CommunityRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HandleView {
    pub community_id: String,
    pub community_name: String,
    pub handle: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommunityRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationSummary {
    pub peer: String,
    pub display_name: String,
    pub last_text: String,
    pub last_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DmItem {
    pub id: String,
    pub from: String,
    pub text: String,
    pub ms: u64,
    /// True while the message sits in the outbox waiting for a delivery acknowledgement.
    pub pending: bool,
}

/// Change notifications. Serialised as `{"kind": "...", ...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    CommunitiesChanged,
    CommunityUpdated { community_id: String },
    Message { community_id: String, channel_id: String },
    Dm { peer: String },
}

/// Moderation actions. Serialised as `ban`, `grant_admin`, and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModAction {
    Ban,
    Unban,
    GrantAdmin,
    RevokeAdmin,
    GrantModerator,
    RevokeModerator,
}

/// Network state for the status bar.
#[derive(Clone, Debug, Serialize)]
pub struct NetStatusView {
    /// Mainline DHT stats. `None` when relays are disabled or the DHT could not be read.
    pub dht: Option<DhtStats>,
    /// Distinct devices that are direct gossip neighbours in any community.
    pub peers: usize,
    /// Host of our home relay, when we have one.
    pub relay: Option<String>,
}
