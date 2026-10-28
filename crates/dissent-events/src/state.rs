//! Deterministic replay of the causal order into community state.
//!
//! Each event is checked against the state at its position in the causal
//! order. Rejected events stay in the DAG, so later events that depend on
//! them still apply. A ban is in force from its position onward, so a
//! concurrent post that sorts after it is rejected and one that sorts before it stays.
//!
//! Permission ranks: Owner > Admin > Moderator > User.
//! * Owner: anything, including granting Admin. Cannot be banned.
//! * Admin: create channels, grant or revoke Moderator, ban and unban users ranked below.
//! * Moderator: delete any message, ban and unban plain users.
//! * User: message, edit and delete own messages, claim a handle.
//!
//! Handle claims are checked with TR39 confusables. The first claim in causal
//! order wins. A user who claims a new handle gives up the old one.
//!
//! Key rotation: a `KeyRotation` signed by the new root makes the new root the
//! same user as the old one. All state is keyed by the canonical root, and
//! events from an old root are rejected once it has rotated away. Device
//! revocation is handled the same way.
//!
//! Each event goes through `check` (read only) and then `commit`. The incremental
//! [`crate::Replayer`] uses the same two steps, so both paths agree.

use crate::emoji::{normalize_emoji, normalize_trait_emoji};
use crate::names::{normalize_handle, skeleton};
use crate::control::{open_event_keyed, EpochKey, OpenError, SealKey};
use crate::{ChannelId, ChannelVisibility, Dag, Event, EventId, HistorySetting, Hlc, Payload, Role, SealedKind};
use dissent_identity::{DeviceId, RootId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityState {
    pub owner: RootId,
    /// Keys are canonical roots. Use [`CommunityState::canonical`] for any root.
    pub roles: BTreeMap<RootId, BTreeSet<Role>>,
    pub banned: BTreeSet<RootId>,
    /// Normalized handle to holder. See [`crate::names::normalize_handle`].
    pub handles: BTreeMap<String, RootId>,
    /// TR39 skeleton of each held handle to its holder. Keeps look-alike claims out.
    pub handle_skeletons: BTreeMap<String, RootId>,
    pub channels: BTreeMap<ChannelId, String>,
    /// Channels made with `VoiceChannelCreate`. Each is also in `channels`.
    pub voice_channels: BTreeSet<ChannelId>,
    pub messages: BTreeMap<ChannelId, Vec<MessageView>>,
    /// Owner is not implicitly a member. The creator emits a Join like everyone else.
    pub members: BTreeMap<RootId, Member>,
    /// Old root to the root that replaced it, applied in causal order.
    pub aliases: BTreeMap<RootId, RootId>,
    /// Revoked device to the HLC of the first accepted revocation.
    pub revoked_devices: BTreeMap<DeviceId, Hlc>,
    /// Revoked device to the wall time of its revocation, as learned before this replay applies any event.
    /// The replayer sets it, so a device's events from the revocation's millisecond on are refused whatever
    /// order they sort in. See `Replayer::apply_all`.
    #[serde(default)]
    pub pre_revoked: BTreeMap<DeviceId, u64>,
    /// Device to the root that signed its events. Learned from every event applied.
    pub device_roots: BTreeMap<DeviceId, RootId>,
    pub rejected: Vec<(EventId, Rejection)>,
    /// Relay servers members use, from the latest accepted `SetRelays`, and its HLC.
    pub relays: Vec<String>,
    pub relays_hlc: Hlc,
    /// Whether members may publish `RelayHostSet`, from the latest accepted `MembersHostRelays`, with its
    /// HLC. Off until an admin turns it on. See `docs/design/12-boosts.md`.
    pub members_host_relays: bool,
    pub members_host_relays_hlc: Hlc,
    /// Whether control events are member-sealed, from the latest accepted `MemberOnlyControl`, and
    /// its HLC. Off until an owner turns it on.
    pub member_only: bool,
    pub member_only_hlc: Hlc,
    /// Which history a new member gets keys for, from the latest accepted `HistoryVisibility`, and
    /// its HLC. `All` until set.
    pub history: HistorySetting,
    pub history_hlc: Hlc,
    /// Generation of the community secret after the latest accepted rotation. Starts at 0.
    pub secret_generation: u64,
    /// Banned roots that some accepted rotation already answers.
    pub secret_covered: BTreeSet<RootId>,
    /// The event that set the current generation. A rival rotation with the same generation and a
    /// lower event id replaces it, so concurrent rotations converge.
    pub secret_winner: Option<EventId>,
    /// Each member's profile picture, from their latest accepted `ProfileUpdate`, with its HLC. A
    /// `None` picture is a removed one. A newer HLC wins.
    pub avatars: BTreeMap<RootId, (Hlc, Option<crate::Picture>)>,
    /// The community icon, from the latest accepted `CommunityIconSet`, with its HLC. A `None`
    /// picture is a removed icon.
    pub icon: Option<(Hlc, Option<crate::Picture>)>,
    /// The animated community icon, from the latest accepted `CommunityIconAnimated`, with its HLC.
    /// It is shown only while it is at least as new as `icon`; see `shown_icon`.
    pub animated_icon: Option<(Hlc, Option<crate::picture::AnimatedIcon>)>,
    /// The community name from the latest accepted `CommunityRename`, with its HLC. `None` keeps
    /// the genesis name.
    pub name: Option<(Hlc, String)>,
    /// The community banner, from the latest accepted `CommunityBannerSet`, with its HLC. A `None`
    /// picture is a removed banner.
    pub banner: Option<(Hlc, Option<crate::picture::Banner>)>,
    /// The description and traits, from the latest accepted `CommunityProfileSet`, with its HLC.
    pub profile: Option<(Hlc, CommunityProfile)>,
    /// Each member's profile banner, from their latest accepted `ProfileBannerSet`, with its HLC. A
    /// `None` banner is a removed one. A newer HLC wins, as for `avatars`.
    pub profile_banners: BTreeMap<RootId, (Hlc, Option<crate::picture::Banner>)>,
    /// The relays each member says they host for this community, from their latest accepted
    /// `RelayHostSet`, with its HLC. Apps verify them before counting a host. See `boosts`.
    pub relay_hosts: BTreeMap<RootId, (Hlc, Vec<String>)>,
    /// Settings from the accepted `ChannelUpdate`s. A channel not listed here has the defaults.
    pub channel_settings: BTreeMap<ChannelId, ChannelSettings>,
    /// Invites by id, from `InviteCreate`.
    pub invites: BTreeMap<[u8; 16], InviteState>,
    /// Wall time of each member's last accepted post in a channel. Slowmode reads it.
    pub last_post: BTreeMap<(ChannelId, RootId), u64>,
    /// Who may read each private channel, from its latest accepted `ChannelAccess`, with its HLC. A channel
    /// without one is public. See `docs/design/11-private-channels.md`.
    pub channel_access: BTreeMap<ChannelId, (Hlc, ChannelAccessSpec)>,
    /// Key generation of each channel that has been private. It starts at 0 and moves on each accepted
    /// change that removes a reader from a private channel.
    pub key_generation: BTreeMap<ChannelId, u64>,
}

/// Who may read a private channel: whether it is private, and the roles and members listed. See
/// `Payload::ChannelAccess`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAccessSpec {
    pub private: bool,
    pub roles: BTreeSet<Role>,
    pub members: BTreeSet<RootId>,
}

/// What a channel's settings say. See `Payload::ChannelUpdate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSettings {
    /// Seconds a member waits between posts. Zero is off.
    pub slowmode_s: u32,
    pub visibility: ChannelVisibility,
    /// Opus target for this channel's calls, in kbps. Voice channels only.
    pub bitrate_kbps: u32,
    /// Most people who can be in this voice channel at once. Zero is no limit.
    pub user_limit: u32,
    /// Relay this channel's calls prefer, or empty for automatic.
    pub relay: String,
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            slowmode_s: 0,
            visibility: ChannelVisibility::Default,
            bitrate_kbps: 64,
            user_limit: 0,
            relay: String::new(),
        }
    }
}

/// The slowmode values a channel may use, in seconds. Matches the UI's list.
pub const SLOWMODE_STEPS: &[u32] = &[0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];
/// Voice bitrate limits, in kbps. A setting above a server's boost ceiling is sent at the ceiling, so
/// replay accepts up to the highest one (`docs/design/12-boosts.md`). 96 was the most before boosts.
pub const BITRATE_RANGE: std::ops::RangeInclusive<u32> = 8..=256;
/// Most people a voice channel's user limit may be set to.
pub const MAX_USER_LIMIT: u32 = 99;
/// Longest community name, in characters. Matches the create dialog's limit.
pub const MAX_COMMUNITY_NAME_CHARS: usize = 64;

/// Whether `name` is a community name as a rename must carry it: already trimmed, not empty, and
/// at most `MAX_COMMUNITY_NAME_CHARS` characters.
pub fn valid_community_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && name.chars().count() <= MAX_COMMUNITY_NAME_CHARS
}

/// Longest community description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 300;
/// Most traits a community shows.
pub const MAX_TRAITS: usize = 5;
/// Longest trait label, in characters.
pub const MAX_TRAIT_LABEL_CHARS: usize = 24;

/// Whether a profile is within its caps: a trimmed description of at most `MAX_DESCRIPTION_CHARS`,
/// and at most `MAX_TRAITS` traits, each with an optional emoji (one grapheme, as `normalize_trait_emoji`
/// keeps it) and a trimmed, non-empty label of at most `MAX_TRAIT_LABEL_CHARS`. Replay rejects a profile over a cap as a
/// whole rather than trimming it, so every peer shows the same profile or none.
pub fn valid_profile(description: &str, traits: &[crate::event::Trait]) -> bool {
    description.trim() == description
        && description.chars().count() <= MAX_DESCRIPTION_CHARS
        && traits.len() <= MAX_TRAITS
        && traits.iter().all(|t| {
            (t.emoji.is_empty() || normalize_trait_emoji(&t.emoji).is_ok_and(|e| e == t.emoji))
                && !t.label.is_empty()
                && t.label.trim() == t.label
                && t.label.chars().count() <= MAX_TRAIT_LABEL_CHARS
        })
}

/// A community's description and traits, from the latest accepted `CommunityProfileSet`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityProfile {
    pub description: String,
    pub traits: Vec<crate::event::Trait>,
}

/// An invite as the replay sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteState {
    pub creator: RootId,
    pub expires_ms: Option<u64>,
    pub revoked: bool,
    /// Accepted joins through this invite.
    pub uses: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub display_name: String,
    /// HLC of the first accepted Join. A rejoin does not move it.
    pub joined: Hlc,
    pub left: bool,
    /// A storage member holds and serves ciphertext, but is outside the MLS group and cannot post.
    pub storage: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    pub id: EventId,
    /// Canonical root of the author.
    pub author: RootId,
    pub hlc: Hlc,
    pub text: String,
    /// True when the text is sealed under MLS. `text` is then empty here; the plaintext lives in
    /// each device's content view. Keeps state identical on every peer.
    pub sealed: bool,
    /// Set by the first accepted Edit.
    pub edited: bool,
    /// Deleted messages keep their slot in the channel with empty text.
    pub deleted: bool,
    /// The message this one replies to. The target may be deleted later.
    pub reply_to: Option<EventId>,
    /// Emoji to the canonical roots that reacted with it. Cleared on delete.
    pub reactions: BTreeMap<String, BTreeSet<RootId>>,
    /// Emoji in the order they first gained a reaction. Keeps views stable across peers.
    pub reaction_order: Vec<String>,
    /// Whether the author could mention everyone when this message was accepted. Set once from the
    /// permission state at that point in the replay, so a later role change does not change it.
    pub mention_all: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    NoPermission,
    Banned,
    UnknownChannel,
    UnknownTarget,
    HandleTaken,
    /// The handle's TR39 skeleton matches a handle someone else holds.
    HandleConfusable,
    NotAuthorOfTarget,
    /// Wrong channel for the payload, or an empty name or handle.
    Malformed,
    /// The author rotated to a new root before this event.
    RotatedAway,
    /// The old root was already rotated by an earlier event.
    AlreadyRotated,
    /// The author's device was revoked before this event.
    DeviceRevoked,
    /// A sealed control event whose generation this replay holds no key for. A device that
    /// fetches the missing key replays it again.
    Locked,
    /// A sealed control event that does not open under the key of its generation.
    Forged,
    /// A member posted again before the channel's slowmode let them.
    Slowmode,
    /// A join through an invite this state has never seen.
    UnknownInvite,
    /// A join through an invite that was revoked.
    InviteRevoked,
    /// A join through an invite whose expiry has passed.
    InviteExpired,
}

impl From<OpenError> for Rejection {
    fn from(why: OpenError) -> Self {
        match why {
            OpenError::NoKey => Rejection::Locked,
            OpenError::Forged => Rejection::Forged,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    User,
    Moderator,
    Admin,
    Owner,
}

/// A message or reply, the posts slowmode limits.
fn is_post(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::Message { .. }
            | Payload::Reply { .. }
            | Payload::Sealed {
                kind: SealedKind::Message | SealedKind::HistoryMessage | SealedKind::Reply { .. },
                ..
            }
            | Payload::PrivateSealed {
                kind: SealedKind::Message | SealedKind::Reply { .. },
                ..
            }
    )
}

/// Replay the DAG's causal order against a fresh state owned by `owner`, with no keys. Sealed
/// control events are rejected as [`Rejection::Locked`]. This is the reference implementation.
/// [`crate::Replayer`] must match it.
pub fn replay(owner: RootId, dag: &Dag) -> CommunityState {
    replay_with(owner, dag, &[])
}

/// Replay the DAG's causal order, opening sealed control events with the keys in `ring`.
pub fn replay_with(owner: RootId, dag: &Dag, ring: &[SealKey]) -> CommunityState {
    replay_keyed(owner, dag, ring, &[])
}

/// Replay the DAG's causal order, opening both kinds of sealed control event. `ring` holds the
/// community secret generations, and `epochs` the encryption key of each MLS epoch.
pub fn replay_keyed(
    owner: RootId,
    dag: &Dag,
    ring: &[SealKey],
    epochs: &[EpochKey],
) -> CommunityState {
    // The same settling passes as `Replayer::apply_all`: each pass replays with the revocations the
    // one before learned in force, until they stop changing. See that function.
    let mut known = BTreeMap::new();
    for pass in 1..=MAX_REVOCATION_PASSES {
        let mut st = CommunityState::new(owner);
        st.pre_revoked = known.clone();
        for ev in dag.causal_order() {
            apply_event(&mut st, ev, ring, epochs);
        }
        let learned = learned_revocations(&st);
        if learned == known || pass == MAX_REVOCATION_PASSES {
            return st;
        }
        known = learned;
    }
    unreachable!("the loop returns on its last pass")
}

/// Replay passes at most this many times to settle the revocations. Two are enough: one learns them, the next
/// confirms. The cap only bounds a pathological set.
pub(crate) const MAX_REVOCATION_PASSES: usize = 4;

/// The wall time of each device's first accepted revocation, in `state`.
pub(crate) fn learned_revocations(state: &CommunityState) -> BTreeMap<DeviceId, u64> {
    state
        .revoked_devices
        .iter()
        .map(|(device, at)| (*device, at.wall_ms))
        .collect()
}

/// Apply one event at its causal position, recording a rejection if it has one.
pub(crate) fn apply_event(
    st: &mut CommunityState,
    ev: &Event,
    ring: &[SealKey],
    epochs: &[EpochKey],
) {
    st.device_roots
        .entry(ev.body.device)
        .or_insert(ev.body.author);
    let plain = match open_event_keyed(ev, ring, epochs) {
        Ok(plain) => plain,
        Err(why) => return st.rejected.push((ev.id, why.into())),
    };
    match st.check(&plain) {
        Ok(()) => st.commit(&plain),
        Err(why) => st.rejected.push((ev.id, why)),
    }
}

impl CommunityState {
    pub fn new(owner: RootId) -> Self {
        Self {
            owner,
            roles: BTreeMap::new(),
            banned: BTreeSet::new(),
            handles: BTreeMap::new(),
            handle_skeletons: BTreeMap::new(),
            channels: BTreeMap::new(),
            voice_channels: BTreeSet::new(),
            messages: BTreeMap::new(),
            members: BTreeMap::new(),
            aliases: BTreeMap::new(),
            revoked_devices: BTreeMap::new(),
            pre_revoked: BTreeMap::new(),
            device_roots: BTreeMap::new(),
            rejected: Vec::new(),
            relays: Vec::new(),
            relays_hlc: Hlc::default(),
            members_host_relays: false,
            members_host_relays_hlc: Hlc::default(),
            member_only: false,
            member_only_hlc: Hlc::default(),
            history: HistorySetting::All,
            history_hlc: Hlc::default(),
            secret_generation: 0,
            secret_covered: BTreeSet::new(),
            secret_winner: None,
            avatars: BTreeMap::new(),
            icon: None,
            animated_icon: None,
            name: None,
            banner: None,
            profile: None,
            profile_banners: BTreeMap::new(),
            relay_hosts: BTreeMap::new(),
            channel_settings: BTreeMap::new(),
            invites: BTreeMap::new(),
            last_post: BTreeMap::new(),
            channel_access: BTreeMap::new(),
            key_generation: BTreeMap::new(),
        }
    }

    /// The settings of a channel, with the defaults where none were set.
    pub fn settings(&self, channel: &ChannelId) -> ChannelSettings {
        self.channel_settings.get(channel).cloned().unwrap_or_default()
    }

    /// The root that now stands for `root`. Follows rotations transitively.
    pub fn canonical(&self, root: &RootId) -> RootId {
        let mut cur = *root;
        while let Some(next) = self.aliases.get(&cur) {
            cur = *next;
        }
        cur
    }

    /// Whether `user` may mention everyone in a community message: the owner and admins by default.
    pub fn may_mention_everyone(&self, user: &RootId) -> bool {
        self.rank(&self.canonical(user)) >= Rank::Admin
    }

    /// Whether `chan` is private, by its latest accepted `ChannelAccess`.
    pub fn is_private(&self, chan: ChannelId) -> bool {
        self.channel_access.get(&chan).is_some_and(|(_, spec)| spec.private)
    }

    /// Whether `user` may read `chan`. A public channel is open to everyone who can see the community.
    /// A private one is open to the owner and admins, to members holding a listed role, and to listed
    /// members who have not left or been banned.
    /// Whether `user` may change who reads a channel: an owner or an admin.
    pub fn may_manage_access(&self, user: RootId) -> bool {
        self.rank(&user) >= Rank::Admin
    }

    pub fn can_read(&self, chan: ChannelId, user: RootId) -> bool {
        let Some((_, spec)) = self.channel_access.get(&chan) else {
            return true;
        };
        if !spec.private || self.rank(&user) >= Rank::Admin {
            return true;
        }
        let has_role = self
            .roles
            .get(&user)
            .is_some_and(|held| spec.roles.iter().any(|role| held.contains(role)));
        has_role
            || (spec.members.contains(&user)
                && !self.banned.contains(&user)
                && self.members.get(&user).is_some_and(|m| !m.left))
    }

    /// Whether a rotation of the community secret may cover `r`: a banned member, or a person who left. A storage
    /// member who left is not covered, because the ban path is the only one that covers storage members.
    fn coverable(&self, r: &RootId) -> bool {
        self.banned.contains(r) || self.members.get(r).is_some_and(|m| m.left && !m.storage)
    }

    /// The members whose departure the secret has not yet been rotated for: banned members, and people who left,
    /// that no rotation covers yet. An admin rotates for all of them at once.
    pub fn rotation_due(&self) -> BTreeSet<RootId> {
        let left = self
            .members
            .iter()
            .filter(|(_, m)| m.left && !m.storage)
            .map(|(root, _)| *root);
        self.banned
            .iter()
            .copied()
            .chain(left)
            .filter(|r| !self.secret_covered.contains(r))
            .collect()
    }

    fn rank(&self, user: &RootId) -> Rank {
        let roles = self.roles.get(user);
        let has = |r| roles.is_some_and(|s| s.contains(&r));
        if *user == self.owner {
            Rank::Owner
        } else if has(Role::Admin) {
            Rank::Admin
        } else if has(Role::Moderator) {
            Rank::Moderator
        } else {
            Rank::User
        }
    }

    /// True if `r` already names a user in this state, in any role.
    fn is_known_root(&self, r: &RootId) -> bool {
        self.owner == *r
            || self.aliases.contains_key(r)
            || self.aliases.values().any(|v| v == r)
            || self.roles.contains_key(r)
            || self.banned.contains(r)
            || self.members.contains_key(r)
            || self.handles.values().any(|h| h == r)
            || self.messages.values().flatten().any(|m| m.author == *r)
    }

    /// Would `ev` be rejected by this state? Reads only, never mutates.
    pub(crate) fn check(&self, ev: &Event) -> Result<(), Rejection> {
        // Events are authored by a raw root. Rotated-away roots are not canonical,
        // so the raw author is the canonical actor whenever the event is accepted.
        let actor = ev.body.author;
        if self.aliases.contains_key(&actor) {
            return Err(Rejection::RotatedAway);
        }
        if self
            .revoked_devices
            .get(&ev.body.device)
            .is_some_and(|revoked_at| ev.body.hlc.wall_ms >= revoked_at.wall_ms)
            || self
                .pre_revoked
                .get(&ev.body.device)
                .is_some_and(|wall| ev.body.hlc.wall_ms >= *wall)
        {
            return Err(Rejection::DeviceRevoked);
        }
        if self.banned.contains(&actor) {
            return Err(Rejection::Banned);
        }
        // A member who left still holds the community secret, so their sealed events decrypt. Until they join
        // again, nothing they send is accepted, whatever their role. Joining is how they become active again.
        let rejoin = matches!(
            ev.body.payload,
            Payload::Join { .. } | Payload::JoinVia { .. } | Payload::JoinAsStorage { .. } | Payload::Leave
        );
        if !rejoin && self.members.get(&actor).is_some_and(|m| m.left) {
            return Err(Rejection::NoPermission);
        }
        let rank = self.rank(&actor);
        // A storage member may only join, or leave. Anything else is for people.
        let storage_only = matches!(
            ev.body.payload,
            Payload::Join { .. } | Payload::JoinAsStorage { .. } | Payload::Leave | Payload::InviteCreate { .. }
        );
        if !storage_only && self.members.get(&actor).is_some_and(|m| m.storage) {
            return Err(Rejection::NoPermission);
        }
        let chan = ev.body.channel;
        if is_post(&ev.body.payload) {
            self.check_slowmode(chan, actor, rank, ev.body.hlc.wall_ms)?;
        }
        match &ev.body.payload {
            // Opened by `apply_event` before it gets here. A sealed payload left is not a control event.
            Payload::SealedControl { .. } | Payload::MemberSealedControl { .. } => {
                return Err(Rejection::Forged)
            }
            Payload::ChannelCreate { name }
            | Payload::VoiceChannelCreate { name }
            | Payload::ChannelCreatePrivate { name, .. } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if name.trim().is_empty() {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ChannelAccess { channel, .. } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !self.channels.contains_key(channel) {
                    return Err(Rejection::UnknownChannel);
                }
            }
            Payload::PrivateSealed { kind, .. } => {
                if !self.channels.contains_key(&chan) {
                    return Err(Rejection::UnknownChannel);
                }
                // Only a private channel takes this form, and only a reader may post in it.
                if !self.is_private(chan) {
                    return Err(Rejection::Malformed);
                }
                if !self.can_read(chan, actor) {
                    return Err(Rejection::NoPermission);
                }
                match kind {
                    // The history form is keyed to the MLS epoch, never to a channel key, so it has no place here.
                    SealedKind::HistoryMessage => return Err(Rejection::Malformed),
                    SealedKind::Message => {}
                    SealedKind::Reply { target }
                    | SealedKind::React { target }
                    | SealedKind::Unreact { target } => {
                        self.live_message(chan, target)?;
                    }
                    SealedKind::Edit { target } => {
                        if self.live_message(chan, target)?.author != actor {
                            return Err(Rejection::NotAuthorOfTarget);
                        }
                    }
                }
            }
            // The message forms below are for public channels only. A private channel's messages take the
            // `PrivateSealed` form, which the community secret and the epoch key cannot open.
            // Plain text forms carry their text in the clear, so they have no place in a private channel either.
            Payload::Sealed { .. } | Payload::Message { .. } | Payload::Reply { .. } | Payload::Edit { .. }
                if self.is_private(chan) =>
            {
                return Err(Rejection::NoPermission);
            }
            Payload::Sealed { kind, .. } => match kind {
                SealedKind::Message | SealedKind::HistoryMessage => {
                    if !self.channels.contains_key(&chan) {
                        return Err(Rejection::UnknownChannel);
                    }
                }
                SealedKind::Reply { target } => {
                    self.live_message(chan, target)?;
                }
                SealedKind::Edit { target } => {
                    if self.live_message(chan, target)?.author != actor {
                        return Err(Rejection::NotAuthorOfTarget);
                    }
                }
                SealedKind::React { target } | SealedKind::Unreact { target } => {
                    self.live_message(chan, target)?;
                }
            },
            Payload::Message { .. } => {
                if !self.channels.contains_key(&chan) {
                    return Err(Rejection::UnknownChannel);
                }
            }
            Payload::Edit { target, .. } => {
                if self.live_message(chan, target)?.author != actor {
                    return Err(Rejection::NotAuthorOfTarget);
                }
            }
            Payload::Delete { target } => {
                let msg = self.live_message(chan, target)?;
                if msg.author != actor && rank < Rank::Moderator {
                    return Err(Rejection::NoPermission);
                }
            }
            // Under member-only control a joiner's Join may go without a name, since that Join is sealed
            // to the community secret storage members hold. The joiner sends its name again once it is in
            // the group. Anywhere else a blank name is refused.
            Payload::Join { display_name } => {
                require_control(chan)?;
                if display_name.trim().is_empty() && !self.member_only {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::JoinVia {
                display_name,
                invite_id,
            } => {
                require_control(chan)?;
                if display_name.trim().is_empty() && !self.member_only {
                    return Err(Rejection::Malformed);
                }
                let invite = self.invites.get(invite_id).ok_or(Rejection::UnknownInvite)?;
                if invite.revoked {
                    return Err(Rejection::InviteRevoked);
                }
                if invite.expires_ms.is_some_and(|at| ev.body.hlc.wall_ms >= at) {
                    return Err(Rejection::InviteExpired);
                }
            }
            Payload::ChannelUpdate {
                channel,
                name,
                slowmode_s,
                visibility: _,
                bitrate_kbps,
                user_limit,
                relay,
            } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !self.channels.contains_key(channel) {
                    return Err(Rejection::UnknownChannel);
                }
                let voice = self.voice_channels.contains(channel);
                if name.as_ref().is_some_and(|n| n.trim().is_empty()) {
                    return Err(Rejection::Malformed);
                }
                // Slowmode is for text, and the voice fields are for voice.
                if slowmode_s.is_some_and(|s| voice || !SLOWMODE_STEPS.contains(&s)) {
                    return Err(Rejection::Malformed);
                }
                if (bitrate_kbps.is_some() || user_limit.is_some() || relay.is_some()) && !voice {
                    return Err(Rejection::Malformed);
                }
                if bitrate_kbps.is_some_and(|b| !BITRATE_RANGE.contains(&b)) {
                    return Err(Rejection::Malformed);
                }
                if user_limit.is_some_and(|u| u > MAX_USER_LIMIT) {
                    return Err(Rejection::Malformed);
                }
                if relay.as_ref().is_some_and(|r| !r.is_empty() && !self.relays.contains(r)) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ChannelDelete { channel } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !self.channels.contains_key(channel) {
                    return Err(Rejection::UnknownChannel);
                }
            }
            Payload::InviteCreate { invite_id, .. } => {
                require_control(chan)?;
                // Any device of an unbanned member may make an invite, as before invite ids existed.
                if self.invites.contains_key(invite_id) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::InviteRevoke { invite_id } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !self.invites.contains_key(invite_id) {
                    return Err(Rejection::UnknownTarget);
                }
            }
            Payload::JoinAsStorage { display_name } => {
                require_control(chan)?;
                if display_name.trim().is_empty() {
                    return Err(Rejection::Malformed);
                }
                // A root that is a person here, active or departed, cannot come back as storage. Storage has no
                // posting, but a storage member is served the secret and the ciphertext, so the switch would
                // hand a former member the community's data. Only a storage root, or a root new to the community,
                // may join as storage. The owner is never storage.
                if actor == self.owner || self.members.get(&actor).is_some_and(|m| !m.storage) {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::Leave => require_control(chan)?,
            Payload::SecretRotation {
                generation,
                covers,
                sealed,
            } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                // The next generation, or a same-generation rival that sorts before the current
                // winner. Concurrent rotations converge on the lowest event id.
                let next = *generation == self.secret_generation + 1
                    && covers.iter().all(|r| self.coverable(r))
                    && covers.iter().any(|r| !self.secret_covered.contains(r));
                let rival = *generation == self.secret_generation
                    && self.secret_generation > 0
                    && self.secret_winner.is_some_and(|w| ev.id < w)
                    && covers.iter().all(|r| self.coverable(r));
                if covers.is_empty() || sealed.is_empty() || !(next || rival) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::SetRelays { relays } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !crate::relay::valid_relays(relays) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::MemberOnlyControl { .. } => {
                require_control(chan)?;
                if rank < Rank::Owner {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::HistoryVisibility { .. } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::RoleGrant { role, .. } | Payload::RoleRevoke { role, .. } => {
                require_control(chan)?;
                check_role_change(rank, *role)?;
            }
            Payload::Ban { user } | Payload::Unban { user } => {
                require_control(chan)?;
                if self.rank(&self.canonical(user)) >= rank {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::HandleClaim { handle } => {
                require_control(chan)?;
                let key = normalize_handle(handle).map_err(|_| Rejection::Malformed)?;
                match self.handles.get(&key) {
                    Some(holder) if *holder != actor => return Err(Rejection::HandleTaken),
                    Some(_) => {}
                    None => {
                        let skel = skeleton(&key);
                        if self
                            .handle_skeletons
                            .get(&skel)
                            .is_some_and(|h| *h != actor)
                        {
                            return Err(Rejection::HandleConfusable);
                        }
                    }
                }
            }
            Payload::Reply { target, .. } => {
                self.live_message(chan, target)?;
            }
            Payload::React { target, emoji } | Payload::Unreact { target, emoji } => {
                normalize_emoji(emoji).map_err(|_| Rejection::Malformed)?;
                self.live_message(chan, target)?;
            }
            Payload::DeviceRevoke(rev) => {
                require_control(chan)?;
                rev.verify().map_err(|_| Rejection::Malformed)?;
                // Any device of the author's root may revoke a sibling device.
                let device_root = self
                    .device_roots
                    .get(&rev.device)
                    .map(|r| self.canonical(r));
                if rev.root != self.canonical(&actor) || device_root != Some(rev.root) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::KeyRotation(rot) => {
                require_control(chan)?;
                rot.verify().map_err(|_| Rejection::Malformed)?;
                let req = &rot.request;
                // Only the new root can announce the rotation.
                if actor != req.new_root {
                    return Err(Rejection::Malformed);
                }
                if self.aliases.contains_key(&req.old_root) {
                    return Err(Rejection::AlreadyRotated);
                }
                if self.is_known_root(&req.new_root) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ProfileUpdate { avatar } => {
                require_control(chan)?;
                if avatar.is_some_and(|p| !p.is_valid()) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::CommunityIconSet { avatar } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if avatar.is_some_and(|p| !p.is_valid()) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::CommunityRename { name } => {
                require_control(chan)?;
                if rank < Rank::Owner {
                    return Err(Rejection::NoPermission);
                }
                if !valid_community_name(name) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::CommunityBannerSet { banner } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if banner.is_some_and(|b| !b.is_valid()) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::CommunityIconAnimated { icon } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if icon.is_some_and(|i| !i.is_valid()) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::CommunityProfileSet { description, traits } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if !valid_profile(description, traits) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::ProfileBannerSet { banner } => {
                require_control(chan)?;
                if banner.is_some_and(|b| !b.is_valid()) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::RelayHostSet { relays } => {
                require_control(chan)?;
                if rank < Rank::Admin && !self.members_host_relays {
                    return Err(Rejection::NoPermission);
                }
                if !crate::relay::valid_relays(relays) {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::MembersHostRelays { .. } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
            }
        }
        Ok(())
    }

    /// Apply an event that `check` accepted against this same state.
    pub(crate) fn commit(&mut self, ev: &Event) {
        let actor = ev.body.author;
        let chan = ev.body.channel;
        let mention_all = self.may_mention_everyone(&actor);
        if is_post(&ev.body.payload) {
            let last = self.last_post.entry((chan, actor)).or_insert(0);
            *last = (*last).max(ev.body.hlc.wall_ms);
        }
        match &ev.body.payload {
            Payload::SealedControl { .. } | Payload::MemberSealedControl { .. } => {}
            Payload::ChannelCreatePrivate { name, roles, members } => {
                // Made private from its first state: the channel and its access are one event.
                let id = ChannelId(ev.id.0);
                self.channels.insert(id, name.clone());
                self.messages.entry(id).or_default();
                let spec = ChannelAccessSpec {
                    private: true,
                    roles: roles.iter().copied().collect(),
                    members: members.iter().copied().collect(),
                };
                self.channel_access.insert(id, (ev.body.hlc, spec));
            }
            Payload::ChannelCreate { name } | Payload::VoiceChannelCreate { name } => {
                let id = ChannelId(ev.id.0);
                self.channels.insert(id, name.clone());
                self.messages.entry(id).or_default();
                if matches!(ev.body.payload, Payload::VoiceChannelCreate { .. }) {
                    self.voice_channels.insert(id);
                }
            }
            Payload::ChannelAccess {
                channel,
                private,
                roles,
                members,
            } => {
                let spec = ChannelAccessSpec {
                    private: *private,
                    roles: roles.iter().copied().collect(),
                    members: members.iter().copied().collect(),
                };
                let newer = self
                    .channel_access
                    .get(channel)
                    .is_none_or(|(hlc, _)| ev.body.hlc > *hlc);
                if newer {
                    // Removing a reader from a private channel starts a new key generation.
                    // Adding a reader under `SinceJoin` does the same, so the newcomer gets only the new generation.
                    if let Some((_, prev)) = self.channel_access.get(channel) {
                        if prev.private && spec.private {
                            let removed = !prev.roles.is_subset(&spec.roles) || !prev.members.is_subset(&spec.members);
                            let added = !spec.roles.is_subset(&prev.roles) || !spec.members.is_subset(&prev.members);
                            if removed || (added && self.history == HistorySetting::SinceJoin) {
                                *self.key_generation.entry(*channel).or_default() += 1;
                            }
                        }
                    }
                    self.channel_access.insert(*channel, (ev.body.hlc, spec));
                }
            }
            Payload::PrivateSealed {
                kind,
                generation,
                ciphertext,
            } => {
                // Replayed as the sealed message it is: the same bookkeeping, with the text left out.
                let mut view = ev.clone();
                view.body.payload = Payload::Sealed {
                    epoch: *generation,
                    kind: *kind,
                    ciphertext: ciphertext.clone(),
                };
                self.commit(&view);
            }
            Payload::Sealed { kind, .. } => match kind {
                SealedKind::Message | SealedKind::HistoryMessage => {
                    self.messages.entry(chan).or_default().push(MessageView {
                        id: ev.id,
                        author: actor,
                        hlc: ev.body.hlc,
                        text: String::new(),
                        sealed: true,
                        edited: false,
                        deleted: false,
                        reply_to: None,
                        reactions: BTreeMap::new(),
                        reaction_order: Vec::new(),
                        mention_all,
                    });
                }
                SealedKind::Reply { target } => {
                    self.messages.entry(chan).or_default().push(MessageView {
                        id: ev.id,
                        author: actor,
                        hlc: ev.body.hlc,
                        text: String::new(),
                        sealed: true,
                        edited: false,
                        deleted: false,
                        reply_to: Some(*target),
                        reactions: BTreeMap::new(),
                        reaction_order: Vec::new(),
                        mention_all,
                    });
                }
                SealedKind::Edit { target } => {
                    if let Some(m) = self.live_message_mut(chan, target) {
                        m.text.clear();
                        m.sealed = true;
                        m.edited = true;
                    }
                }
                // Reactions are aggregated by members, from the sealed emoji. Nothing is stored here.
                SealedKind::React { .. } | SealedKind::Unreact { .. } => {}
            },
            Payload::Message { text } => {
                self.messages.entry(chan).or_default().push(MessageView {
                    id: ev.id,
                    author: actor,
                    hlc: ev.body.hlc,
                    text: text.clone(),
                    sealed: false,
                    edited: false,
                    deleted: false,
                    reply_to: None,
                    reactions: BTreeMap::new(),
                    reaction_order: Vec::new(),
                    mention_all,
                });
            }
            Payload::Edit { target, text } => {
                if let Some(m) = self.live_message_mut(chan, target) {
                    m.text = text.clone();
                    m.sealed = false;
                    m.edited = true;
                }
            }
            Payload::Delete { target } => {
                if let Some(m) = self.live_message_mut(chan, target) {
                    m.deleted = true;
                    m.text.clear();
                    m.reactions.clear();
                    m.reaction_order.clear();
                }
            }
            Payload::Join { display_name } | Payload::JoinVia { display_name, .. } => {
                let m = self.members.entry(actor).or_insert_with(|| Member {
                    display_name: String::new(),
                    joined: ev.body.hlc,
                    left: false,
                    storage: false,
                });
                m.display_name = display_name.clone();
                m.left = false;
                // A rejoined member gets the current secret, so a later departure needs a rotation of its own.
                self.secret_covered.remove(&actor);
                if let Payload::JoinVia { invite_id, .. } = &ev.body.payload {
                    if let Some(invite) = self.invites.get_mut(invite_id) {
                        invite.uses += 1;
                    }
                }
            }
            Payload::ChannelUpdate {
                channel,
                name,
                slowmode_s,
                visibility,
                bitrate_kbps,
                user_limit,
                relay,
            } => {
                if let Some(name) = name {
                    self.channels.insert(*channel, name.clone());
                }
                let settings = self.channel_settings.entry(*channel).or_default();
                if let Some(v) = slowmode_s {
                    settings.slowmode_s = *v;
                }
                if let Some(v) = visibility {
                    settings.visibility = *v;
                }
                if let Some(v) = bitrate_kbps {
                    settings.bitrate_kbps = *v;
                }
                if let Some(v) = user_limit {
                    settings.user_limit = *v;
                }
                if let Some(v) = relay {
                    settings.relay = v.clone();
                }
            }
            Payload::ChannelDelete { channel } => {
                self.channels.remove(channel);
                self.voice_channels.remove(channel);
                self.messages.remove(channel);
                self.channel_settings.remove(channel);
                self.last_post.retain(|(c, _), _| c != channel);
            }
            Payload::InviteCreate {
                invite_id,
                expires_ms,
            } => {
                self.invites.insert(
                    *invite_id,
                    InviteState {
                        creator: actor,
                        expires_ms: *expires_ms,
                        revoked: false,
                        uses: 0,
                    },
                );
            }
            Payload::InviteRevoke { invite_id } => {
                if let Some(invite) = self.invites.get_mut(invite_id) {
                    invite.revoked = true;
                }
            }
            Payload::JoinAsStorage { display_name } => {
                let m = self.members.entry(actor).or_insert_with(|| Member {
                    display_name: String::new(),
                    joined: ev.body.hlc,
                    left: false,
                    storage: true,
                });
                if m.display_name.is_empty() {
                    m.display_name = display_name.clone();
                }
                m.left = false;
                self.secret_covered.remove(&actor);
            }
            Payload::Leave => {
                if let Some(m) = self.members.get_mut(&actor) {
                    m.left = true;
                }
            }
            Payload::SecretRotation {
                generation,
                covers,
                ..
            } => {
                if *generation > self.secret_generation {
                    self.secret_generation = *generation;
                }
                self.secret_winner = Some(ev.id);
                self.secret_covered.extend(covers.iter().copied());
            }
            Payload::SetRelays { relays } => {
                if ev.body.hlc > self.relays_hlc {
                    self.relays = relays.clone();
                    self.relays_hlc = ev.body.hlc;
                }
            }
            Payload::MemberOnlyControl { on } => {
                if ev.body.hlc > self.member_only_hlc {
                    self.member_only = *on;
                    self.member_only_hlc = ev.body.hlc;
                }
            }
            Payload::HistoryVisibility { setting } => {
                if ev.body.hlc > self.history_hlc {
                    self.history = *setting;
                    self.history_hlc = ev.body.hlc;
                }
            }
            Payload::RoleGrant { user, role } => {
                let user = self.canonical(user);
                self.roles.entry(user).or_default().insert(*role);
            }
            Payload::RoleRevoke { user, role } => {
                let user = self.canonical(user);
                if let Some(set) = self.roles.get_mut(&user) {
                    set.remove(role);
                    if set.is_empty() {
                        self.roles.remove(&user);
                    }
                }
            }
            Payload::Ban { user } => {
                let user = self.canonical(user);
                self.banned.insert(user);
            }
            Payload::Unban { user } => {
                let user = self.canonical(user);
                self.banned.remove(&user);
            }
            Payload::HandleClaim { handle } => {
                // `check` has already validated the handle.
                let Ok(key) = normalize_handle(handle) else {
                    return;
                };
                if self.handles.get(&key) != Some(&actor) {
                    self.handles.retain(|_, u| *u != actor);
                    self.handle_skeletons.retain(|_, u| *u != actor);
                    self.handle_skeletons.insert(skeleton(&key), actor);
                    self.handles.insert(key, actor);
                }
            }
            Payload::Reply { target, text } => {
                self.messages.entry(chan).or_default().push(MessageView {
                    id: ev.id,
                    author: actor,
                    hlc: ev.body.hlc,
                    text: text.clone(),
                    sealed: false,
                    edited: false,
                    deleted: false,
                    reply_to: Some(*target),
                    reactions: BTreeMap::new(),
                    reaction_order: Vec::new(),
                    mention_all,
                });
            }
            Payload::React { target, emoji } => {
                let Ok(emoji) = normalize_emoji(emoji) else {
                    return;
                };
                if let Some(m) = self.live_message_mut(chan, target) {
                    if !m.reactions.contains_key(&emoji) {
                        m.reaction_order.push(emoji.clone());
                    }
                    m.reactions.entry(emoji).or_default().insert(actor);
                }
            }
            Payload::Unreact { target, emoji } => {
                let Ok(emoji) = normalize_emoji(emoji) else {
                    return;
                };
                if let Some(m) = self.live_message_mut(chan, target) {
                    if let Some(set) = m.reactions.get_mut(&emoji) {
                        set.remove(&actor);
                        if set.is_empty() {
                            m.reactions.remove(&emoji);
                            m.reaction_order.retain(|e| e != &emoji);
                        }
                    }
                }
            }
            Payload::DeviceRevoke(rev) => {
                self.revoked_devices
                    .entry(rev.device)
                    .or_insert(ev.body.hlc);
            }
            Payload::KeyRotation(rot) => self.migrate(rot.request.old_root, rot.request.new_root),
            Payload::ProfileUpdate { avatar } => {
                if self.avatars.get(&actor).is_none_or(|(hlc, _)| ev.body.hlc > *hlc) {
                    self.avatars.insert(actor, (ev.body.hlc, *avatar));
                }
            }
            Payload::CommunityIconSet { avatar } => {
                if self.icon.is_none_or(|(hlc, _)| ev.body.hlc > hlc) {
                    self.icon = Some((ev.body.hlc, *avatar));
                }
            }
            Payload::CommunityRename { name } => {
                if self.name.as_ref().is_none_or(|(hlc, _)| ev.body.hlc > *hlc) {
                    self.name = Some((ev.body.hlc, name.clone()));
                }
            }
            Payload::CommunityBannerSet { banner } => {
                if self.banner.is_none_or(|(hlc, _)| ev.body.hlc > hlc) {
                    self.banner = Some((ev.body.hlc, *banner));
                }
            }
            Payload::CommunityIconAnimated { icon } => {
                if self.animated_icon.is_none_or(|(hlc, _)| ev.body.hlc > hlc) {
                    self.animated_icon = Some((ev.body.hlc, *icon));
                }
            }
            Payload::CommunityProfileSet { description, traits } => {
                if self.profile.as_ref().is_none_or(|(hlc, _)| ev.body.hlc > *hlc) {
                    let profile = CommunityProfile {
                        description: description.clone(),
                        traits: traits.clone(),
                    };
                    self.profile = Some((ev.body.hlc, profile));
                }
            }
            Payload::ProfileBannerSet { banner } => {
                if self.profile_banners.get(&actor).is_none_or(|(hlc, _)| ev.body.hlc > *hlc) {
                    self.profile_banners.insert(actor, (ev.body.hlc, *banner));
                }
            }
            Payload::RelayHostSet { relays } => {
                if self.relay_hosts.get(&actor).is_none_or(|(hlc, _)| ev.body.hlc > *hlc) {
                    self.relay_hosts.insert(actor, (ev.body.hlc, relays.clone()));
                }
            }
            Payload::MembersHostRelays { on } => {
                if ev.body.hlc > self.members_host_relays_hlc {
                    self.members_host_relays = *on;
                    self.members_host_relays_hlc = ev.body.hlc;
                }
            }
        }
    }

    /// Refuses a post sent before the channel's slowmode lets this author post again. Owners,
    /// admins and moderators are exempt.
    fn check_slowmode(&self, chan: ChannelId, actor: RootId, rank: Rank, wall_ms: u64) -> Result<(), Rejection> {
        let seconds = self.settings(&chan).slowmode_s;
        if seconds == 0 || rank >= Rank::Moderator {
            return Ok(());
        }
        match self.last_post.get(&(chan, actor)) {
            Some(last) if wall_ms < last.saturating_add(u64::from(seconds) * 1000) => Err(Rejection::Slowmode),
            _ => Ok(()),
        }
    }

    /// Make `new` the same user as `old` from here on. Moves every stored reference.
    fn migrate(&mut self, old: RootId, new: RootId) {
        self.aliases.insert(old, new);
        if let Some(roles) = self.roles.remove(&old) {
            self.roles.insert(new, roles);
        }
        if self.banned.remove(&old) {
            self.banned.insert(new);
        }
        if let Some(member) = self.members.remove(&old) {
            self.members.insert(new, member);
        }
        for holder in self
            .handles
            .values_mut()
            .chain(self.handle_skeletons.values_mut())
        {
            if *holder == old {
                *holder = new;
            }
        }
        if self.owner == old {
            self.owner = new;
        }
        for m in self.messages.values_mut().flatten() {
            if m.author == old {
                m.author = new;
            }
            for reactors in m.reactions.values_mut() {
                if reactors.remove(&old) {
                    reactors.insert(new);
                }
            }
        }
    }

    fn live_message(&self, chan: ChannelId, target: &EventId) -> Result<&MessageView, Rejection> {
        self.messages
            .get(&chan)
            .and_then(|msgs| msgs.iter().find(|m| m.id == *target && !m.deleted))
            .ok_or(Rejection::UnknownTarget)
    }

    fn live_message_mut(&mut self, chan: ChannelId, target: &EventId) -> Option<&mut MessageView> {
        self.messages
            .get_mut(&chan)?
            .iter_mut()
            .find(|m| m.id == *target && !m.deleted)
    }
}

fn require_control(chan: ChannelId) -> Result<(), Rejection> {
    if chan == ChannelId::CONTROL {
        Ok(())
    } else {
        Err(Rejection::Malformed)
    }
}

fn check_role_change(actor: Rank, role: Role) -> Result<(), Rejection> {
    let ok = match role {
        Role::Admin => actor == Rank::Owner,
        Role::Moderator => actor >= Rank::Admin,
    };
    if ok {
        Ok(())
    } else {
        Err(Rejection::NoPermission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{on_channel, User};
    use crate::Hlc;

    /// Append `payload` as a causal child of the previous event, stamped by position.
    fn chain(evs: &mut Vec<Event>, u: &User, chan: ChannelId, payload: Payload) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let hlc = Hlc {
            wall_ms: evs.len() as u64,
            counter: 0,
        };
        let ev = on_channel(u, chan, parents, hlc, payload);
        let id = ev.id;
        evs.push(ev);
        id
    }

    #[test]
    fn roles_bans_handles_and_deletes() {
        let (owner, admin, moderator, user) = (User::new(), User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();

        let general = ChannelId(
            chain(
                &mut evs,
                &owner,
                ctl,
                Payload::ChannelCreate {
                    name: "general".into(),
                },
            )
            .0,
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: moderator.id(),
                role: Role::Moderator,
            },
        );
        let no_perm = chain(
            &mut evs,
            &user,
            ctl,
            Payload::ChannelCreate { name: "x".into() },
        );
        let mod_bans_admin = chain(&mut evs, &moderator, ctl, Payload::Ban { user: admin.id() });
        let admin_grants_admin = chain(
            &mut evs,
            &admin,
            ctl,
            Payload::RoleGrant {
                user: user.id(),
                role: Role::Admin,
            },
        );
        chain(&mut evs, &moderator, ctl, Payload::Ban { user: user.id() });
        let late = chain(
            &mut evs,
            &user,
            general,
            Payload::Message { text: "hi".into() },
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::HandleClaim {
                handle: " Alice ".into(),
            },
        );
        let taken = chain(
            &mut evs,
            &admin,
            ctl,
            Payload::HandleClaim {
                handle: "ALICE".into(),
            },
        );
        let m = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message {
                text: "hello".into(),
            },
        );
        chain(&mut evs, &admin, general, Payload::Delete { target: m });
        let m2 = chain(
            &mut evs,
            &admin,
            general,
            Payload::Message { text: "bye".into() },
        );
        let owner_edits_admin = chain(
            &mut evs,
            &owner,
            general,
            Payload::Edit {
                target: m2,
                text: "x".into(),
            },
        );
        chain(
            &mut evs,
            &moderator,
            general,
            Payload::Delete { target: m2 },
        );
        let edit_deleted = chain(
            &mut evs,
            &admin,
            general,
            Payload::Edit {
                target: m,
                text: "x".into(),
            },
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::Ban {
                user: moderator.id(),
            },
        );
        let mod_unbans = chain(
            &mut evs,
            &moderator,
            ctl,
            Payload::Unban { user: user.id() },
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::HandleClaim {
                handle: "bob".into(),
            },
        );
        let carol = chain(
            &mut evs,
            &user,
            ctl,
            Payload::HandleClaim {
                handle: "carol".into(),
            },
        );

        // Insert in reverse so the whole chain is released from pending.
        let mut dag = Dag::new();
        for ev in evs.iter().rev() {
            dag.insert(ev.clone()).unwrap();
        }
        assert!(dag.missing().is_empty());
        assert_eq!(dag.len(), evs.len());

        let st = replay(owner.id(), &dag);
        for (id, why) in [
            (no_perm, Rejection::NoPermission),
            (mod_bans_admin, Rejection::NoPermission),
            (admin_grants_admin, Rejection::NoPermission),
            (late, Rejection::Banned),
            (taken, Rejection::HandleTaken),
            (owner_edits_admin, Rejection::NotAuthorOfTarget),
            (edit_deleted, Rejection::UnknownTarget),
            (mod_unbans, Rejection::Banned),
            (carol, Rejection::Banned),
        ] {
            assert!(
                st.rejected.contains(&(id, why)),
                "missing {why:?} for {id:?}"
            );
        }
        assert_eq!(st.rejected.len(), 9);

        // Owner's later "bob" claim gives up "alice".
        assert_eq!(st.handles.get("alice"), None);
        assert_eq!(st.handles.get("bob"), Some(&owner.id()));
        assert_eq!(st.handles.len(), 1);
        assert_eq!(st.banned, BTreeSet::from([user.id(), moderator.id()]));
        assert_eq!(
            st.roles.get(&admin.id()),
            Some(&BTreeSet::from([Role::Admin]))
        );
        assert!(!st.roles.contains_key(&user.id()));

        let msgs = &st.messages[&general];
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().all(|m| m.deleted && m.text.is_empty()));
    }

    #[test]
    fn membership_join_leave_rejoin_and_edited_flag() {
        let (owner, user) = (User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        let general = ChannelId(
            chain(
                &mut evs,
                &owner,
                ctl,
                Payload::ChannelCreate { name: "g".into() },
            )
            .0,
        );
        let join1 = chain(
            &mut evs,
            &user,
            ctl,
            Payload::Join {
                display_name: "Ann".into(),
            },
        );
        let msg = chain(
            &mut evs,
            &user,
            general,
            Payload::Message { text: "a".into() },
        );
        chain(
            &mut evs,
            &user,
            general,
            Payload::Edit {
                target: msg,
                text: "b".into(),
            },
        );
        chain(&mut evs, &user, ctl, Payload::Leave);
        let join2 = chain(
            &mut evs,
            &user,
            ctl,
            Payload::Join {
                display_name: "Ann B".into(),
            },
        );
        let blank = chain(
            &mut evs,
            &user,
            ctl,
            Payload::Join {
                display_name: "  ".into(),
            },
        );
        let owner_leave = chain(&mut evs, &owner, general, Payload::Leave);

        let mut dag = Dag::new();
        for ev in &evs {
            dag.insert(ev.clone()).unwrap();
        }
        let st = replay(owner.id(), &dag);

        let m = &st.members[&user.id()];
        assert_eq!(m.display_name, "Ann B");
        assert!(!m.left, "rejoin clears left");
        assert_eq!(
            m.joined, evs[1].body.hlc,
            "joined keeps the first Join's stamp"
        );
        assert!(
            !st.members.contains_key(&owner.id()),
            "owner is not implicit"
        );
        assert_eq!(st.members.len(), 1);
        assert_eq!(
            st.rejected,
            vec![
                (blank, Rejection::Malformed),
                (owner_leave, Rejection::Malformed)
            ]
        );
        assert!(st
            .rejected
            .iter()
            .all(|(id, _)| *id != join1 && *id != join2));
        let msgs = &st.messages[&general];
        assert!(msgs[0].edited);
        assert_eq!(msgs[0].text, "b");
    }

    /// A member who left still holds the community secret, so their sealed events decrypt. Their events after the
    /// Leave must be refused, including admin actions, until they join again.
    #[test]
    fn a_member_who_left_cannot_publish_until_they_rejoin() {
        let (owner, admin, rival) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        let general = ChannelId(
            chain(&mut evs, &owner, ctl, Payload::ChannelCreate { name: "g".into() }).0,
        );
        chain(&mut evs, &admin, ctl, Payload::Join { display_name: "Ada".into() });
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        chain(&mut evs, &admin, ctl, Payload::Leave);
        let grant_after_leave = chain(
            &mut evs,
            &admin,
            ctl,
            Payload::RoleGrant {
                user: rival.id(),
                role: Role::Moderator,
            },
        );
        let message_after_leave = chain(
            &mut evs,
            &admin,
            general,
            Payload::Message { text: "still here".into() },
        );
        let mut dag = Dag::new();
        for ev in &evs {
            dag.insert(ev.clone()).unwrap();
        }
        let st = replay(owner.id(), &dag);
        assert!(st.rejected.iter().any(|(id, _)| *id == grant_after_leave), "a grant after leaving is refused");
        assert!(st.rejected.iter().any(|(id, _)| *id == message_after_leave), "a message after leaving is refused");
        assert!(!st.roles.get(&rival.id()).is_some_and(|r| r.contains(&Role::Moderator)));
        assert!(st.messages.get(&general).is_none_or(|m| m.is_empty()));

        // Joining again is what makes the member active. Their next event is accepted.
        chain(&mut evs, &admin, ctl, Payload::Join { display_name: "Ada".into() });
        let back = chain(
            &mut evs,
            &admin,
            general,
            Payload::Message { text: "back".into() },
        );
        let mut dag = Dag::new();
        for ev in &evs {
            dag.insert(ev.clone()).unwrap();
        }
        let st = replay(owner.id(), &dag);
        assert!(!st.rejected.iter().any(|(id, _)| *id == back), "a post after rejoining is accepted");
        assert_eq!(st.messages[&general].len(), 1);
    }

    /// A person, active or departed, cannot come back as storage. A storage member can rejoin as storage, and a
    /// root new to the community can join as storage.
    #[test]
    fn a_person_cannot_join_as_storage_but_a_storage_member_can_rejoin() {
        let (owner, person, seeder, newcomer) = (User::new(), User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(&mut evs, &person, ctl, Payload::Join { display_name: "Pat".into() });
        let active = chain(&mut evs, &person, ctl, Payload::JoinAsStorage { display_name: "Pat".into() });
        chain(&mut evs, &person, ctl, Payload::Leave);
        let departed = chain(&mut evs, &person, ctl, Payload::JoinAsStorage { display_name: "Pat".into() });
        let owner_storage = chain(&mut evs, &owner, ctl, Payload::JoinAsStorage { display_name: "Own".into() });
        chain(&mut evs, &seeder, ctl, Payload::JoinAsStorage { display_name: "Seed".into() });
        chain(&mut evs, &seeder, ctl, Payload::Leave);
        let seeder_back = chain(&mut evs, &seeder, ctl, Payload::JoinAsStorage { display_name: "Seed".into() });
        let fresh = chain(&mut evs, &newcomer, ctl, Payload::JoinAsStorage { display_name: "New".into() });

        let mut dag = Dag::new();
        for ev in &evs {
            dag.insert(ev.clone()).unwrap();
        }
        let st = replay(owner.id(), &dag);
        for refused in [active, departed, owner_storage] {
            assert!(st.rejected.iter().any(|(id, _)| *id == refused), "a person may not join as storage");
        }
        for accepted in [seeder_back, fresh] {
            assert!(!st.rejected.iter().any(|(id, _)| *id == accepted), "storage joins by storage roots and new roots are kept");
        }
        assert!(!st.members[&person.id()].storage, "the person stays a full member");
        assert!(st.members[&seeder.id()].storage);
        assert!(st.members[&newcomer.id()].storage);
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    use crate::testutil::{on_channel, User};
    use crate::Hlc;

    fn claim(evs: &mut Vec<Event>, u: &User, handle: &str) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let hlc = Hlc {
            wall_ms: evs.len() as u64,
            counter: 0,
        };
        let ev = on_channel(
            u,
            ChannelId::CONTROL,
            parents,
            hlc,
            Payload::HandleClaim {
                handle: handle.into(),
            },
        );
        let id = ev.id;
        evs.push(ev);
        id
    }

    #[test]
    fn look_alike_and_invalid_handles_are_refused() {
        let (owner, user, admin) = (User::new(), User::new(), User::new());
        let mut evs = Vec::new();
        claim(&mut evs, &owner, "paypal");
        let mixed = claim(&mut evs, &user, "p\u{0430}ypal");
        let bidi = claim(&mut evs, &user, "ali\u{202E}ce");
        let copy = claim(&mut evs, &owner, "copy");
        let cyr = claim(&mut evs, &admin, "\u{0441}\u{043E}\u{0440}\u{0443}");
        let fullwidth = claim(&mut evs, &admin, "ＣＯＰＹ");
        // The holder can re-claim a look-alike of its own handle.
        let own = claim(&mut evs, &owner, "\u{0441}\u{043E}\u{0440}\u{0443}");

        let mut dag = Dag::new();
        for ev in &evs {
            dag.insert(ev.clone()).unwrap();
        }
        let st = replay(owner.id(), &dag);

        assert!(st.rejected.contains(&(mixed, Rejection::Malformed)));
        assert!(st.rejected.contains(&(bidi, Rejection::Malformed)));
        assert!(st.rejected.contains(&(cyr, Rejection::HandleConfusable)));
        assert!(st.rejected.contains(&(fullwidth, Rejection::HandleTaken)));
        assert!(!st.rejected.iter().any(|(id, _)| *id == copy || *id == own));
        assert_eq!(st.rejected.len(), 4);

        // Owner gave up "paypal" for "copy" and then the Cyrillic form. Only one handle remains.
        assert_eq!(st.handles.len(), 1);
        assert_eq!(st.handles.get("copy"), None);
        assert_eq!(st.handles.get("сору"), Some(&owner.id()));
        assert_eq!(st.handle_skeletons.get("copy"), Some(&owner.id()));
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;
    use crate::testutil::{on_channel, User};
    use crate::Hlc;
    use dissent_identity::KeyRotation;

    /// Append a child of the last event on the control channel or `chan`.
    fn chain(evs: &mut Vec<Event>, u: &User, chan: ChannelId, payload: Payload) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let hlc = Hlc {
            wall_ms: evs.len() as u64,
            counter: 0,
        };
        let ev = on_channel(u, chan, parents, hlc, payload);
        let id = ev.id;
        evs.push(ev);
        id
    }

    fn replay_all(owner: &User, evs: &[Event]) -> CommunityState {
        let mut dag = Dag::new();
        for ev in evs {
            dag.insert(ev.clone()).unwrap();
        }
        replay(owner.id(), &dag)
    }

    fn rotate(old: &User, new: &User, now: u64) -> Payload {
        Payload::KeyRotation(KeyRotation::by_owner(&old.root, &new.root, now))
    }

    #[test]
    fn rotation_moves_roles_handles_and_moderation() {
        let (owner, user, victim, newer) = (User::new(), User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        let general = ChannelId(
            chain(
                &mut evs,
                &owner,
                ctl,
                Payload::ChannelCreate { name: "g".into() },
            )
            .0,
        );
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: user.id(),
                role: Role::Moderator,
            },
        );
        chain(
            &mut evs,
            &user,
            ctl,
            Payload::HandleClaim {
                handle: "neo".into(),
            },
        );
        let v_msg = chain(
            &mut evs,
            &victim,
            general,
            Payload::Message {
                text: "spam".into(),
            },
        );
        chain(&mut evs, &newer, ctl, rotate(&user, &newer, 10));
        let del = chain(&mut evs, &newer, general, Payload::Delete { target: v_msg });
        let old_msg = chain(
            &mut evs,
            &user,
            general,
            Payload::Message {
                text: "late".into(),
            },
        );

        let st = replay_all(&owner, &evs);
        assert_eq!(st.canonical(&user.id()), newer.id());
        assert_eq!(st.aliases.get(&user.id()), Some(&newer.id()));
        assert!(st.roles[&newer.id()].contains(&Role::Moderator));
        assert!(!st.roles.contains_key(&user.id()));
        assert_eq!(st.handles.get("neo"), Some(&newer.id()));
        assert_eq!(st.handle_skeletons.get("neo"), Some(&newer.id()));
        assert!(
            !st.rejected.iter().any(|(id, _)| *id == del),
            "new root moderates"
        );
        assert!(st.rejected.contains(&(old_msg, Rejection::RotatedAway)));
        assert_eq!(st.rejected.len(), 1);
        assert!(st.messages[&general].iter().any(|m| m.deleted));
    }

    #[test]
    fn owner_rotation_moves_ownership() {
        let (owner, successor, user) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::ChannelCreate { name: "g".into() },
        );
        chain(&mut evs, &successor, ctl, rotate(&owner, &successor, 5));
        let old_owner_grant = chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: user.id(),
                role: Role::Admin,
            },
        );
        let grant = chain(
            &mut evs,
            &successor,
            ctl,
            Payload::RoleGrant {
                user: user.id(),
                role: Role::Admin,
            },
        );

        let st = replay_all(&owner, &evs);
        assert_eq!(st.owner, successor.id());
        assert!(st
            .rejected
            .contains(&(old_owner_grant, Rejection::RotatedAway)));
        assert!(!st.rejected.iter().any(|(id, _)| *id == grant));
        assert!(st.roles[&user.id()].contains(&Role::Admin));
    }

    #[test]
    fn rotation_chain_resolves_transitively() {
        let (a, b, c) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(
            &mut evs,
            &a,
            ctl,
            Payload::ChannelCreate { name: "g".into() },
        );
        chain(
            &mut evs,
            &a,
            ctl,
            Payload::RoleGrant {
                user: a.id(),
                role: Role::Moderator,
            },
        );
        chain(&mut evs, &b, ctl, rotate(&a, &b, 2));
        chain(&mut evs, &c, ctl, rotate(&b, &c, 3));

        let st = replay_all(&a, &evs);
        assert!(st.rejected.is_empty(), "{:?}", st.rejected);
        assert_eq!(st.aliases.len(), 2);
        assert_eq!(st.canonical(&a.id()), c.id());
        assert_eq!(st.canonical(&b.id()), c.id());
        assert_eq!(st.canonical(&c.id()), c.id());
        assert!(st.roles[&c.id()].contains(&Role::Moderator));
        assert_eq!(st.owner, c.id());
    }

    #[test]
    fn competing_rotations_first_in_causal_order_wins() {
        let (a, b1, b2) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        let g = chain(
            &mut evs,
            &a,
            ctl,
            Payload::ChannelCreate { name: "g".into() },
        );
        let parents = vec![g];
        let first = on_channel(
            &b1,
            ctl,
            parents.clone(),
            Hlc {
                wall_ms: 2,
                counter: 0,
            },
            rotate(&a, &b1, 2),
        );
        let second = on_channel(
            &b2,
            ctl,
            parents,
            Hlc {
                wall_ms: 3,
                counter: 0,
            },
            rotate(&a, &b2, 3),
        );
        let second_id = second.id;
        evs.push(second);
        evs.push(first);

        let st = replay_all(&a, &evs);
        assert_eq!(st.canonical(&a.id()), b1.id());
        assert!(st
            .rejected
            .contains(&(second_id, Rejection::AlreadyRotated)));
        assert_eq!(st.rejected.len(), 1);
    }

    #[test]
    fn invalid_rotation_proofs_are_malformed() {
        let (a, b, c) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(
            &mut evs,
            &a,
            ctl,
            Payload::ChannelCreate { name: "g".into() },
        );

        let mut forged = KeyRotation::by_owner(&a.root, &b.root, 4);
        forged.new_root_sig.0[0] ^= 1;
        let bad_sig = chain(&mut evs, &b, ctl, Payload::KeyRotation(forged));

        // Valid proof, but authored by the old root rather than the new one.
        let wrong_author = chain(&mut evs, &a, ctl, rotate(&a, &c, 5));

        let st = replay_all(&a, &evs);
        assert!(st.rejected.contains(&(bad_sig, Rejection::Malformed)));
        assert!(st.rejected.contains(&(wrong_author, Rejection::Malformed)));
        assert!(st.aliases.is_empty());
    }

    /// A rotation may cover a person who left, and a rejoin makes the next departure need a rotation of its own.
    /// A storage member who left is not covered.
    #[test]
    fn a_departure_is_covered_by_one_rotation_and_a_rejoin_is_covered_again() {
        let (owner, admin, leaver, storage) = (User::new(), User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(&mut evs, &admin, ctl, Payload::Join { display_name: "Ada".into() });
        chain(&mut evs, &owner, ctl, Payload::RoleGrant { user: admin.id(), role: Role::Admin });
        chain(&mut evs, &leaver, ctl, Payload::Join { display_name: "Lee".into() });
        chain(&mut evs, &storage, ctl, Payload::JoinAsStorage { display_name: "Store".into() });
        chain(&mut evs, &leaver, ctl, Payload::Leave);
        chain(&mut evs, &storage, ctl, Payload::Leave);
        let rotation = |generation: u64, covers: Vec<RootId>| Payload::SecretRotation {
            generation,
            covers,
            sealed: vec![1, 2, 3],
        };
        let ok = chain(&mut evs, &admin, ctl, rotation(1, vec![leaver.id()]));
        let st = replay_all(&owner, &evs);
        assert_eq!(st.secret_generation, 1, "one rotation covers the leaver");
        assert!(!st.rejected.iter().any(|(id, _)| *id == ok));
        assert!(st.rotation_due().is_empty(), "nothing is due after it");

        let refused = chain(&mut evs, &admin, ctl, rotation(2, vec![storage.id()]));
        let st = replay_all(&owner, &evs);
        assert!(st.rejected.iter().any(|(id, _)| *id == refused), "a storage member's leave is not covered");

        chain(&mut evs, &leaver, ctl, Payload::Join { display_name: "Lee".into() });
        chain(&mut evs, &leaver, ctl, Payload::Leave);
        let st = replay_all(&owner, &evs);
        assert_eq!(st.rotation_due(), BTreeSet::from([leaver.id()]), "the next departure is due again");
    }
}

#[cfg(test)]
mod revocation_tests {
    use super::*;
    use crate::testutil::{on_channel, User};
    use crate::{EventBody, Hlc};
    use dissent_identity::{CommunityId, DeviceKey};

    /// Event from a specific device of `root`. Used for multi-device users.
    fn on_device(
        root: &User,
        device: &DeviceKey,
        chan: ChannelId,
        parents: Vec<EventId>,
        wall: u64,
        payload: Payload,
    ) -> Event {
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel: chan,
            author: root.id(),
            device: device.id(),
            parents,
            hlc: Hlc {
                wall_ms: wall,
                counter: 0,
            },
            payload,
        };
        Event::sign(body, device)
    }

    fn replay_all(owner: &User, evs: &[Event]) -> CommunityState {
        let mut dag = Dag::new();
        for ev in evs {
            dag.insert(ev.clone()).unwrap();
        }
        replay(owner.id(), &dag)
    }

    #[test]
    fn a_post_in_the_revocations_millisecond_is_refused() {
        let (owner, user) = (User::new(), User::new());
        let d1 = user.device.id();
        let d2 = DeviceKey::generate();
        let ctl = ChannelId::CONTROL;
        let g = on_channel(
            &owner,
            ctl,
            vec![],
            Hlc {
                wall_ms: 1,
                counter: 0,
            },
            Payload::ChannelCreate { name: "g".into() },
        );
        let chan = ChannelId(g.id.0);
        // The revocation is stamped at millisecond 5. The laptop has not seen it, so its post is concurrent with
        // it, and in the same millisecond. Take a post whose id sorts first, the order that used to accept it.
        let rev = on_device(
            &user,
            &d2,
            ctl,
            vec![g.id],
            5,
            Payload::DeviceRevoke(user.root.revoke(d1, 5)),
        );
        let same_ms = (0..10_000)
            .map(|i| {
                on_device(
                    &user,
                    &user.device,
                    chan,
                    vec![g.id],
                    5,
                    Payload::Message {
                        text: format!("same millisecond {i}"),
                    },
                )
            })
            .find(|m| m.id < rev.id)
            .expect("a post that sorts before the revocation");
        let earlier = on_device(
            &user,
            &user.device,
            chan,
            vec![g.id],
            4,
            Payload::Message {
                text: "earlier".into(),
            },
        );
        let (same_id, earlier_id) = (same_ms.id, earlier.id);
        let st = replay_all(&owner, &[g, rev, same_ms, earlier]);
        assert!(
            st.rejected.contains(&(same_id, Rejection::DeviceRevoked)),
            "a post in the revocation's millisecond is refused, whatever order it sorts in"
        );
        assert!(!st.rejected.iter().any(|(id, _)| *id == earlier_id), "a post before the revocation is kept");
    }

    #[test]
    fn revoked_device_is_cut_off_and_sibling_still_posts() {
        let (owner, user, other) = (User::new(), User::new(), User::new());
        let d1 = user.device.id();
        let d2 = DeviceKey::generate();
        let ctl = ChannelId::CONTROL;
        let g = on_channel(
            &owner,
            ctl,
            vec![],
            Hlc {
                wall_ms: 1,
                counter: 0,
            },
            Payload::ChannelCreate { name: "g".into() },
        );
        let chan = ChannelId(g.id.0);
        let mut evs = vec![g.clone()];

        // Device 1 posts before the revoke sorts first, so it is accepted.
        let before = on_device(
            &user,
            &user.device,
            chan,
            vec![g.id],
            3,
            Payload::Message {
                text: "before".into(),
            },
        );
        // Device 2 revokes device 1.
        let rev = on_device(
            &user,
            &d2,
            ctl,
            vec![g.id],
            4,
            Payload::DeviceRevoke(user.root.revoke(d1, 4)),
        );
        let rev_id = rev.id;
        let after = on_device(
            &user,
            &user.device,
            chan,
            vec![rev_id],
            6,
            Payload::Message {
                text: "after".into(),
            },
        );
        let after_id = after.id;
        let sibling = on_device(
            &user,
            &d2,
            chan,
            vec![rev_id],
            7,
            Payload::Message {
                text: "sibling".into(),
            },
        );
        // Another root cannot revoke this user's device.
        let forged = on_device(
            &other,
            &other.device,
            ctl,
            vec![g.id],
            5,
            Payload::DeviceRevoke(other.root.revoke(d1, 5)),
        );
        let forged_id = forged.id;
        evs.extend([before, rev, after, sibling, forged]);

        let st = replay_all(&owner, &evs);
        assert_eq!(
            st.revoked_devices.get(&d1),
            Some(&Hlc {
                wall_ms: 4,
                counter: 0
            })
        );
        assert!(st.rejected.contains(&(after_id, Rejection::DeviceRevoked)));
        assert!(st.rejected.contains(&(forged_id, Rejection::Malformed)));
        assert_eq!(st.rejected.len(), 2);
        assert_eq!(st.messages[&chan].len(), 2, "before and sibling are kept");
        assert_eq!(st.canonical(&user.id()), user.id());
    }

    #[test]
    fn revocation_of_unknown_device_is_malformed() {
        let (owner, user) = (User::new(), User::new());
        let stranger = DeviceKey::generate();
        let ctl = ChannelId::CONTROL;
        let g = on_channel(
            &owner,
            ctl,
            vec![],
            Hlc {
                wall_ms: 1,
                counter: 0,
            },
            Payload::ChannelCreate { name: "g".into() },
        );
        let rev = on_device(
            &user,
            &user.device,
            ctl,
            vec![g.id],
            2,
            Payload::DeviceRevoke(user.root.revoke(stranger.id(), 2)),
        );
        let rev_id = rev.id;
        let st = replay_all(&owner, &[g, rev]);
        assert!(st.rejected.contains(&(rev_id, Rejection::Malformed)));
    }
}

#[cfg(test)]
mod reply_reaction_tests {
    use super::*;
    use crate::testutil::{on_channel, User};
    use crate::Hlc;
    use dissent_identity::KeyRotation;

    /// Append a causal child of the last event, on `chan`.
    fn chain(evs: &mut Vec<Event>, u: &User, chan: ChannelId, payload: Payload) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let hlc = Hlc {
            wall_ms: evs.len() as u64,
            counter: 0,
        };
        let ev = on_channel(u, chan, parents, hlc, payload);
        let id = ev.id;
        evs.push(ev);
        id
    }

    fn replay_all(owner: &User, evs: &[Event]) -> CommunityState {
        let mut dag = Dag::new();
        for ev in evs {
            dag.insert(ev.clone()).unwrap();
        }
        replay(owner.id(), &dag)
    }

    #[test]
    fn replies_point_at_live_messages_in_the_same_channel() {
        let (owner, user) = (User::new(), User::new());
        let mut evs = Vec::new();
        let g = chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::ChannelCreate { name: "g".into() },
        );
        let other = chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::ChannelCreate { name: "h".into() },
        );
        let (general, side) = (ChannelId(g.0), ChannelId(other.0));
        let m = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message { text: "hi".into() },
        );
        let ok = chain(
            &mut evs,
            &user,
            general,
            Payload::Reply {
                target: m,
                text: "yo".into(),
            },
        );
        let wrong_chan = chain(
            &mut evs,
            &user,
            side,
            Payload::Reply {
                target: m,
                text: "x".into(),
            },
        );
        let on_control = chain(
            &mut evs,
            &user,
            ChannelId::CONTROL,
            Payload::Reply {
                target: m,
                text: "x".into(),
            },
        );
        let dead = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message {
                text: "gone".into(),
            },
        );
        chain(&mut evs, &owner, general, Payload::Delete { target: dead });
        let late = chain(
            &mut evs,
            &user,
            general,
            Payload::Reply {
                target: dead,
                text: "late".into(),
            },
        );
        // The reply stays when its target is deleted afterwards.
        chain(&mut evs, &owner, general, Payload::Delete { target: m });

        let st = replay_all(&owner, &evs);
        assert!(st
            .rejected
            .contains(&(wrong_chan, Rejection::UnknownTarget)));
        assert!(st
            .rejected
            .contains(&(on_control, Rejection::UnknownTarget)));
        assert!(st.rejected.contains(&(late, Rejection::UnknownTarget)));
        assert_eq!(st.rejected.len(), 3);
        let msgs = &st.messages[&general];
        let reply = msgs.iter().find(|x| x.id == ok).unwrap();
        assert_eq!(reply.reply_to, Some(m));
        assert_eq!(reply.text, "yo");
        assert!(!reply.deleted);
        let original = msgs.iter().find(|x| x.id == m).unwrap();
        assert!(original.deleted && original.text.is_empty());
    }

    #[test]
    fn reactions_are_idempotent_and_unreact_is_a_noop_without_reaction() {
        let (owner, user, other) = (User::new(), User::new(), User::new());
        let mut evs = Vec::new();
        let g = chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::ChannelCreate { name: "g".into() },
        );
        let general = ChannelId(g.0);
        let m = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message { text: "hi".into() },
        );
        let like = || Payload::React {
            target: m,
            emoji: "👍".into(),
        };
        chain(&mut evs, &user, general, like());
        chain(&mut evs, &user, general, like());
        chain(
            &mut evs,
            &other,
            general,
            Payload::Unreact {
                target: m,
                emoji: "👍".into(),
            },
        );
        chain(
            &mut evs,
            &owner,
            general,
            Payload::React {
                target: m,
                emoji: "🎉".into(),
            },
        );
        chain(
            &mut evs,
            &owner,
            general,
            Payload::Unreact {
                target: m,
                emoji: "🎉".into(),
            },
        );
        // Emoji set stays on the message, with one user per emoji.
        let st = replay_all(&owner, &evs);
        assert!(st.rejected.is_empty(), "{:?}", st.rejected);
        let reacts = &st.messages[&general][0].reactions;
        assert_eq!(reacts.len(), 1, "🎉 emptied and removed");
        assert_eq!(reacts["👍"], BTreeSet::from([user.id()]));

        // Only the reactor's own reaction is removed.
        chain(
            &mut evs,
            &owner,
            general,
            Payload::React {
                target: m,
                emoji: "👍".into(),
            },
        );
        chain(
            &mut evs,
            &user,
            general,
            Payload::Unreact {
                target: m,
                emoji: "👍".into(),
            },
        );
        let st = replay_all(&owner, &evs);
        assert_eq!(
            st.messages[&general][0].reactions["👍"],
            BTreeSet::from([owner.id()])
        );
    }

    #[test]
    fn reaction_rules_bans_deletes_and_validation() {
        let (owner, user) = (User::new(), User::new());
        let mut evs = Vec::new();
        let g = chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::ChannelCreate { name: "g".into() },
        );
        let general = ChannelId(g.0);
        let m = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message { text: "hi".into() },
        );
        let bad_text = chain(
            &mut evs,
            &user,
            general,
            Payload::React {
                target: m,
                emoji: "a".into(),
            },
        );
        let bad_space = chain(
            &mut evs,
            &user,
            general,
            Payload::React {
                target: m,
                emoji: "👍 ".into(),
            },
        );
        let bad_unreact = chain(
            &mut evs,
            &user,
            general,
            Payload::Unreact {
                target: m,
                emoji: "1".into(),
            },
        );
        chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::Ban { user: user.id() },
        );
        let banned = chain(
            &mut evs,
            &user,
            general,
            Payload::React {
                target: m,
                emoji: "🎉".into(),
            },
        );
        chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::Unban { user: user.id() },
        );
        chain(
            &mut evs,
            &owner,
            general,
            Payload::React {
                target: m,
                emoji: "🎉".into(),
            },
        );
        chain(&mut evs, &owner, general, Payload::Delete { target: m });
        let on_dead = chain(
            &mut evs,
            &owner,
            general,
            Payload::React {
                target: m,
                emoji: "🎉".into(),
            },
        );
        let dead_unreact = chain(
            &mut evs,
            &owner,
            general,
            Payload::Unreact {
                target: m,
                emoji: "🎉".into(),
            },
        );

        let st = replay_all(&owner, &evs);
        assert!(st.rejected.contains(&(bad_text, Rejection::Malformed)));
        assert!(st.rejected.contains(&(bad_space, Rejection::Malformed)));
        assert!(st.rejected.contains(&(bad_unreact, Rejection::Malformed)));
        assert!(st.rejected.contains(&(banned, Rejection::Banned)));
        assert!(st.rejected.contains(&(on_dead, Rejection::UnknownTarget)));
        assert!(st
            .rejected
            .contains(&(dead_unreact, Rejection::UnknownTarget)));
        assert_eq!(st.rejected.len(), 6);
        let original = &st.messages[&general][0];
        assert!(original.reactions.is_empty(), "deleting clears reactions");
    }

    #[test]
    fn reactions_follow_a_rotation() {
        let (owner, user, newer) = (User::new(), User::new(), User::new());
        let mut evs = Vec::new();
        let g = chain(
            &mut evs,
            &owner,
            ChannelId::CONTROL,
            Payload::ChannelCreate { name: "g".into() },
        );
        let general = ChannelId(g.0);
        let m = chain(
            &mut evs,
            &owner,
            general,
            Payload::Message { text: "hi".into() },
        );
        chain(
            &mut evs,
            &user,
            general,
            Payload::React {
                target: m,
                emoji: "👍".into(),
            },
        );
        chain(
            &mut evs,
            &newer,
            ChannelId::CONTROL,
            Payload::KeyRotation(KeyRotation::by_owner(&user.root, &newer.root, 1)),
        );
        let st = replay_all(&owner, &evs);
        assert!(st.rejected.is_empty(), "{:?}", st.rejected);
        assert_eq!(
            st.messages[&general][0].reactions["👍"],
            BTreeSet::from([newer.id()])
        );
    }

    #[test]
    fn concurrent_rotations_converge_on_the_lowest_event_id() {
        let (owner, admin, bob) = (User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        chain(
            &mut evs,
            &owner,
            ctl,
            Payload::RoleGrant {
                user: admin.id(),
                role: Role::Admin,
            },
        );
        chain(&mut evs, &owner, ctl, Payload::Ban { user: bob.id() });
        let base = evs.last().unwrap().id;
        let hlc = Hlc {
            wall_ms: 100,
            counter: 0,
        };
        let rotation = |sealed: u8| Payload::SecretRotation {
            generation: 1,
            covers: vec![bob.id()],
            sealed: vec![sealed],
        };
        // Two admins rotate from the same parent, neither having seen the other.
        let a = on_channel(&owner, ctl, vec![base], hlc, rotation(1));
        let b = on_channel(&admin, ctl, vec![base], hlc, rotation(2));
        let lo = if a.id < b.id { a.id } else { b.id };

        for order in [[a.clone(), b.clone()], [b.clone(), a.clone()]] {
            let mut all = evs.clone();
            all.extend(order);
            let st = replay_all(&owner, &all);
            assert_eq!(st.secret_generation, 1);
            assert_eq!(st.secret_winner, Some(lo), "the lowest event id wins");
        }
    }
}

#[cfg(test)]
mod mention_tests {
    use super::*;
    use crate::testutil::{on_channel, User};

    fn chain(evs: &mut Vec<Event>, u: &User, chan: ChannelId, payload: Payload) -> EventId {
        let parents = evs.last().map(|e| vec![e.id]).unwrap_or_default();
        let hlc = Hlc {
            wall_ms: evs.len() as u64,
            counter: 0,
        };
        let ev = on_channel(u, chan, parents, hlc, payload);
        let id = ev.id;
        evs.push(ev);
        id
    }

    fn replay_all(owner: &User, evs: &[Event]) -> CommunityState {
        let mut dag = Dag::new();
        for ev in evs {
            dag.insert(ev.clone()).unwrap();
        }
        replay(owner.id(), &dag)
    }

    fn msg(text: &str) -> Payload {
        Payload::Message { text: text.into() }
    }

    /// Mention-everyone belongs to the owner and admins at the point where a message is accepted. A
    /// later demotion does not change messages already accepted.
    #[test]
    fn mention_everyone_follows_the_role_when_the_message_was_accepted() {
        let (owner, admin, moderator, user) = (User::new(), User::new(), User::new(), User::new());
        let ctl = ChannelId::CONTROL;
        let mut evs = Vec::new();
        let general = ChannelId(
            chain(&mut evs, &owner, ctl, Payload::ChannelCreate { name: "general".into() }).0,
        );
        chain(&mut evs, &owner, ctl, Payload::RoleGrant { user: admin.id(), role: Role::Admin });
        chain(&mut evs, &owner, ctl, Payload::RoleGrant { user: moderator.id(), role: Role::Moderator });

        let by_owner = chain(&mut evs, &owner, general, msg("hi all"));
        let by_admin = chain(&mut evs, &admin, general, msg("hi all"));
        let by_moderator = chain(&mut evs, &moderator, general, msg("hi all"));
        let by_user = chain(&mut evs, &user, general, msg("hi all"));
        chain(&mut evs, &owner, ctl, Payload::RoleRevoke { user: admin.id(), role: Role::Admin });
        let after_demotion = chain(&mut evs, &admin, general, msg("hi all"));

        let st = replay_all(&owner, &evs);
        let flag = |id: EventId| {
            st.messages[&general]
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("message {id:?} was not accepted"))
                .mention_all
        };
        assert!(flag(by_owner), "the owner may mention everyone");
        assert!(flag(by_admin), "an admin may mention everyone");
        assert!(!flag(by_moderator), "a moderator may not");
        assert!(!flag(by_user), "a member may not");
        assert!(!flag(after_demotion), "a demoted admin may not, from then on");
        assert!(st.may_mention_everyone(&owner.id()));
        assert!(!st.may_mention_everyone(&user.id()));
    }
}

impl CommunityState {
    /// The icon a community shows: the picture to fetch and, for an animated icon, its poster frame.
    /// The animated icon shows while its event is at least as new as the still icon's, so a later
    /// still icon replaces it and a later removal clears both. `None` is no icon.
    pub fn shown_icon(&self) -> Option<(crate::Picture, Option<crate::Picture>)> {
        match (self.animated_icon, self.icon) {
            (Some((animated_hlc, animated)), icon) if icon.is_none_or(|(still_hlc, _)| animated_hlc >= still_hlc) => {
                animated.map(|a| (a.image, Some(a.poster)))
            }
            (_, Some((_, still))) => still.map(|p| (p, None)),
            _ => None,
        }
    }
}
