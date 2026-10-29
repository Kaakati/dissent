//! `HistorySupport`: a device says it reads history messages. It counts for that device only, the latest by
//! HLC wins, and the usual bans apply. See `docs/design/05-encrypted-servers.md`, "Restarts and joins".

use crate::state::replay_with;
use crate::testutil::{on_channel, User};
use crate::{ChannelId, CommunityState, Dag, Event, EventBody, EventId, Hlc, Payload, Rejection};
use dissent_identity::{CommunityId, DeviceKey, RootId};

/// A chain of events, each citing the one before.
struct Chain {
    owner: RootId,
    dag: Dag,
    last: Option<EventId>,
    t: u64,
}

impl Chain {
    /// `owner` and `bob` joined.
    fn new(owner: &User, bob: &User) -> Self {
        let mut c = Chain { owner: owner.id(), dag: Dag::new(), last: None, t: 0 };
        c.push(owner, ChannelId::CONTROL, Payload::Join { display_name: "owner".into() });
        c.push(bob, ChannelId::CONTROL, Payload::Join { display_name: "bob".into() });
        c
    }

    fn push(&mut self, u: &User, chan: ChannelId, payload: Payload) -> EventId {
        self.t += 1;
        let hlc = Hlc { wall_ms: self.t, counter: 0 };
        let ev = on_channel(u, chan, self.last.into_iter().collect(), hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).expect("insert");
        self.last = Some(id);
        id
    }

    fn state(&self) -> CommunityState {
        replay_with(self.owner, &self.dag, &[])
    }

    fn rejection_of(&self, id: EventId) -> Option<Rejection> {
        self.state().rejected.iter().find(|(r, _)| *r == id).map(|(_, why)| *why)
    }
}

fn says(supported: bool) -> Payload {
    Payload::HistorySupport { supported }
}

#[test]
fn a_member_device_says_it_reads_history_and_only_that_device_counts() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    let id = c.push(&bob, ChannelId::CONTROL, says(true));
    assert_eq!(c.rejection_of(id), None);
    let st = c.state();
    assert!(st.declares_history(&bob.device.id()));
    // Another device of the same root has said nothing, nor has the owner.
    assert!(!st.declares_history(&DeviceKey::generate().id()));
    assert!(!st.declares_history(&owner.device.id()));
}

#[test]
fn the_latest_answer_wins() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    c.push(&bob, ChannelId::CONTROL, says(true));
    c.push(&bob, ChannelId::CONTROL, says(false));
    assert!(!c.state().declares_history(&bob.device.id()));
    c.push(&bob, ChannelId::CONTROL, says(true));
    assert!(c.state().declares_history(&bob.device.id()));
}

#[test]
fn a_banned_member_or_one_off_the_control_channel_is_refused() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    let channel = ChannelId(c.push(&owner, ChannelId::CONTROL, Payload::ChannelCreate { name: "g".into() }).0);
    let off = c.push(&bob, channel, says(true));
    assert_eq!(c.rejection_of(off), Some(Rejection::Malformed));
    c.push(&owner, ChannelId::CONTROL, Payload::Ban { user: bob.id() });
    let banned = c.push(&bob, ChannelId::CONTROL, says(true));
    assert_eq!(c.rejection_of(banned), Some(Rejection::Banned));
    assert!(!c.state().declares_history(&bob.device.id()));
}

#[test]
fn a_storage_member_may_say_it_too() {
    let (owner, bob, seeder) = (User::new(), User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    c.push(&seeder, ChannelId::CONTROL, Payload::JoinAsStorage { display_name: "seeder".into() });
    let id = c.push(&seeder, ChannelId::CONTROL, says(true));
    assert_eq!(c.rejection_of(id), None);
    assert!(c.state().declares_history(&seeder.device.id()));
}

#[test]
fn a_carried_answer_opens_for_members_and_is_locked_for_a_client_before_the_carrier() {
    const SECRET: [u8; 32] = [5; 32];
    let u = User::new();
    let body = EventBody {
        community: CommunityId([0; 32]),
        channel: ChannelId::CONTROL,
        author: u.id(),
        device: u.device.id(),
        parents: vec![],
        hlc: Hlc { wall_ms: 1, counter: 0 },
        payload: says(true),
    };
    let sealed = crate::seal(&SECRET, 0, &body).unwrap();
    let ev = Event::sign(EventBody { payload: crate::carry_sealed(&sealed).unwrap(), ..body }, &u.device);
    assert_eq!(crate::open_event(&ev, &[(0, SECRET)]).unwrap().body.payload, says(true));
    assert_eq!(crate::control::open(&[(0, SECRET)], &ev.body), Err(crate::OpenError::NoKey));
}
