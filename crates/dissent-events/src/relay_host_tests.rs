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
