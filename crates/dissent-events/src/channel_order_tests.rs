//! A server's channel order: `Payload::ChannelOrder`, the latest by HLC wins, owners and admins only. See
//! `docs/design/02-communities-and-dms.md`, "Channel order".

use crate::state::{replay_with, MAX_ORDERED_CHANNELS};
use crate::testutil::{event, User};
use crate::{ChannelId, Dag, Event, EventBody, EventId, Hlc, Payload, Rejection, Replayer};
use dissent_identity::CommunityId;

struct Fixture {
    owner: User,
    alice: User,
    bob: User,
    dag: Dag,
    last: Option<EventId>,
    t: u64,
}

#[derive(Clone, Copy)]
enum Who {
    Owner,
    Alice,
    Bob,
}

impl Fixture {
    /// The owner, with alice (an admin) and bob (a plain member) joined.
    fn new() -> Self {
        let mut f = Fixture {
            owner: User::new(),
            alice: User::new(),
            bob: User::new(),
            dag: Dag::new(),
            last: None,
            t: 0,
        };
        let alice = f.alice.id();
        f.push_as(Who::Owner, Payload::Join { display_name: "owner".into() });
        f.push_as(Who::Alice, Payload::Join { display_name: "alice".into() });
        f.push_as(Who::Bob, Payload::Join { display_name: "bob".into() });
        f.push_as(Who::Owner, Payload::RoleGrant { user: alice, role: crate::Role::Admin });
        f
    }

    fn user(&self, who: Who) -> &User {
        match who {
            Who::Owner => &self.owner,
            Who::Alice => &self.alice,
            Who::Bob => &self.bob,
        }
    }

    /// The next control event by `who`, citing the last one. Not inserted.
    fn next(&mut self, who: Who, payload: Payload) -> Event {
        self.t += 1;
        let hlc = Hlc {
            wall_ms: self.t,
            counter: 0,
        };
        let parents: Vec<EventId> = self.last.into_iter().collect();
        event(self.user(who), parents, hlc, payload)
    }

    /// Adds a control event by `who`, citing the last one, and returns its id.
    fn push_as(&mut self, who: Who, payload: Payload) -> EventId {
        let ev = self.next(who, payload);
        let id = ev.id;
        self.dag.insert(ev).expect("insert");
        self.last = Some(id);
        id
    }

    fn text(&mut self, name: &str) -> ChannelId {
        ChannelId(self.push_as(Who::Owner, Payload::ChannelCreate { name: name.into() }).0)
    }

    fn voice(&mut self, name: &str) -> ChannelId {
        ChannelId(self.push_as(Who::Owner, Payload::VoiceChannelCreate { name: name.into() }).0)
    }

    fn state(&self) -> crate::CommunityState {
        replay_with(self.owner.id(), &self.dag, &[])
    }

    fn rejection_of(&self, id: EventId) -> Option<Rejection> {
        self.state().rejected.iter().find(|(r, _)| *r == id).map(|(_, why)| *why)
    }
}

fn order(text: &[ChannelId], voice: &[ChannelId]) -> Payload {
    Payload::ChannelOrder {
        text: text.to_vec(),
        voice: voice.to_vec(),
    }
}

#[test]
fn without_an_order_channels_are_listed_in_creation_order() {
    let mut f = Fixture::new();
    let (a, b, c) = (f.text("a"), f.text("b"), f.text("c"));
    let (v1, v2) = (f.voice("v1"), f.voice("v2"));
    assert_eq!(f.state().channel_order(), (vec![a, b, c], vec![v1, v2]));
}

#[test]
fn an_admins_order_is_applied() {
    let mut f = Fixture::new();
    let (a, b, c) = (f.text("a"), f.text("b"), f.text("c"));
    let (v1, v2) = (f.voice("v1"), f.voice("v2"));
    let id = f.push_as(Who::Alice, order(&[c, a, b], &[v2, v1]));
    assert_eq!(f.rejection_of(id), None);
    assert_eq!(f.state().channel_order(), (vec![c, a, b], vec![v2, v1]));
}

#[test]
fn the_latest_order_wins_whatever_order_it_arrives_in() {
    let mut f = Fixture::new();
    let (a, b) = (f.text("a"), f.text("b"));
    // Two concurrent orders on the same parents. The one with the higher HLC wins on every peer.
    let early = f.next(Who::Owner, order(&[b, a], &[]));
    let late = f.next(Who::Alice, order(&[a, b], &[]));
    for evs in [[early.clone(), late.clone()], [late.clone(), early.clone()]] {
        let mut dag = f.dag.clone();
        let mut replayer = Replayer::new(f.owner.id());
        replayer.apply_all(&dag);
        for ev in evs {
            let id = ev.id;
            dag.insert(ev).unwrap();
            replayer.on_insert(&dag, &[id]);
        }
        let full = replay_with(f.owner.id(), &dag, &[]);
        assert_eq!(full.channel_order().0, vec![a, b]);
        assert_eq!(replayer.state().channel_order().0, vec![a, b], "the incremental replay agrees");
    }
}

#[test]
fn a_members_order_is_rejected() {
    let mut f = Fixture::new();
    let (a, b) = (f.text("a"), f.text("b"));
    let id = f.push_as(Who::Bob, order(&[b, a], &[]));
    assert_eq!(f.rejection_of(id), Some(Rejection::NoPermission));
    assert_eq!(f.state().channel_order().0, vec![a, b], "the creation order stays");
}

#[test]
fn an_order_off_the_control_channel_or_too_long_is_malformed() {
    let mut f = Fixture::new();
    let a = f.text("a");
    let long = vec![a; MAX_ORDERED_CHANNELS + 1];
    let id = f.push_as(Who::Owner, order(&long, &[]));
    assert_eq!(f.rejection_of(id), Some(Rejection::Malformed));
    let mut ev = f.next(Who::Owner, order(&[a], &[]));
    ev.body.channel = a;
    let ev = Event::sign(ev.body, &f.owner.device);
    let id = ev.id;
    f.dag.insert(ev).unwrap();
    assert_eq!(f.rejection_of(id), Some(Rejection::Malformed));
}

#[test]
fn unknown_deleted_repeated_and_wrong_kind_ids_are_ignored() {
    let mut f = Fixture::new();
    let (a, b, c) = (f.text("a"), f.text("b"), f.text("c"));
    let v = f.voice("v");
    f.push_as(Who::Owner, Payload::ChannelDelete { channel: b });
    let unknown = ChannelId([42; 32]);
    let id = f.push_as(Who::Owner, order(&[unknown, c, b, v, c, a], &[a, unknown, v]));
    assert_eq!(f.rejection_of(id), None, "the ids are skipped, not the order");
    assert_eq!(f.state().channel_order(), (vec![c, a], vec![v]));
}

#[test]
fn a_channel_the_order_leaves_out_goes_at_the_end() {
    let mut f = Fixture::new();
    let (a, b, c) = (f.text("a"), f.text("b"), f.text("c"));
    f.push_as(Who::Owner, order(&[c], &[]));
    // Made after the order: listed after the ones it names, then the rest in creation order.
    let d = f.text("d");
    let w = f.voice("w");
    assert_eq!(f.state().channel_order(), (vec![c, a, b, d], vec![w]));
}

#[test]
fn a_carried_order_opens_for_members_and_is_locked_for_a_client_before_the_carrier() {
    const SECRET: [u8; 32] = [3; 32];
    let owner = User::new();
    let body = EventBody {
        community: CommunityId([0; 32]),
        channel: ChannelId::CONTROL,
        author: owner.id(),
        device: owner.device.id(),
        parents: vec![],
        hlc: Hlc { wall_ms: 1, counter: 0 },
        payload: order(&[ChannelId([1; 32])], &[]),
    };
    let sealed = crate::seal(&SECRET, 0, &body).unwrap();
    let carried = crate::carry_sealed(&sealed).unwrap();
    let ev = Event::sign(EventBody { payload: carried, ..body.clone() }, &owner.device);
    let plain = crate::open_event(&ev, &[(0, SECRET)]).unwrap();
    assert_eq!(plain.body.payload, body.payload);
    // A client from before the carrier tries the outer generation, which no secret has: locked, not failed.
    assert_eq!(crate::control::open(&[(0, SECRET)], &ev.body), Err(crate::OpenError::NoKey));
}
