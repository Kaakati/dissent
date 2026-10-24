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

use crate::emoji::normalize_emoji;
use crate::names::{normalize_handle, skeleton};
use crate::{ChannelId, Dag, Event, EventId, Hlc, Payload, Role, SealedKind};
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
    /// Device to the root that signed its events. Learned from every event applied.
    pub device_roots: BTreeMap<DeviceId, RootId>,
    pub rejected: Vec<(EventId, Rejection)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub display_name: String,
    /// HLC of the first accepted Join. A rejoin does not move it.
    pub joined: Hlc,
    pub left: bool,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    User,
    Moderator,
    Admin,
    Owner,
}

/// Replay the DAG's causal order against a fresh state owned by `owner`.
/// This is the reference implementation. [`crate::Replayer`] must match it.
pub fn replay(owner: RootId, dag: &Dag) -> CommunityState {
    let mut st = CommunityState::new(owner);
    for ev in dag.causal_order() {
        apply_event(&mut st, ev);
    }
    st
}

/// Apply one event at its causal position, recording a rejection if it has one.
pub(crate) fn apply_event(st: &mut CommunityState, ev: &Event) {
    st.device_roots
        .entry(ev.body.device)
        .or_insert(ev.body.author);
    match st.check(ev) {
        Ok(()) => st.commit(ev),
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
            device_roots: BTreeMap::new(),
            rejected: Vec::new(),
        }
    }

    /// The root that now stands for `root`. Follows rotations transitively.
    pub fn canonical(&self, root: &RootId) -> RootId {
        let mut cur = *root;
        while let Some(next) = self.aliases.get(&cur) {
            cur = *next;
        }
        cur
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
            .is_some_and(|revoked_at| ev.body.hlc > *revoked_at)
        {
            return Err(Rejection::DeviceRevoked);
        }
        if self.banned.contains(&actor) {
            return Err(Rejection::Banned);
        }
        let rank = self.rank(&actor);
        let chan = ev.body.channel;
        match &ev.body.payload {
            Payload::ChannelCreate { name } | Payload::VoiceChannelCreate { name } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if name.trim().is_empty() {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::Sealed { kind, .. } => match kind {
                SealedKind::Message => {
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
            Payload::Join { display_name } => {
                require_control(chan)?;
                if display_name.trim().is_empty() {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::Leave => require_control(chan)?,
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
        }
        Ok(())
    }

    /// Apply an event that `check` accepted against this same state.
    pub(crate) fn commit(&mut self, ev: &Event) {
        let actor = ev.body.author;
        let chan = ev.body.channel;
        match &ev.body.payload {
            Payload::ChannelCreate { name } | Payload::VoiceChannelCreate { name } => {
                let id = ChannelId(ev.id.0);
                self.channels.insert(id, name.clone());
                self.messages.entry(id).or_default();
                if matches!(ev.body.payload, Payload::VoiceChannelCreate { .. }) {
                    self.voice_channels.insert(id);
                }
            }
            Payload::Sealed { kind, .. } => match kind {
                SealedKind::Message => {
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
                    });
                }
                SealedKind::Edit { target } => {
                    if let Some(m) = self.live_message_mut(chan, target) {
                        m.text.clear();
                        m.sealed = true;
                        m.edited = true;
                    }
                }
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
            Payload::Join { display_name } => {
                let m = self.members.entry(actor).or_insert_with(|| Member {
                    display_name: String::new(),
                    joined: ev.body.hlc,
                    left: false,
                });
                m.display_name = display_name.clone();
                m.left = false;
            }
            Payload::Leave => {
                if let Some(m) = self.members.get_mut(&actor) {
                    m.left = true;
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
}
