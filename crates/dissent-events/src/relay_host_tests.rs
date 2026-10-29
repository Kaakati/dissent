//! Who may publish `RelayHostSet`: admins always, and members only while the community's "Members can
//! host relays" setting is on. See `docs/design/12-boosts.md`.

use crate::state::replay_with;
use crate::testutil::{User, event};
use crate::{Dag, EventId, Hlc, Payload, Rejection};

const RELAY: &str = "https://relay.example.org";

struct Fixture {
    owner: User,
    alice: User,
    bob: User,
    dag: Dag,
    last: Option<EventId>,
    t: u64,
}

impl Fixture {
    /// The owner, with alice (an admin) and bob (a plain member) joined. Alice is granted admin.
    fn new() -> Self {
        let mut f = Fixture {
            owner: User::new(),
            alice: User::new(),
            bob: User::new(),
            dag: Dag::new(),
            last: None,
            t: 0,
        };
        let (alice, bob) = (f.alice.id(), f.bob.id());
        f.push_as(Who::Alice, Payload::Join { display_name: "alice".into() });
        f.push_as(Who::Bob, Payload::Join { display_name: "bob".into() });
        f.push_as(Who::Owner, Payload::RoleGrant { user: alice, role: crate::Role::Admin });
        let _ = bob;
        f
    }

    fn user(&self, who: Who) -> &User {
        match who {
            Who::Owner => &self.owner,
            Who::Alice => &self.alice,
            Who::Bob => &self.bob,
        }
    }

    /// Adds a control event by `who`, citing the last one, and returns its id.
    fn push_as(&mut self, who: Who, payload: Payload) -> EventId {
        self.t += 1;
        let hlc = Hlc {
            wall_ms: self.t,
            counter: 0,
        };
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = event(self.user(who), parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).expect("insert");
        self.last = Some(id);
        id
    }

    fn state(&self) -> crate::CommunityState {
        replay_with(self.owner.id(), &self.dag, &[])
    }

    fn rejection_of(&self, id: EventId) -> Option<Rejection> {
        self.state()
            .rejected
            .iter()
            .find(|(rejected, _)| *rejected == id)
            .map(|(_, why)| *why)
    }
}

#[derive(Clone, Copy)]
enum Who {
    Owner,
    Alice,
    Bob,
}

fn publish(urls: &[&str]) -> Payload {
    Payload::RelayHostSet {
        relays: urls.iter().map(|u| (*u).to_string()).collect(),
    }
}

#[test]
fn members_may_not_publish_a_relay_by_default() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    let id = f.push_as(Who::Bob, publish(&[RELAY]));
    assert!(matches!(f.rejection_of(id), Some(Rejection::NoPermission)));
    assert!(!f.state().relay_hosts.contains_key(&bob));
}

#[test]
fn admins_and_the_owner_may_always_publish() {
    let mut f = Fixture::new();
    let (alice, owner) = (f.alice.id(), f.owner.id());
    let a = f.push_as(Who::Alice, publish(&[RELAY]));
    let o = f.push_as(Who::Owner, publish(&["https://owner.example.org"]));
    assert!(f.rejection_of(a).is_none(), "an admin may publish");
    assert!(f.rejection_of(o).is_none(), "the owner may publish");
    let state = f.state();
    assert!(state.relay_hosts.contains_key(&alice));
    assert!(state.relay_hosts.contains_key(&owner));
}

#[test]
fn the_setting_lets_members_publish() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.push_as(Who::Owner, Payload::MembersHostRelays { on: true });
    let id = f.push_as(Who::Bob, publish(&[RELAY]));
    assert!(f.rejection_of(id).is_none(), "a member may publish while the setting is on");
    assert!(f.state().relay_hosts.contains_key(&bob));
}

#[test]
fn only_admins_may_change_the_setting() {
    let mut f = Fixture::new();
    let id = f.push_as(Who::Bob, Payload::MembersHostRelays { on: true });
    assert!(matches!(f.rejection_of(id), Some(Rejection::NoPermission)));
    assert!(!f.state().members_host_relays, "the setting stays off");
}

#[test]
fn switching_the_setting_off_stops_new_publishes() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.push_as(Who::Owner, Payload::MembersHostRelays { on: true });
    f.push_as(Who::Bob, publish(&[RELAY]));
    f.push_as(Who::Owner, Payload::MembersHostRelays { on: false });
    let later = f.push_as(Who::Bob, publish(&["https://other.example.org"]));
    assert!(matches!(f.rejection_of(later), Some(Rejection::NoPermission)));
    // The list bob published while the setting was on is kept.
    let state = f.state();
    let (_, relays) = state.relay_hosts.get(&bob).expect("bob's earlier list");
    assert_eq!(relays, &vec![RELAY.to_string()]);
}

// "Seeder may forward calls" (`SeederForwardsCalls`): off by default, and only the owner changes it.
// See `docs/design/13-forwarded-calls.md`, phase 4.

#[test]
fn seeders_do_not_forward_calls_until_the_owner_allows_it() {
    let mut f = Fixture::new();
    assert!(!f.state().seeder_forwards_calls, "off by default");
    let admin = f.push_as(Who::Alice, Payload::SeederForwardsCalls { on: true });
    let member = f.push_as(Who::Bob, Payload::SeederForwardsCalls { on: true });
    assert!(matches!(f.rejection_of(admin), Some(Rejection::NoPermission)), "an admin may not allow it");
    assert!(matches!(f.rejection_of(member), Some(Rejection::NoPermission)));
    assert!(!f.state().seeder_forwards_calls);
    f.push_as(Who::Owner, Payload::SeederForwardsCalls { on: true });
    assert!(f.state().seeder_forwards_calls, "the owner allows it");
    f.push_as(Who::Owner, Payload::SeederForwardsCalls { on: false });
    assert!(!f.state().seeder_forwards_calls, "and the latest wins");
}

// `StorageGrant`: admins and the owner grant a root storage, or withdraw it; the latest per root wins.
// Phase 1 records it only. See `docs/design/05-encrypted-servers.md`, "Storage joins".

#[test]
fn admins_grant_storage_and_the_latest_grant_per_root_wins() {
    let mut f = Fixture::new();
    let seeder = User::new().id();
    let other = User::new().id();
    let member = f.push_as(Who::Bob, Payload::StorageGrant { root: seeder, granted: true });
    assert!(matches!(f.rejection_of(member), Some(Rejection::NoPermission)), "a member may not grant");
    assert!(!f.state().storage_granted(&seeder));
    let admin = f.push_as(Who::Alice, Payload::StorageGrant { root: seeder, granted: true });
    assert!(f.rejection_of(admin).is_none(), "an admin may grant");
    assert!(f.state().storage_granted(&seeder));
    assert!(!f.state().storage_granted(&other), "a grant names one root");
    f.push_as(Who::Owner, Payload::StorageGrant { root: seeder, granted: false });
    assert!(!f.state().storage_granted(&seeder), "the owner withdraws it");
}

#[test]
fn a_storage_join_does_not_need_a_grant_yet() {
    let mut f = Fixture::new();
    let seeder = User::new();
    let parents: Vec<EventId> = f.last.into_iter().collect();
    f.t += 1;
    let ev = event(&seeder, parents, Hlc { wall_ms: f.t, counter: 0 }, Payload::JoinAsStorage { display_name: "Seed".into() });
    let id = ev.id;
    f.dag.insert(ev).expect("insert");
    f.last = Some(id);
    assert!(f.rejection_of(id).is_none(), "phase 1 accepts an ungranted storage join");
    assert!(f.state().members.get(&seeder.id()).is_some_and(|m| m.storage));
}
