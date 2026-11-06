//! `ServerFileSizeLimit`: the largest file members may share in a server. Managers set it, the latest by HLC wins,
//! and only 1 MiB to 1 TiB is accepted.

use crate::state::replay_with;
use crate::testutil::{on_channel, User};
use crate::{ChannelId, CommunityState, Dag, EventId, Hlc, Payload, Rejection, DEFAULT_FILE_SIZE_LIMIT, MAX_FILE_SIZE_LIMIT, MIN_FILE_SIZE_LIMIT};
use dissent_identity::RootId;

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

fn limit(bytes: u64) -> Payload {
    Payload::ServerFileSizeLimit { bytes }
}

#[test]
fn a_server_starts_at_the_default_and_the_latest_limit_wins() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    assert_eq!(c.state().max_file_size(), DEFAULT_FILE_SIZE_LIMIT);
    c.push(&owner, ChannelId::CONTROL, limit(2 << 30));
    assert_eq!(c.state().max_file_size(), 2 << 30);
    // Above the old 10 GiB cap is fine: only the format bounds it.
    c.push(&owner, ChannelId::CONTROL, limit(500 << 30));
    assert_eq!(c.state().max_file_size(), 500 << 30);
}

#[test]
fn only_managers_set_it_and_only_on_the_control_channel() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    let by_member = c.push(&bob, ChannelId::CONTROL, limit(5 << 20));
    assert_eq!(c.rejection_of(by_member), Some(Rejection::NoPermission));
    c.push(&owner, ChannelId::CONTROL, Payload::RoleGrant { user: bob.id(), role: crate::Role::Admin });
    let by_admin = c.push(&bob, ChannelId::CONTROL, limit(5 << 20));
    assert_eq!(c.rejection_of(by_admin), None);
    assert_eq!(c.state().max_file_size(), 5 << 20);
    let channel = ChannelId(c.push(&owner, ChannelId::CONTROL, Payload::ChannelCreate { name: "g".into() }).0);
    let off = c.push(&owner, channel, limit(7 << 20));
    assert_eq!(c.rejection_of(off), Some(Rejection::Malformed));
    assert_eq!(c.state().max_file_size(), 5 << 20);
}

#[test]
fn a_limit_outside_one_mib_to_one_tib_is_malformed() {
    let (owner, bob) = (User::new(), User::new());
    let mut c = Chain::new(&owner, &bob);
    for bad in [0, MIN_FILE_SIZE_LIMIT - 1, MAX_FILE_SIZE_LIMIT + 1, u64::MAX] {
        let id = c.push(&owner, ChannelId::CONTROL, limit(bad));
        assert_eq!(c.rejection_of(id), Some(Rejection::Malformed), "{bad}");
    }
    for good in [MIN_FILE_SIZE_LIMIT, MAX_FILE_SIZE_LIMIT] {
        let id = c.push(&owner, ChannelId::CONTROL, limit(good));
        assert_eq!(c.rejection_of(id), None, "{good}");
    }
    assert_eq!(c.state().max_file_size(), MAX_FILE_SIZE_LIMIT);
}
