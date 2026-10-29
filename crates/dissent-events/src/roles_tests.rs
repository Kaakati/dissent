//! Custom roles and bots: who may make, change and assign roles, and when their bits count. See
//! `docs/design/14-file-tracker.md`, "Phase 0".

use crate::roles::{Perms, RoleId, RoleSpec};
use crate::state::replay_with;
use crate::testutil::{on_channel, User};
use crate::{ChannelId, Dag, EventId, Hlc, Payload, Rejection, Role};

struct Fixture {
    users: Vec<User>,
    dag: Dag,
    last: Option<EventId>,
    t: u64,
}

const OWNER: usize = 0;
const ADMIN: usize = 1;
const MANAGER: usize = 2;
const BOB: usize = 3;
const CAROL: usize = 4;

fn spec(name: &str, perms: Perms, position: u16) -> RoleSpec {
    RoleSpec { name: name.into(), color: 0x3366ff, perms, position }
}

impl Fixture {
    /// The owner, an admin, and three members: a manager, bob and carol.
    fn new() -> Self {
        let mut f = Fixture { users: (0..5).map(|_| User::new()).collect(), dag: Dag::new(), last: None, t: 0 };
        for who in [ADMIN, MANAGER, BOB, CAROL] {
            f.push(who, Payload::Join { display_name: format!("user{who}") });
        }
        let admin = f.id(ADMIN);
        f.push(OWNER, Payload::RoleGrant { user: admin, role: Role::Admin });
        f
    }

    fn id(&self, who: usize) -> dissent_identity::RootId {
        self.users[who].id()
    }

    fn push_in(&mut self, who: usize, channel: ChannelId, payload: Payload) -> EventId {
        self.t += 1;
        let hlc = Hlc { wall_ms: self.t, counter: 0 };
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = on_channel(&self.users[who], channel, parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).expect("insert");
        self.last = Some(id);
        id
    }

    fn push(&mut self, who: usize, payload: Payload) -> EventId {
        self.push_in(who, ChannelId::CONTROL, payload)
    }

    fn state(&self) -> crate::CommunityState {
        replay_with(self.id(OWNER), &self.dag, &[])
    }

    fn rejection(&self, id: EventId) -> Option<Rejection> {
        self.state().rejected.iter().find(|(r, _)| *r == id).map(|(_, why)| *why)
    }

    /// A role made by the admin, assigned to `who`.
    fn role_for(&mut self, who: usize, spec: RoleSpec) -> RoleId {
        let role = RoleId(self.push(ADMIN, Payload::RoleCreate { spec }).0);
        let user = self.id(who);
        self.push(ADMIN, Payload::RoleAssign { user, role, on: true });
        role
    }
}

#[test]
fn an_admin_creates_and_assigns_a_role() {
    let mut f = Fixture::new();
    let role = f.role_for(BOB, spec("Testers", Perms::default(), 1));
    let st = f.state();
    assert_eq!(st.custom.roles[&role].1.name, "Testers");
    assert_eq!(st.custom.roles_of(&f.id(BOB)), vec![role]);
}

#[test]
fn a_member_without_manage_roles_cannot_create_one() {
    let mut f = Fixture::new();
    let id = f.push(BOB, Payload::RoleCreate { spec: spec("Mine", Perms::default(), 1) });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
}

#[test]
fn a_manager_cannot_hand_out_bits_they_lack_or_roles_at_their_level() {
    let mut f = Fixture::new();
    f.role_for(MANAGER, spec("Managers", Perms::MANAGE_ROLES.union(Perms::MANAGE_FILES), 10));
    // Bits the manager lacks.
    let id = f.push(MANAGER, Payload::RoleCreate { spec: spec("Banners", Perms::BAN_MEMBERS, 1) });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
    // A role at or above the manager's own position.
    let id = f.push(MANAGER, Payload::RoleCreate { spec: spec("Peers", Perms::MANAGE_FILES, 10) });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
    // Below, with a subset of the bits: accepted, and assignable to a member below.
    let low = RoleId(f.push(MANAGER, Payload::RoleCreate { spec: spec("Curators", Perms::MANAGE_FILES, 5) }).0);
    let bob = f.id(BOB);
    let id = f.push(MANAGER, Payload::RoleAssign { user: bob, role: low, on: true });
    assert_eq!(f.rejection(id), None);
    assert!(f.state().perms_of(&bob).contains(Perms::MANAGE_FILES));
}

#[test]
fn a_manager_cannot_change_the_roles_of_someone_who_stands_as_high() {
    let mut f = Fixture::new();
    let top = f.role_for(MANAGER, spec("Managers", Perms::MANAGE_ROLES, 10));
    f.role_for(BOB, spec("Seniors", Perms::default(), 10));
    let low = RoleId(f.push(ADMIN, Payload::RoleCreate { spec: spec("Juniors", Perms::default(), 1) }).0);
    let bob = f.id(BOB);
    let id = f.push(MANAGER, Payload::RoleAssign { user: bob, role: low, on: true });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission), "bob stands level with the manager");
    // Nor edit or delete their own top role.
    let id = f.push(MANAGER, Payload::RoleDelete { role: top });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
}

#[test]
fn deleting_a_role_drops_its_assignments_and_refuses_late_uses() {
    let mut f = Fixture::new();
    let role = f.role_for(BOB, spec("Testers", Perms::MANAGE_FILES, 1));
    f.push(ADMIN, Payload::RoleDelete { role });
    let carol = f.id(CAROL);
    let id = f.push(ADMIN, Payload::RoleAssign { user: carol, role, on: true });
    assert_eq!(f.rejection(id), Some(Rejection::UnknownTarget));
    let st = f.state();
    assert!(st.custom.roles_of(&f.id(BOB)).is_empty());
    assert!(!st.perms_of(&f.id(BOB)).contains(Perms::MANAGE_FILES));
}

#[test]
fn the_latest_assignment_wins() {
    let mut f = Fixture::new();
    let role = f.role_for(BOB, spec("Testers", Perms::default(), 1));
    let bob = f.id(BOB);
    f.push(ADMIN, Payload::RoleAssign { user: bob, role, on: false });
    assert!(f.state().custom.roles_of(&bob).is_empty());
}

#[test]
fn custom_bits_count_on_released_kinds_only_while_the_owner_allows_it() {
    let mut f = Fixture::new();
    f.role_for(BOB, spec("Builders", Perms::MANAGE_CHANNELS.union(Perms::BAN_MEMBERS), 1));
    let early = f.push(BOB, Payload::ChannelCreate { name: "early".into() });
    assert_eq!(f.rejection(early), Some(Rejection::NoPermission), "a released client would refuse it too");

    let id = f.push(ADMIN, Payload::CustomRolePerms { on: true });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission), "owner only");
    f.push(OWNER, Payload::CustomRolePerms { on: true });
    let late = f.push(BOB, Payload::ChannelCreate { name: "late".into() });
    assert_eq!(f.rejection(late), None);

    // Ban: carol stands below bob, the admin does not.
    let carol = f.id(CAROL);
    let admin = f.id(ADMIN);
    let ban_admin = f.push(BOB, Payload::Ban { user: admin });
    assert_eq!(f.rejection(ban_admin), Some(Rejection::NoPermission));
    let ban_carol = f.push(BOB, Payload::Ban { user: carol });
    assert_eq!(f.rejection(ban_carol), None);
    assert!(f.state().banned.contains(&carol));
}

#[test]
fn a_member_with_the_same_standing_cannot_ban() {
    let mut f = Fixture::new();
    f.push(OWNER, Payload::CustomRolePerms { on: true });
    let role = f.role_for(BOB, spec("Guards", Perms::BAN_MEMBERS, 3));
    let carol = f.id(CAROL);
    f.push(ADMIN, Payload::RoleAssign { user: carol, role, on: true });
    let id = f.push(BOB, Payload::Ban { user: carol });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
}

#[test]
fn roles_events_belong_on_the_control_channel() {
    let mut f = Fixture::new();
    let chan = ChannelId(f.push(ADMIN, Payload::ChannelCreate { name: "general".into() }).0);
    let id = f.push_in(ADMIN, chan, Payload::RoleCreate { spec: spec("Testers", Perms::default(), 1) });
    assert_eq!(f.rejection(id), Some(Rejection::Malformed));
}

#[test]
fn names_and_colours_are_checked() {
    let mut f = Fixture::new();
    for bad in [spec("", Perms::default(), 1), spec(" padded", Perms::default(), 1), spec(&"x".repeat(33), Perms::default(), 1)] {
        let id = f.push(ADMIN, Payload::RoleCreate { spec: bad });
        assert_eq!(f.rejection(id), Some(Rejection::Malformed));
    }
    let mut colour = spec("Ok", Perms::default(), 1);
    colour.color = 0x0100_0000;
    let id = f.push(ADMIN, Payload::RoleCreate { spec: colour });
    assert_eq!(f.rejection(id), Some(Rejection::Malformed));
}

#[test]
fn bots_are_registered_by_admins_and_hold_roles() {
    let mut f = Fixture::new();
    let bot = User::new();
    let root = bot.id();
    let id = f.push(BOB, Payload::BotRegister { root, name: "CI bot".into(), on: true });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
    f.push(ADMIN, Payload::BotRegister { root, name: "CI bot".into(), on: true });
    let role = RoleId(f.push(ADMIN, Payload::RoleCreate { spec: spec("CI", Perms::default(), 1) }).0);
    f.push(ADMIN, Payload::RoleAssign { user: root, role, on: true });
    let st = f.state();
    assert!(st.is_bot(&root));
    assert_eq!(st.custom.bots[&root].name, "CI bot");
    assert_eq!(st.custom.roles_of(&root), vec![role]);
    let owner = f.id(OWNER);
    let id = f.push(ADMIN, Payload::BotRegister { root: owner, name: "nope".into(), on: true });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
}

#[test]
fn perm_names_round_trip() {
    let bits = Perms::MANAGE_FILES.union(Perms::MANAGE_BOTS);
    assert_eq!(bits.names(), vec!["manage_files", "manage_bots"]);
    assert_eq!(Perms::from_names(bits.names()), Some(bits));
    assert_eq!(Perms::from_names(["nope"]), None);
}

#[test]
fn a_carried_role_event_opens_and_applies() {
    use crate::control::{carry_sealed, seal};
    let mut f = Fixture::new();
    let secret = [7u8; 32];
    let body = crate::EventBody {
        community: dissent_identity::CommunityId([0; 32]),
        channel: ChannelId::CONTROL,
        author: f.id(ADMIN),
        device: f.users[ADMIN].device.id(),
        parents: f.last.into_iter().collect(),
        hlc: Hlc { wall_ms: 1000, counter: 0 },
        payload: Payload::RoleCreate { spec: spec("Sealed", Perms::default(), 1) },
    };
    let mut sealed = body.clone();
    sealed.payload = carry_sealed(&seal(&secret, 0, &body).unwrap()).unwrap();
    let ev = crate::Event::sign(sealed, &f.users[ADMIN].device);
    let id = ev.id;
    f.dag.insert(ev).unwrap();
    let st = replay_with(f.id(OWNER), &f.dag, &[(0, secret)]);
    assert!(st.rejected.iter().all(|(r, _)| *r != id), "{:?}", st.rejected);
    assert_eq!(st.custom.roles[&RoleId(id.0)].1.name, "Sealed");
    // Without the key it is locked, as a released client sees it.
    let st = replay_with(f.id(OWNER), &f.dag, &[]);
    assert!(st.rejected.iter().any(|(r, why)| *r == id && *why == Rejection::Locked));
}
