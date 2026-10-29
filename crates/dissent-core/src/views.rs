//! Serialisable views handed to the UI. Ids are base32 strings and fields are snake_case.

use dissent_discovery::DhtStats;
use dissent_events::state::ChannelSettings;
use dissent_events::{ChannelId, ChannelVisibility};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct MeView {
    pub root_id: String,
    pub device_id: String,
    pub suffix: String,
    pub display_name: String,
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
    /// Path of the profile banner's picture on this device, when the banner is one. See `profile_banners`.
    pub banner: Option<String>,
    /// Path of an animated banner's still frame, shown under reduced motion.
    pub banner_poster: Option<String>,
    /// The preset gradient the profile banner is, when it is one.
    pub banner_preset: Option<u8>,
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
    /// Path of the decrypted icon on this device, once fetched. `None` when there is none yet.
    pub icon: Option<String>,
    /// Path of an animated icon's still frame, shown under reduced motion and while the window is in
    /// the background. `None` for a still icon.
    pub icon_poster: Option<String>,
    /// Unread mentions of me in the community's text channels, since each was last marked read.
    pub mentions: u32,
    /// Whether this device's user may mention everyone here: the owner and admins.
    pub can_mention_everyone: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommunityView {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub my_role: String,
    /// Path of the decrypted icon on this device, once fetched. `None` when there is none yet.
    pub icon: Option<String>,
    /// Path of an animated icon's still frame, shown under reduced motion and while the window is in
    /// the background. `None` for a still icon.
    pub icon_poster: Option<String>,
    /// Path of the decrypted banner on this device, once fetched. `None` when there is none yet.
    pub banner: Option<String>,
    /// Path of an animated banner's still frame, shown under reduced motion.
    pub banner_poster: Option<String>,
    /// The preset gradient the banner is, when it is one. The UI draws presets itself.
    pub banner_preset: Option<u8>,
    /// The server's boost, as this app verifies its relay hosts. See `boosts`.
    pub boost: crate::boosts::BoostView,
    /// The description from Server Settings, empty when there is none.
    pub description: String,
    /// The traits from Server Settings, at most five.
    pub traits: Vec<dissent_events::Trait>,
    /// When the community was created, in ms since the Unix epoch, from its genesis.
    pub created_ms: u64,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    /// Relays the community asks members to use, from its latest `SetRelays`.
    pub relays: Vec<String>,
    /// Whether control events are member-sealed, so storage members cannot read them. See
    /// `Engine::set_member_only`.
    pub member_only: bool,
    /// Who a joiner can read past messages of: `since_join` or `all`. See `set_history_visibility`.
    pub history: String,
    /// Whether some events stay unreadable here until a member sends the key for their epoch. The
    /// UI shows "Waiting for a member to unlock history" while it is set.
    pub history_locked: bool,
    /// Each voice channel with the people in its call, as far as this device has heard. Includes
    /// people who are not in the call themselves.
    pub voice_channels: Vec<VoiceChannelView>,
    /// Whether this device's user may mention everyone here: the owner and admins.
    pub can_mention_everyone: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    /// True for a voice channel, made with `create_voice_channel`.
    pub voice: bool,
    pub slowmode_s: u32,
    /// `default`, `spoiler` or `age`.
    pub visibility: &'static str,
    pub bitrate_kbps: u32,
    /// Zero is no limit.
    pub user_limit: u32,
    /// The relay URL, or empty for automatic.
    pub relay: String,
    /// Unread mentions of me in this channel, since it was last marked read. Zero for voice channels.
    pub mentions: u32,
    /// True for a private channel, which only its listed readers see. The client shows it with a lock.
    pub private: bool,
}

/// Who may read a channel, for its settings picker. Roles are `admin` or `moderator`; members are root ids.
#[derive(Clone, Debug, Serialize)]
pub struct ChannelAccessView {
    pub private: bool,
    pub roles: Vec<&'static str>,
    pub members: Vec<String>,
}

impl ChannelInfo {
    pub(crate) fn new(id: ChannelId, name: String, voice: bool, settings: &ChannelSettings) -> Self {
        Self {
            id: id.to_string(),
            name,
            voice,
            slowmode_s: settings.slowmode_s,
            visibility: match settings.visibility {
                ChannelVisibility::Default => "default",
                ChannelVisibility::Spoiler => "spoiler",
                ChannelVisibility::AgeRestricted => "age",
            },
            bitrate_kbps: settings.bitrate_kbps,
            user_limit: settings.user_limit,
            relay: settings.relay.clone(),
            mentions: 0,
            private: false,
        }
    }
}

/// A voice channel with the people in its call.
#[derive(Clone, Debug, Serialize)]
pub struct VoiceChannelView {
    pub id: String,
    pub name: String,
    pub participants: Vec<VoiceParticipantView>,
}

/// One person in a voice channel, as this device has heard it.
#[derive(Clone, Debug, Serialize)]
pub struct VoiceParticipantView {
    pub root_id: String,
    pub display_name: String,
    pub muted: bool,
    pub deafened: bool,
    /// Speaking is only known for people this device is connected to, so it is false here.
    pub speaking: bool,
}

/// An invite this device made, with what the community knows of it.
#[derive(Clone, Debug, Serialize)]
pub struct InviteView {
    pub id: String,
    pub link: String,
    pub created_ms: u64,
    pub expires_ms: Option<u64>,
    /// Accepted joins through the invite, as this device's replay counts them.
    pub uses: u32,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemberInfo {
    pub root_id: String,
    pub suffix: String,
    pub display_name: String,
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
    pub handle: Option<String>,
    pub role: String,
    pub banned: bool,
    /// What others see of this member: their status, or offline.
    pub status: crate::presence::Presence,
    /// Whether this app verifies the member as a relay host of this server. See `boosts`.
    pub relay_host: bool,
    /// Whether the member is a seeder: a storage member, outside the encrypted group.
    pub storage: bool,
    /// For a seeder: whether an admin granted it storage (`StorageGrant`). False for a person.
    pub storage_granted: bool,
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
    /// True when `locked` and the message was sent before this device joined. The UI says so, and that
    /// the server's history setting decides what joiners read.
    pub before_join: bool,
    /// The message this one replies to, if any.
    pub reply_to: Option<ReplyPreview>,
    /// Reactions in the order each emoji first got one. Empty when there are none.
    pub reactions: Vec<ReactionView>,
    /// Files in the message, with their state on this device.
    pub attachments: Vec<crate::attachments::AttachmentView>,
    /// Link previews the sender made.
    pub embeds: Vec<crate::previews::EmbedView>,
    /// True when this message mentions me, by name or with `@everyone` the sender could use. Only
    /// members are counted, see `mentions`.
    pub mentions_me: bool,
    /// The people this message mentions who are members of the channel's community, resolved to names.
    pub mentions: Vec<MentionView>,
    /// True when the message's `@everyone` counts: the author could mention everyone when it was accepted.
    pub mentions_everyone: bool,
}

/// A person a message mentions, for the mention pill.
#[derive(Clone, Debug, Serialize)]
pub struct MentionView {
    pub id: String,
    pub display_name: String,
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
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
    /// Path of the profile banner's picture on this device, once fetched, when the banner is one.
    pub banner: Option<String>,
    /// Path of an animated banner's still frame, shown under reduced motion.
    pub banner_poster: Option<String>,
    /// The preset gradient the profile banner is, when it is one. Neither set: the hue colour.
    pub banner_preset: Option<u8>,
    pub is_contact: bool,
    pub handles: Vec<HandleView>,
    pub mutual_communities: Vec<CommunityRef>,
    /// The shared servers this app verifies the person as a relay host of. See `boosts`.
    pub hosts_for: Vec<CommunityRef>,
    /// Our contacts that they also have, when they share their list with us. Filled by
    /// `Engine::user_profile_full`; empty otherwise.
    pub mutual_friends: Vec<crate::friends::FriendView>,
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
    /// True if this device ignores the person. Their messages still arrive, collapsed, and never notify.
    pub ignored: bool,
    /// What others see of this person: their status, or offline.
    pub status: crate::presence::Presence,
    /// When this person was added as a contact, in milliseconds. `None` if they are not one.
    pub contact_since_ms: Option<u64>,
    /// When this device first saw this person, in milliseconds. `None` if it never has.
    pub first_seen_ms: Option<u64>,
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
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
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
    /// The other party's status in a 1:1 DM. `None` for a group.
    pub status: Option<crate::presence::Presence>,
    /// The other party's picture in a 1:1 DM. `None` for a group, or when there is none yet.
    pub avatar: Option<String>,
    /// The call in this conversation that another member's device says is live. `None` when none is.
    pub live_call: Option<crate::calls::LiveCallView>,
    /// When the other party was added as a contact, in milliseconds. `None` for a group or a non-contact.
    pub contact_since_ms: Option<u64>,
    /// When this device first saw the other party, in milliseconds. `None` for a group.
    pub first_seen_ms: Option<u64>,
    /// Unread messages from others since the conversation was last marked read, counted up to 100.
    /// Computed when the summary is built, never stored.
    pub unread: u32,
    /// Unread messages from others that mention this device, counted up to 100. Part of `unread`. See `dm_mentions.rs`.
    pub mentions: u32,
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
    /// True while a refused message can still be sent again. It stops after the envelope lifetime.
    pub retryable: bool,
    /// Files in the message.
    pub attachments: Vec<crate::attachments::AttachmentView>,
    /// Link previews the sender made.
    pub embeds: Vec<crate::previews::EmbedView>,
    /// The message this one replies to, if any. See `conv_actions.rs`.
    pub reply_to: Option<ReplyPreview>,
    /// Reactions in the order each emoji first got one. Empty when there are none.
    pub reactions: Vec<ReactionView>,
    /// True once the author edited the message.
    pub edited: bool,
    /// True once the author deleted the message. `text` is empty then.
    pub deleted: bool,
    /// True when the message mentions this device and someone else wrote it. See `dm_mentions.rs`.
    pub mentions_me: bool,
    /// The members the message mentions, for the mention pills.
    pub mentions: Vec<MentionView>,
}

/// Change notifications. Serialised as `{"kind": "...", ...}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    CommunitiesChanged,
    /// A profile picture or a community icon changed, or was fetched. Exactly one of the two ids is set.
    AvatarChanged {
        root_id: Option<String>,
        community_id: Option<String>,
    },
    CommunityUpdated { community_id: String },
    /// A new message in a channel. Alerts and sounds are decided from this event alone.
    Message { community_id: String, channel_id: String },
    /// Messages in a channel changed, and no new one came: an edit, a delete, a reaction, or history keys that
    /// made older messages readable. The view re-reads the channel. Nothing alerts.
    MessagesChanged { community_id: String, channel_id: String },
    /// A new direct message came from this peer.
    Dm { peer: String },
    /// Direct messages with this peer changed, and none came from the peer: our own message was delivered, or the
    /// conversation's details changed. The view re-reads it. Nothing alerts.
    DmChanged { peer: String },
    /// A new text message in a group. Alerts are decided from this event alone.
    Group { group_id: String },
    /// A conversation changed: a message, a rename, a member added or removed, or a new group.
    ConversationChanged { id: String },
    /// Someone asked to join a group with an invite link. Approve or decline it.
    GroupJoinRequest { conversation: String, user: String },
    /// A download or upload moved. Matches `dissent://transfers`.
    TransfersChanged,
    /// An attachment's thumbnail arrived. The UI re-reads the open conversation, without an alert.
    AttachmentChanged { message_id: String, attachment_id: String },
    /// A file for an attachment moved. `progress` is `0..1`.
    AttachmentProgress {
        message_id: String,
        attachment_id: String,
        progress: f64,
    },
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
        /// The group conversation of a group call. `None` for a direct call.
        conv: Option<String>,
    },
    /// A direct call rang out, or the caller gave up. It counts as missed.
    CallMissed { call_id: String, from: String },
    /// Another device of the same person answered this call, so this one stopped ringing.
    CallAnsweredElsewhere { call_id: String },
    /// Our status changed, by a set, an expiry, or auto-idle. `until_ms` is the end of a timed status.
    StatusChanged {
        status: crate::presence::Status,
        until_ms: Option<u64>,
    },
    /// What we show for `user` changed because a status beacon arrived. A person going quiet emits nothing.
    PresenceChanged { user: String },
    /// Who is typing in `target` changed: someone started, stopped, sent, or timed out.
    TypingChanged { target: crate::typing::TypingTarget },
    /// A preview of a camera or a shared screen, from this device or a peer. `jpeg` is a JPEG of
    /// the picture, base64 encoded. `root_id` names the person, so the window can place the tile.
    VideoFrame {
        sender: u64,
        root_id: String,
        screen: bool,
        /// Whether the picture was encoded with AV1. False for H.264, and for this device's own preview.
        av1: bool,
        width: u32,
        height: u32,
        jpeg: String,
    },
    /// A reminder's time has come. It is removed from the list as it fires. Appended, see `saved`.
    ReminderDue { reminder: crate::saved::Reminder },
    /// The latest latency measurements for the status bar, in milliseconds: the median DHT lookup time and the home relay's
    /// round trip. `None` when there is no reading. Sent every DHT probe, so the bar updates without polling.
    NetLatency { dht_ms: Option<u64>, relay_ms: Option<u64> },
    /// A seeder asked this owner or admin device for a storage grant it has not asked for before. Sent
    /// once per ask, so the UI can say so once. See `storage_grants`.
    StorageGrantAsked {
        community_id: String,
        community_name: String,
        root_id: String,
        display_name: String,
    },
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
    /// The home relay's round-trip time in milliseconds, from the latest net report. `None` when unknown.
    pub relay_ms: Option<u64>,
    /// How dials were resolved since start. See `DialsView`.
    pub dials: DialsView,
}

/// How dials were resolved: successful dials per address source, failed dials, and the latest dial.
/// A source of `dht` means the Mainline DHT answered. The sources are `cache`, `peer`, `seeder`,
/// `lan`, `dht` and `open` (a connection that was already up).
#[derive(Clone, Debug, Default, Serialize)]
pub struct DialsView {
    pub by_source: std::collections::BTreeMap<String, u64>,
    pub failed: u64,
    /// The source of the most recent successful dial.
    pub last_via: Option<String>,
    /// Milliseconds from the start of the most recent successful dial to its reply.
    pub last_connect_ms: Option<u64>,
}
