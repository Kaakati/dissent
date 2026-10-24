//! Serialisable views handed to the UI. Ids are base32 strings and fields are snake_case.

use dissent_discovery::DhtStats;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct MeView {
    pub root_id: String,
    pub device_id: String,
    pub suffix: String,
    pub display_name: String,
    /// True until the recovery phrase has been shown on a fresh device. See `capabilities`.
    pub first_run: bool,
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
    /// True for a voice channel, made with `create_voice_channel`.
    pub voice: bool,
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
    /// True when the text is encrypted and this device holds no key for it, for example a message
    /// from before this device joined. `text` is empty then.
    pub locked: bool,
    /// The message this one replies to, if any.
    pub reply_to: Option<ReplyPreview>,
    /// Reactions in the order each emoji first got one. Empty when there are none.
    pub reactions: Vec<ReactionView>,
}

/// A short view of the message a reply points at.
#[derive(Clone, Debug, Serialize)]
pub struct ReplyPreview {
    pub id: String,
    /// Root id of the original author, for colouring the avatar.
    pub author: String,
    pub author_name: String,
    /// About 120 characters of the original. `None` if it was deleted or this device cannot read it.
    pub text: Option<String>,
    /// True when the original exists but this device cannot decrypt it.
    pub locked: bool,
}

/// One emoji on a message, with how many people used it.
#[derive(Clone, Debug, Serialize)]
pub struct ReactionView {
    pub emoji: String,
    pub count: usize,
    /// True when this user reacted with it.
    pub me: bool,
    /// Up to three display names of the people who reacted, for a tooltip.
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserView {
    pub root_id: String,
    pub suffix: String,
    pub display_name: String,
    pub is_contact: bool,
    pub handles: Vec<HandleView>,
    pub mutual_communities: Vec<CommunityRef>,
    /// When this person last changed identity, in milliseconds. `None` if they never did.
    pub identity_changed_ms: Option<u64>,
    /// When the key changed and was not yet compared with the safety number. `None` once verified.
    pub key_changed_ms: Option<u64>,
    /// When the safety number was last compared, in milliseconds.
    pub verified_ms: Option<u64>,
    /// A private note about this person, from this device only.
    pub note: Option<String>,
    /// A name only this device shows for this person.
    pub nickname: Option<String>,
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

/// One member of a conversation, as the UI lists it.
#[derive(Clone, Debug, Serialize)]
pub struct ConvMember {
    pub root_id: String,
    pub display_name: String,
    pub suffix: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationSummary {
    /// Conversation id. A 1:1 DM has an id derived from the two roots.
    pub id: String,
    /// `dm` or `group`.
    pub kind: String,
    /// The other party of a 1:1 DM. Empty for a group.
    pub peer: String,
    /// Every member, us included.
    pub members: Vec<ConvMember>,
    /// The group name. For a 1:1 DM, the other party's display name.
    pub name: String,
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
    /// `sending`, `sent_direct`, `queued_inbox`, `delivered` or `failed` for messages we sent. A message
    /// from someone else is `delivered`.
    pub delivery: String,
    /// True while the message waits in an inbox for the recipient to collect it.
    pub queued: bool,
    /// Why the recipient refused this message, when `delivery` is `failed`.
    pub reason: Option<String>,
}

/// Change notifications. Serialised as `{"kind": "...", ...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    CommunitiesChanged,
    CommunityUpdated { community_id: String },
    Message { community_id: String, channel_id: String },
    Dm { peer: String },
    /// A conversation changed: a message, a rename, a member added or removed, or a new group.
    ConversationChanged { id: String },
    /// Someone asked to become a contact. Answer with `accept_request` or `decline_request`.
    ContactRequest { user: String },
    /// Contacts or contact requests changed.
    ContactsChanged,
    /// Someone we share a server with, or have messaged, changed identity. `reason` is "rotated"
    /// or "new_device". The safety number may need checking again.
    IdentityChanged { user: String, reason: String },
    /// The call on this device changed: a participant joined, left, muted, or the link quality moved.
    CallUpdated,
    /// Someone is calling us directly. Nothing connects until we accept with `accept_call`.
    IncomingCall {
        call_id: String,
        from: String,
        scope: crate::calls::CallScope,
    },
    /// A direct call rang out, or the caller gave up. It counts as missed.
    CallMissed { call_id: String, from: String },
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
