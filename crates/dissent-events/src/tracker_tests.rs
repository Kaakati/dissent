//! File tracker replay: who may make categories and list files, restricted categories, receipts and levels. See
//! `docs/design/14-file-tracker.md`, "Events".

use crate::roles::{Perms, RoleId, RoleSpec};
use crate::state::replay_with;
use crate::testutil::{on_channel, User};
use crate::tracker::{AddSpec, CategoryId, FilesSealed, LevelRule, ReaderSpec, ReputationRules, NO_RANK};
use crate::{ChannelId, Dag, EventId, Hlc, Payload, Rejection, Role};

const OWNER: usize = 0;
const ADMIN: usize = 1;
const BOB: usize = 2;
const CAROL: usize = 3;
const DAN: usize = 4;
const DAY: u64 = 24 * 60 * 60 * 1000;

struct Fixture {
    users: Vec<User>,
    dag: Dag,
    last: Option<EventId>,
    t: u64,
}

fn sealed() -> FilesSealed {
    FilesSealed { generation: 0, ciphertext: vec![1; 40] }
}

fn blob(n: u8) -> Option<([u8; 32], u64)> {
    Some(([n; 32], 1000))
}

impl Fixture {
    fn new() -> Self {
        let mut f = Fixture { users: (0..5).map(|_| User::new()).collect(), dag: Dag::new(), last: None, t: 0 };
        for who in [ADMIN, BOB, CAROL, DAN] {
            f.push(who, Payload::Join { display_name: format!("user{who}") });
        }
        let admin = f.id(ADMIN);
        f.push(OWNER, Payload::RoleGrant { user: admin, role: Role::Admin });
        f
    }

    fn id(&self, who: usize) -> dissent_identity::RootId {
        self.users[who].id()
    }

    fn push_at(&mut self, who: usize, wall_ms: u64, payload: Payload) -> EventId {
        self.t = self.t.max(wall_ms);
        let hlc = Hlc { wall_ms: self.t, counter: 0 };
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = on_channel(&self.users[who], ChannelId::CONTROL, parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).expect("insert");
        self.last = Some(id);
        id
    }

    fn push(&mut self, who: usize, payload: Payload) -> EventId {
        let next = self.t + 1;
        self.push_at(who, next, payload)
    }

    fn state(&self) -> crate::CommunityState {
        replay_with(self.id(OWNER), &self.dag, &[])
    }

    fn rejection(&self, id: EventId) -> Option<Rejection> {
        self.state().rejected.iter().find(|(r, _)| *r == id).map(|(_, why)| *why)
    }

    fn category(&mut self, add: AddSpec, visible: Option<ReaderSpec>) -> CategoryId {
        CategoryId(self.push(ADMIN, Payload::FileCategorySet { category: None, add, visible, inner: sealed() }).0)
    }

    fn list(&mut self, who: usize, category: CategoryId, n: u8) -> EventId {
        self.push(who, Payload::FileListing { category, blob: blob(n), inner: sealed() })
    }
}

fn admins_only() -> AddSpec {
    AddSpec { min_rank: 2, ..AddSpec::default() }
}

#[test]
fn only_file_managers_make_categories() {
    let mut f = Fixture::new();
    let id = f.push(BOB, Payload::FileCategorySet { category: None, add: AddSpec::default(), visible: None, inner: sealed() });
    assert_eq!(f.rejection(id), Some(Rejection::NoPermission));
    let cat = f.category(AddSpec::default(), None);
    assert!(f.state().files.categories.contains_key(&cat));

    // A custom role with MANAGE_FILES counts, since these kinds are new.
    let role = RoleId(f.push(ADMIN, Payload::RoleCreate { spec: RoleSpec { name: "Curators".into(), color: 0, perms: Perms::MANAGE_FILES, position: 1 } }).0);
    let bob = f.id(BOB);
    f.push(ADMIN, Payload::RoleAssign { user: bob, role, on: true });
    let id = f.push(BOB, Payload::FileCategorySet { category: Some(cat), add: admins_only(), visible: None, inner: sealed() });
    assert_eq!(f.rejection(id), None);
    assert_eq!(f.state().files.categories[&cat].add.min_rank, 2);
}

#[test]
fn add_spec_paths_admit_by_rank_role_member_or_level() {
    let mut f = Fixture::new();
    let role = RoleId(f.push(ADMIN, Payload::RoleCreate { spec: RoleSpec { name: "CI".into(), color: 0, perms: Perms::default(), position: 1 } }).0);
    let (carol, dan) = (f.id(CAROL), f.id(DAN));
    f.push(ADMIN, Payload::RoleAssign { user: carol, role, on: true });
    let cat = f.category(AddSpec { min_rank: NO_RANK, roles: vec![role], members: vec![dan], min_level: 0 }, None);

    let bob = f.list(BOB, cat, 1);
    assert_eq!(f.rejection(bob), Some(Rejection::NoPermission), "no path admits bob");
    let carol_listing = f.list(CAROL, cat, 2);
    assert_eq!(f.rejection(carol_listing), None, "carol holds the role");
    let dan_listing = f.list(DAN, cat, 3);
    assert_eq!(f.rejection(dan_listing), None, "dan is listed");
    let admin = f.list(ADMIN, cat, 4);
    assert_eq!(f.rejection(admin), None, "file managers always may");

    let open = f.category(AddSpec::default(), None);
    let id = f.list(BOB, open, 5);
    assert_eq!(f.rejection(id), None, "min_rank 0 admits every member");
}

#[test]
fn a_level_gate_opens_once_receipts_credit_enough() {
    let mut f = Fixture::new();
    let rules = ReputationRules {
        levels: vec![
            LevelRule { name: "Newcomer".into(), bytes: 0, members: 0 },
            LevelRule { name: "Sharer".into(), bytes: 1000, members: 2 },
        ],
        ..ReputationRules::default()
    };
    f.push(ADMIN, Payload::ReputationRulesSet { rules });
    let cat = f.category(AddSpec { min_rank: NO_RANK, min_level: 2, ..AddSpec::default() }, None);
    let bob = f.id(BOB);
    let early = f.list(BOB, cat, 1);
    assert_eq!(f.rejection(early), Some(Rejection::NoPermission));

    f.push(CAROL, Payload::FileReceipt { tag: [1; 16], credits: vec![(bob, 600)] });
    f.push(DAN, Payload::FileReceipt { tag: [1; 16], credits: vec![(bob, 600)] });
    let st = f.state();
    assert_eq!(st.files.credited_bytes(&bob, f.t), 1200);
    assert_eq!(st.files.served_members(&bob), 2);
    assert_eq!(st.files.level_of(&bob, f.t), 2);
    let late = f.list(BOB, cat, 2);
    assert_eq!(f.rejection(late), None);
}

#[test]
fn receipts_count_once_per_tag_never_for_oneself_and_within_bounds() {
    let mut f = Fixture::new();
    let (bob, carol) = (f.id(BOB), f.id(CAROL));
    f.push(CAROL, Payload::FileReceipt { tag: [9; 16], credits: vec![(bob, 500), (carol, 500)] });
    let again = f.push(CAROL, Payload::FileReceipt { tag: [9; 16], credits: vec![(bob, 500)] });
    assert_eq!(f.rejection(again), Some(Rejection::Malformed), "the same download counts once");
    let empty = f.push(CAROL, Payload::FileReceipt { tag: [8; 16], credits: vec![] });
    assert_eq!(f.rejection(empty), Some(Rejection::Malformed));
    let huge = f.push(CAROL, Payload::FileReceipt { tag: [7; 16], credits: vec![(bob, u64::MAX)] });
    assert_eq!(f.rejection(huge), Some(Rejection::Malformed));
    let st = f.state();
    assert_eq!(st.files.credited_bytes(&bob, f.t), 500);
    assert_eq!(st.files.credited_bytes(&carol, f.t), 0, "no credit to oneself");

    // With distinct counting on (the default), carol's next download adds nothing more to bob.
    f.push(CAROL, Payload::FileReceipt { tag: [6; 16], credits: vec![(bob, 500)] });
    assert_eq!(f.state().files.credited_bytes(&bob, f.t), 500);
}

#[test]
fn credit_halves_after_each_decay_period() {
    let mut f = Fixture::new();
    let bob = f.id(BOB);
    f.push_at(CAROL, 10 * DAY, Payload::FileReceipt { tag: [1; 16], credits: vec![(bob, 1000)] });
    let st = f.state();
    assert_eq!(st.files.credited_bytes(&bob, 10 * DAY + 29 * DAY), 1000);
    assert_eq!(st.files.credited_bytes(&bob, 10 * DAY + 30 * DAY), 500);
    assert_eq!(st.files.credited_bytes(&bob, 10 * DAY + 61 * DAY), 250);
}

#[test]
fn listers_and_file_managers_edit_and_remove_listings() {
    let mut f = Fixture::new();
    let cat = f.category(AddSpec::default(), None);
    let listing = f.list(BOB, cat, 1);
    let carol = f.push(CAROL, Payload::FileListingRemove { listing });
    assert_eq!(f.rejection(carol), Some(Rejection::NoPermission));
    let update = f.push(BOB, Payload::FileListingUpdate { listing, inner: sealed() });
    assert_eq!(f.rejection(update), None);
    assert!(f.state().files.listings[&listing].update.is_some());
    f.push(ADMIN, Payload::FileListingRemove { listing });
    let st = f.state();
    assert!(!st.files.listings.contains_key(&listing));
    let late = f.push(BOB, Payload::FileListingUpdate { listing, inner: sealed() });
    assert_eq!(f.rejection(late), Some(Rejection::UnknownTarget));
}

#[test]
fn deleting_a_category_drops_its_listings() {
    let mut f = Fixture::new();
    let cat = f.category(AddSpec::default(), None);
    let listing = f.list(BOB, cat, 1);
    f.push(ADMIN, Payload::FileCategoryDelete { category: cat });
    let st = f.state();
    assert!(!st.files.categories.contains_key(&cat));
    assert!(!st.files.listings.contains_key(&listing));
    let late = f.list(BOB, cat, 2);
    assert_eq!(f.rejection(late), Some(Rejection::UnknownTarget));
}

#[test]
fn a_restricted_category_keeps_blobs_out_of_the_clear() {
    let mut f = Fixture::new();
    let carol = f.id(CAROL);
    let cat = f.category(AddSpec::default(), Some(ReaderSpec { min_rank: NO_RANK, roles: vec![], members: vec![carol] }));
    let clear = f.list(BOB, cat, 1);
    assert_eq!(f.rejection(clear), Some(Rejection::Malformed), "a blob in clear would let seeders mirror it");
    let hidden = f.push(BOB, Payload::FileListing { category: cat, blob: None, inner: sealed() });
    assert_eq!(f.rejection(hidden), None);
    let st = f.state();
    assert!(st.may_see_files(&carol, &cat));
    assert!(!st.may_see_files(&f.id(DAN), &cat));
    assert!(st.may_see_files(&f.id(ADMIN), &cat), "file managers see every category");

    let open = f.category(AddSpec::default(), None);
    let missing = f.push(BOB, Payload::FileListing { category: open, blob: None, inner: sealed() });
    assert_eq!(f.rejection(missing), Some(Rejection::Malformed), "an open category's listing names its blob");
}

#[test]
fn reputation_resets_follow_the_rules() {
    let mut f = Fixture::new();
    let bob = f.id(BOB);
    f.push(CAROL, Payload::FileReceipt { tag: [1; 16], credits: vec![(bob, 1000)] });
    let by_member = f.push(DAN, Payload::ReputationReset { root: bob });
    assert_eq!(f.rejection(by_member), Some(Rejection::NoPermission));
    f.push(ADMIN, Payload::ReputationReset { root: bob });
    assert_eq!(f.state().files.credited_bytes(&bob, f.t), 0);

    let rules = ReputationRules { admins_reset: false, ..ReputationRules::default() };
    f.push(ADMIN, Payload::ReputationRulesSet { rules });
    let refused = f.push(ADMIN, Payload::ReputationReset { root: bob });
    assert_eq!(f.rejection(refused), Some(Rejection::NoPermission));
}

#[test]
fn sealed_fields_are_bounded() {
    let mut f = Fixture::new();
    let big = FilesSealed { generation: 0, ciphertext: vec![0; crate::tracker::MAX_SEALED_BYTES + 1] };
    let id = f.push(ADMIN, Payload::FileCategorySet { category: None, add: AddSpec::default(), visible: None, inner: big });
    assert_eq!(f.rejection(id), Some(Rejection::Malformed));
    let empty = FilesSealed { generation: 0, ciphertext: vec![] };
    let id = f.push(ADMIN, Payload::FileCategorySet { category: None, add: AddSpec::default(), visible: None, inner: empty });
    assert_eq!(f.rejection(id), Some(Rejection::Malformed));
}

#[test]
fn the_default_levels_are_the_designs() {
    let names: Vec<String> = ReputationRules::default().levels.into_iter().map(|l| l.name).collect();
    assert_eq!(names, ["Newcomer", "Sharer", "Seeder", "Keeper", "Archivist"]);
}
