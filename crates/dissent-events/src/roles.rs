//! Custom roles and bots. Built-in `Role::Admin` and `Role::Moderator` keep their rules; a custom role adds a set of
//! permission bits below them. See `docs/design/14-file-tracker.md`, "Phase 0".

use std::collections::{BTreeMap, BTreeSet};

use dissent_identity::RootId;
use serde::{Deserialize, Serialize};

use crate::event::digest_id;
use crate::state::{Rank, Rejection};
use crate::{ChannelId, CommunityState, Event, Hlc, Payload};

digest_id!(RoleId, "A custom role: the id of the `RoleCreate` event that made it.");

/// A set of permission bits. Unknown bits are kept and ignored, so a later permission needs no wire change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Perms(pub u64);

impl Perms {
    pub const MANAGE_ROLES: Self = Self(1 << 0);
    pub const MANAGE_CHANNELS: Self = Self(1 << 1);
    pub const MANAGE_SERVER: Self = Self(1 << 2);
    pub const MANAGE_MESSAGES: Self = Self(1 << 3);
    pub const BAN_MEMBERS: Self = Self(1 << 4);
    pub const MANAGE_INVITES: Self = Self(1 << 5);
    pub const MENTION_EVERYONE: Self = Self(1 << 6);
    pub const BYPASS_SLOWMODE: Self = Self(1 << 7);
    pub const MANAGE_FILES: Self = Self(1 << 8);
    pub const MANAGE_BOTS: Self = Self(1 << 9);
    pub const MANAGE_STORAGE: Self = Self(1 << 10);
    /// Every bit, known or not. What the owner and admins hold.
    pub const ALL: Self = Self(u64::MAX);
    /// What the moderator rank allows without custom roles.
    pub const MODERATOR: Self = Self(Self::MANAGE_MESSAGES.0 | Self::BAN_MEMBERS.0 | Self::BYPASS_SLOWMODE.0);
    /// Every bit this version knows, with its name, in bit order.
    pub const NAMED: &'static [(Self, &'static str)] = &[
        (Self::MANAGE_ROLES, "manage_roles"),
        (Self::MANAGE_CHANNELS, "manage_channels"),
        (Self::MANAGE_SERVER, "manage_server"),
        (Self::MANAGE_MESSAGES, "manage_messages"),
        (Self::BAN_MEMBERS, "ban_members"),
        (Self::MANAGE_INVITES, "manage_invites"),
        (Self::MENTION_EVERYONE, "mention_everyone"),
        (Self::BYPASS_SLOWMODE, "bypass_slowmode"),
        (Self::MANAGE_FILES, "manage_files"),
        (Self::MANAGE_BOTS, "manage_bots"),
        (Self::MANAGE_STORAGE, "manage_storage"),
    ];

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The names of the known bits that are set.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMED.iter().filter(|(bit, _)| self.contains(*bit)).map(|(_, name)| *name).collect()
    }

    /// The bits for `names`. An unknown name is `None`.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        names.into_iter().try_fold(Self::default(), |acc, name| {
            Self::NAMED.iter().find(|(_, n)| *n == name).map(|(bit, _)| acc.union(*bit))
        })
    }
}

/// What a custom role is: its name, colour (0xRRGGBB, 0 for none), permission bits and position. A higher position
/// ranks higher; every custom role ranks below Moderator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    pub name: String,
    pub color: u32,
    pub perms: Perms,
    pub position: u16,
}

/// Longest role or bot name, in characters.
pub const MAX_ROLE_NAME_CHARS: usize = 32;
/// Most live custom roles a community may have.
pub const MAX_CUSTOM_ROLES: usize = 64;

/// Whether `name` is a role or bot name: trimmed, not empty, at most `MAX_ROLE_NAME_CHARS` characters.
pub fn valid_role_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && name.chars().count() <= MAX_ROLE_NAME_CHARS
}

/// A bot's registration: its name and whether it is marked, with the HLC of the latest `BotRegister`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotEntry {
    pub hlc: Hlc,
    pub name: String,
    pub on: bool,
}

/// Custom roles, their assignments and bots, as replay computes them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBook {
    /// Live custom roles, with the HLC of the spec in force.
    pub roles: BTreeMap<RoleId, (Hlc, RoleSpec)>,
    /// Roles deleted for good. A late update or assignment of one is refused.
    pub deleted: BTreeSet<RoleId>,
    /// The latest `RoleAssign` per canonical root and role.
    pub assigned: BTreeMap<RootId, BTreeMap<RoleId, (Hlc, bool)>>,
    pub bots: BTreeMap<RootId, BotEntry>,
    /// Whether custom bits count on the kinds released clients know, from the latest `CustomRolePerms`.
    pub honored: bool,
    pub honored_hlc: Hlc,
}

impl RoleBook {
    /// The live custom roles `root` holds. `root` must be canonical.
    pub fn roles_of(&self, root: &RootId) -> Vec<RoleId> {
        self.assigned
            .get(root)
            .into_iter()
            .flatten()
            .filter(|(role, (_, on))| *on && self.roles.contains_key(role))
            .map(|(role, _)| *role)
            .collect()
    }

    /// Whether `root` is a registered bot.
    pub fn is_bot(&self, root: &RootId) -> bool {
        self.bots.get(root).is_some_and(|b| b.on)
    }

    /// The union of the bits of the live custom roles `root` holds.
    fn custom_bits(&self, root: &RootId) -> Perms {
        self.roles_of(root)
            .iter()
            .filter_map(|r| self.roles.get(r))
            .fold(Perms::default(), |acc, (_, spec)| acc.union(spec.perms))
    }

    /// The highest position among the live custom roles `root` holds.
    fn top_position(&self, root: &RootId) -> Option<u16> {
        self.roles_of(root).iter().filter_map(|r| self.roles.get(r)).map(|(_, s)| s.position).max()
    }

    fn migrate(&mut self, old: RootId, new: RootId) {
        if let Some(held) = self.assigned.remove(&old) {
            self.assigned.insert(new, held);
        }
        if let Some(bot) = self.bots.remove(&old) {
            self.bots.insert(new, bot);
        }
    }
}

/// Where a root stands, for "below the actor": its rank, then its highest custom position.
type Standing = (Rank, Option<u16>);

impl CommunityState {
    /// Every bit `root` holds: all of them for the owner and admins, the moderator's for a moderator, plus the
    /// bits of its custom roles.
    pub fn perms_of(&self, root: &RootId) -> Perms {
        let root = self.canonical(root);
        let base = match self.rank(&root) {
            Rank::Owner | Rank::Admin => Perms::ALL,
            Rank::Moderator => Perms::MODERATOR,
            Rank::User => Perms::default(),
        };
        base.union(self.custom.custom_bits(&root))
    }

    /// Whether `root` is a registered bot.
    pub fn is_bot(&self, root: &RootId) -> bool {
        self.custom.is_bot(&self.canonical(root))
    }

    fn standing(&self, root: &RootId) -> Standing {
        let root = self.canonical(root);
        (self.rank(&root), self.custom.top_position(&root))
    }

    /// Whether a role at `position` sits below `actor`. Moderators and above outrank every custom role.
    fn position_below(&self, actor: &RootId, position: u16) -> bool {
        match self.standing(actor) {
            (rank, _) if rank >= Rank::Moderator => true,
            (_, top) => top.is_some_and(|top| position < top),
        }
    }

    /// Whether `actor`, of `rank`, may do something released clients gate at `need`. Custom bits count only
    /// while `CustomRolePerms` is on, so every client agrees until all of them know custom roles.
    pub(crate) fn may(&self, actor: &RootId, rank: Rank, need: Rank, bit: Perms) -> bool {
        rank >= need || (self.custom.honored && self.custom.custom_bits(&self.canonical(actor)).contains(bit))
    }

    /// Whether `actor` may ban or unban `user`. The rank rule released clients use, or, while custom bits count,
    /// `BAN_MEMBERS` over someone who stands below.
    pub(crate) fn may_ban(&self, actor: &RootId, rank: Rank, user: &RootId) -> bool {
        let target = self.rank(&self.canonical(user));
        if target < rank {
            return true;
        }
        rank == Rank::User
            && target == Rank::User
            && self.custom.honored
            && self.custom.custom_bits(&self.canonical(actor)).contains(Perms::BAN_MEMBERS)
            && self.standing(user) < self.standing(actor)
    }

    /// Checks a roles payload. `rank` is the actor's.
    pub(crate) fn check_roles(&self, ev: &Event, rank: Rank) -> Result<(), Rejection> {
        let actor = ev.body.author;
        if ev.body.channel != ChannelId::CONTROL {
            return Err(Rejection::Malformed);
        }
        let perms = self.perms_of(&actor);
        let need = |bit: Perms| if perms.contains(bit) { Ok(()) } else { Err(Rejection::NoPermission) };
        let live = |role: &RoleId| match self.custom.roles.get(role) {
            Some((_, spec)) => Ok(spec),
            None => Err(Rejection::UnknownTarget),
        };
        let spec_ok = |spec: &RoleSpec| {
            if !valid_role_name(&spec.name) || spec.color > 0xff_ffff {
                return Err(Rejection::Malformed);
            }
            if !perms.contains(spec.perms) || !self.position_below(&actor, spec.position) {
                return Err(Rejection::NoPermission);
            }
            Ok(())
        };
        match &ev.body.payload {
            Payload::RoleCreate { spec } => {
                need(Perms::MANAGE_ROLES)?;
                spec_ok(spec)?;
                if self.custom.roles.len() >= MAX_CUSTOM_ROLES {
                    return Err(Rejection::Malformed);
                }
            }
            Payload::RoleUpdate { role, spec } => {
                need(Perms::MANAGE_ROLES)?;
                if !self.position_below(&actor, live(role)?.position) {
                    return Err(Rejection::NoPermission);
                }
                spec_ok(spec)?;
            }
            Payload::RoleDelete { role } => {
                need(Perms::MANAGE_ROLES)?;
                if !self.position_below(&actor, live(role)?.position) {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::RoleAssign { user, role, .. } => {
                need(Perms::MANAGE_ROLES)?;
                if !self.position_below(&actor, live(role)?.position) {
                    return Err(Rejection::NoPermission);
                }
                let user = self.canonical(user);
                if self.banned.contains(&user) {
                    return Err(Rejection::Banned);
                }
                if user != actor && self.standing(&user) >= self.standing(&actor) {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::BotRegister { root, name, .. } => {
                need(Perms::MANAGE_BOTS)?;
                if !valid_role_name(name) {
                    return Err(Rejection::Malformed);
                }
                if self.canonical(root) == self.owner {
                    return Err(Rejection::NoPermission);
                }
            }
            Payload::CustomRolePerms { .. } => {
                if rank < Rank::Owner {
                    return Err(Rejection::NoPermission);
                }
            }
            _ => unreachable!("check_roles takes roles payloads only"),
        }
        Ok(())
    }

    /// Applies a roles payload that `check_roles` accepted.
    pub(crate) fn commit_roles(&mut self, ev: &Event) {
        let hlc = ev.body.hlc;
        match &ev.body.payload {
            Payload::RoleCreate { spec } => {
                self.custom.roles.insert(RoleId(ev.id.0), (hlc, spec.clone()));
            }
            Payload::RoleUpdate { role, spec } => {
                if let Some(entry) = self.custom.roles.get_mut(role) {
                    if hlc > entry.0 {
                        *entry = (hlc, spec.clone());
                    }
                }
            }
            Payload::RoleDelete { role } => {
                self.custom.roles.remove(role);
                self.custom.deleted.insert(*role);
                for held in self.custom.assigned.values_mut() {
                    held.remove(role);
                }
                self.custom.assigned.retain(|_, held| !held.is_empty());
            }
            Payload::RoleAssign { user, role, on } => {
                let user = self.canonical(user);
                let held = self.custom.assigned.entry(user).or_default();
                if held.get(role).is_none_or(|(at, _)| hlc > *at) {
                    held.insert(*role, (hlc, *on));
                }
            }
            Payload::BotRegister { root, name, on } => {
                let root = self.canonical(root);
                if self.custom.bots.get(&root).is_none_or(|b| hlc > b.hlc) {
                    self.custom.bots.insert(root, BotEntry { hlc, name: name.clone(), on: *on });
                }
            }
            Payload::CustomRolePerms { on } => {
                if hlc > self.custom.honored_hlc {
                    self.custom.honored = *on;
                    self.custom.honored_hlc = hlc;
                }
            }
            _ => unreachable!("commit_roles takes roles payloads only"),
        }
    }

    pub(crate) fn migrate_roles(&mut self, old: RootId, new: RootId) {
        self.custom.migrate(old, new);
    }
}

/// Whether `payload` is one of the roles payloads.
pub fn is_roles_payload(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::RoleCreate { .. }
            | Payload::RoleUpdate { .. }
            | Payload::RoleDelete { .. }
            | Payload::RoleAssign { .. }
            | Payload::BotRegister { .. }
            | Payload::CustomRolePerms { .. }
    )
}
