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
//! Handle claims are compared after trimming and lowercasing. The first claim in
//! causal order wins. A user who claims a new handle gives up the old one.
//!
//! Each event goes through `check` (read only) and then `commit`. The incremental
//! [`crate::Replayer`] uses the same two steps, so both paths agree.

use crate::{ChannelId, Dag, Event, EventId, Hlc, Payload, Role};
use dissent_identity::RootId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityState {
    pub owner: RootId,
    pub roles: BTreeMap<RootId, BTreeSet<Role>>,
    pub banned: BTreeSet<RootId>,
    pub handles: BTreeMap<String, RootId>,
    pub channels: BTreeMap<ChannelId, String>,
    pub messages: BTreeMap<ChannelId, Vec<MessageView>>,
    /// Owner is not implicitly a member. The creator emits a Join like everyone else.
    pub members: BTreeMap<RootId, Member>,
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
    pub author: RootId,
    pub hlc: Hlc,
    pub text: String,
    /// Set by the first accepted Edit.
    pub edited: bool,
    /// Deleted messages keep their slot in the channel with empty text.
    pub deleted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    NoPermission,
    Banned,
    UnknownChannel,
    UnknownTarget,
    HandleTaken,
    NotAuthorOfTarget,
    /// Wrong channel for the payload, or an empty name or handle.
    Malformed,
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
            channels: BTreeMap::new(),
            messages: BTreeMap::new(),
            members: BTreeMap::new(),
            rejected: Vec::new(),
        }
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

    /// Would `ev` be rejected by this state? Reads only, never mutates.
    pub(crate) fn check(&self, ev: &Event) -> Result<(), Rejection> {
        let actor = ev.body.author;
        if self.banned.contains(&actor) {
            return Err(Rejection::Banned);
        }
        let rank = self.rank(&actor);
        let chan = ev.body.channel;
        match &ev.body.payload {
            Payload::ChannelCreate { name } => {
                require_control(chan)?;
                if rank < Rank::Admin {
                    return Err(Rejection::NoPermission);
                }
                if name.trim().is_empty() {
                    return Err(Rejection::Malformed);
                }
            }
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
                if self.rank(user) >= rank {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::HandleClaim { handle } => {
                require_control(chan)?;
                let key = handle.trim().to_lowercase();
                if key.is_empty() {
                    return Err(Rejection::Malformed);
                }
                if self.handles.get(&key).is_some_and(|h| *h != actor) {
                    return Err(Rejection::HandleTaken);
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
            Payload::ChannelCreate { name } => {
                let id = ChannelId(ev.id.0);
                self.channels.insert(id, name.clone());
                self.messages.entry(id).or_default();
            }
            Payload::Message { text } => {
                self.messages.entry(chan).or_default().push(MessageView {
                    id: ev.id,
                    author: actor,
                    hlc: ev.body.hlc,
                    text: text.clone(),
                    edited: false,
                    deleted: false,
                });
            }
            Payload::Edit { target, text } => {
                if let Some(m) = self.live_message_mut(chan, target) {
                    m.text = text.clone();
                    m.edited = true;
                }
            }
            Payload::Delete { target } => {
                if let Some(m) = self.live_message_mut(chan, target) {
                    m.deleted = true;
                    m.text.clear();
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
                self.roles.entry(*user).or_default().insert(*role);
            }
            Payload::RoleRevoke { user, role } => {
                if let Some(set) = self.roles.get_mut(user) {
                    set.remove(role);
                    if set.is_empty() {
                        self.roles.remove(user);
                    }
                }
            }
            Payload::Ban { user } => {
                self.banned.insert(*user);
            }
            Payload::Unban { user } => {
                self.banned.remove(user);
            }
            Payload::HandleClaim { handle } => {
                let key = handle.trim().to_lowercase();
                if self.handles.get(&key) != Some(&actor) {
                    self.handles.retain(|_, u| *u != actor);
                    self.handles.insert(key, actor);
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
