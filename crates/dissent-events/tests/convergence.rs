//! Convergence fuzz: the same events in any arrival order must replay to identical state.

use dissent_events::{
    replay, ChannelId, CommunityState, Dag, Event, EventBody, EventId, Hlc, InsertOutcome, Payload,
    Rejection, Replayer, Role,
};
use dissent_identity::{CommunityId, CommunityKey, DeviceCert, DeviceKey, RootKey};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use std::collections::BTreeMap;

const OWNER: usize = 0;
const ADMIN: usize = 1;
const MOD: usize = 2;
const USER: usize = 3;

struct Member {
    root: RootKey,
    device: DeviceKey,
    cert: DeviceCert,
}

struct Builder {
    community: CommunityId,
    members: Vec<Member>,
    events: Vec<Event>,
    hlcs: BTreeMap<EventId, Hlc>,
}

impl Builder {
    fn new() -> Self {
        let members = (0..4)
            .map(|_| {
                let root = RootKey::generate();
                let device = DeviceKey::generate();
                let cert = root.certify(device.id(), 0);
                Member { root, device, cert }
            })
            .collect();
        Self {
            community: CommunityKey::generate().id(),
            members,
            events: Vec::new(),
            hlcs: BTreeMap::new(),
        }
    }

    fn owner(&self) -> dissent_identity::RootId {
        self.members[OWNER].root.id()
    }

    fn push(
        &mut self,
        who: usize,
        chan: ChannelId,
        parents: Vec<EventId>,
        wall: u64,
        payload: Payload,
    ) -> EventId {
        // Stamp above every parent so the causal-HLC rule holds.
        let floor = parents
            .iter()
            .filter_map(|p| self.hlcs.get(p).copied())
            .max()
            .unwrap_or_default();
        let hlc = if wall > floor.wall_ms {
            Hlc {
                wall_ms: wall,
                counter: 0,
            }
        } else {
            Hlc {
                wall_ms: floor.wall_ms,
                counter: floor.counter + 1,
            }
        };
        let m = &self.members[who];
        let body = EventBody {
            community: self.community,
            channel: chan,
            author: m.root.id(),
            device: m.device.id(),
            parents,
            hlc,
            payload,
        };
        let ev = Event::sign(body, &m.device);
        let id = ev.id;
        self.hlcs.insert(id, hlc);
        self.events.push(ev);
        id
    }

    fn user_id(&self, who: usize) -> dissent_identity::RootId {
        self.members[who].root.id()
    }
}

/// Scripted conflicts plus seeded random filler. About 40 events total.
fn build() -> Builder {
    let mut b = Builder::new();
    let mut rng = StdRng::seed_from_u64(0xD155E17);
    let ctl = ChannelId::CONTROL;

    // Setup. Grants and the admin's channel create are concurrent (both parent `g`).
    let g = b.push(
        OWNER,
        ctl,
        vec![],
        1,
        Payload::ChannelCreate {
            name: "general".into(),
        },
    );
    let general = ChannelId(g.0);
    b.push(
        OWNER,
        ctl,
        vec![g],
        10,
        Payload::RoleGrant {
            user: b.user_id(ADMIN),
            role: Role::Admin,
        },
    );
    let grant_mod = b.push(
        OWNER,
        ctl,
        vec![g],
        11,
        Payload::RoleGrant {
            user: b.user_id(MOD),
            role: Role::Moderator,
        },
    );
    let dev = ChannelId(
        b.push(
            ADMIN,
            ctl,
            vec![g],
            25,
            Payload::ChannelCreate { name: "dev".into() },
        )
        .0,
    );

    // Ban by a moderator, concurrent with three messages from the banned user.
    let ban = b.push(
        MOD,
        ctl,
        vec![grant_mod],
        1000,
        Payload::Ban {
            user: b.user_id(USER),
        },
    );
    for i in 0..3 {
        b.push(
            USER,
            general,
            vec![g],
            2000 + i,
            Payload::Message {
                text: format!("late {i}"),
            },
        );
    }

    // Two concurrent handle claims for the same handle.
    b.push(
        OWNER,
        ctl,
        vec![g],
        30,
        Payload::HandleClaim {
            handle: "Alice".into(),
        },
    );
    b.push(
        ADMIN,
        ctl,
        vec![g],
        31,
        Payload::HandleClaim {
            handle: "alice ".into(),
        },
    );

    // Join, Leave, Join for one user, all concurrent (parent `g`). Then a Join from the
    // banned user at wall 1200, which sorts after the ban and is rejected.
    b.push(
        USER,
        ctl,
        vec![g],
        40,
        Payload::Join {
            display_name: "u1".into(),
        },
    );
    b.push(USER, ctl, vec![g], 41, Payload::Leave);
    b.push(
        USER,
        ctl,
        vec![g],
        42,
        Payload::Join {
            display_name: "u2".into(),
        },
    );
    b.push(
        USER,
        ctl,
        vec![g],
        1200,
        Payload::Join {
            display_name: "u3".into(),
        },
    );

    // Random filler. Parents never include the ban, so the scripted conflicts stay concurrent.
    let mut channels = vec![general, dev];
    let mut messages: Vec<(EventId, ChannelId)> = Vec::new();
    let mut candidates: Vec<EventId> = b
        .events
        .iter()
        .map(|e| e.id)
        .filter(|id| *id != ban)
        .collect();
    while b.events.len() < 40 {
        let n_parents = rng.random_range(0..=2usize);
        let parents: Vec<EventId> = (0..n_parents)
            .map(|_| candidates[rng.random_range(0..candidates.len())])
            .collect();
        let wall = rng.random_range(0..3000u64);
        let author = rng.random_range(0..4usize);
        let id = match rng.random_range(0..10u8) {
            0 => {
                let name = format!("c{}", b.events.len());
                let id = b.push(OWNER, ctl, parents, wall, Payload::ChannelCreate { name });
                channels.push(ChannelId(id.0));
                id
            }
            1 => {
                // Never grant the banned user a role: a staff member cannot ban staff.
                let who = rng.random_range(1..3usize);
                let role = if rng.random_bool(0.5) {
                    Role::Admin
                } else {
                    Role::Moderator
                };
                b.push(
                    OWNER,
                    ctl,
                    parents,
                    wall,
                    Payload::RoleGrant {
                        user: b.user_id(who),
                        role,
                    },
                )
            }
            2 => {
                // Revoking the moderator or unbanning the user would erase the scripted conflict.
                let role = if rng.random_bool(0.5) {
                    Role::Admin
                } else {
                    Role::Moderator
                };
                b.push(
                    ADMIN,
                    ctl,
                    parents,
                    wall,
                    Payload::RoleRevoke {
                        user: b.user_id(ADMIN),
                        role,
                    },
                )
            }
            3 => {
                let who = rng.random_range(0..4usize);
                b.push(
                    MOD,
                    ctl,
                    parents,
                    wall,
                    Payload::Ban {
                        user: b.user_id(who),
                    },
                )
            }
            4 => b.push(
                OWNER,
                ctl,
                parents,
                wall,
                Payload::Unban {
                    user: b.user_id(ADMIN),
                },
            ),
            5 => {
                let handle =
                    ["Alice", "bob", "BOB ", "carol"][rng.random_range(0..4usize)].to_string();
                b.push(author, ctl, parents, wall, Payload::HandleClaim { handle })
            }
            6 if !messages.is_empty() => {
                let (target, chan) = messages[rng.random_range(0..messages.len())];
                b.push(
                    author,
                    chan,
                    parents,
                    wall,
                    Payload::Edit {
                        target,
                        text: "edited".into(),
                    },
                )
            }
            7 if !messages.is_empty() => {
                let (target, chan) = messages[rng.random_range(0..messages.len())];
                b.push(author, chan, parents, wall, Payload::Delete { target })
            }
            8 => b.push(author, ctl, parents, wall, Payload::Leave),
            9 => b.push(
                author,
                ctl,
                parents,
                wall,
                Payload::Join {
                    display_name: format!("n{author}"),
                },
            ),
            _ => {
                let chan = channels[rng.random_range(0..channels.len())];
                let id = b.push(
                    author,
                    chan,
                    parents,
                    wall,
                    Payload::Message {
                        text: format!("f{}", b.events.len()),
                    },
                );
                messages.push((id, chan));
                id
            }
        };
        candidates.push(id);
    }
    b
}

/// Insert in the given order, asserting every event is accepted and nothing is left pending.
fn dag_from(events: &[Event]) -> Dag {
    let mut dag = Dag::new();
    for ev in events {
        dag.insert(ev.clone()).unwrap();
    }
    assert!(dag.missing().is_empty());
    assert_eq!(dag.len(), events.len());
    dag
}

#[test]
fn replay_converges_across_200_shuffles() {
    let b = build();
    assert_eq!(b.events.len(), 40);
    for ev in &b.events {
        let author = b
            .members
            .iter()
            .find(|m| m.root.id() == ev.body.author)
            .unwrap();
        ev.verify(&author.cert).unwrap();
    }

    let owner = b.owner();
    let reference: CommunityState = replay(owner, &dag_from(&b.events));
    assert!(
        reference
            .rejected
            .iter()
            .any(|(_, r)| *r == Rejection::Banned),
        "no Banned rejection exercised"
    );
    assert!(
        reference
            .rejected
            .iter()
            .any(|(_, r)| *r == Rejection::HandleTaken),
        "no HandleTaken rejection exercised"
    );
    let reference_bytes = postcard::to_allocvec(&reference).unwrap();

    for seed in 0..200u64 {
        let mut shuffled = b.events.clone();
        shuffled.shuffle(&mut StdRng::seed_from_u64(seed));
        let st = replay(owner, &dag_from(&shuffled));
        assert_eq!(
            postcard::to_allocvec(&st).unwrap(),
            reference_bytes,
            "divergence at seed {seed}"
        );
    }
}

fn bytes(st: &CommunityState) -> Vec<u8> {
    postcard::to_allocvec(st).unwrap()
}

/// Feed events one at a time, as a peer receiving them off the wire would.
fn feed(order: &[Event], owner: dissent_identity::RootId) -> (Replayer, Dag) {
    let mut dag = Dag::new();
    let mut rep = Replayer::new(owner);
    for ev in order {
        if let InsertOutcome::Inserted { entered } = dag.insert(ev.clone()).unwrap() {
            rep.on_insert(&dag, &entered);
        }
    }
    (rep, dag)
}

#[test]
fn replayer_converges_across_200_shuffles() {
    let b = build();
    let owner = b.owner();
    let reference = bytes(&replay(owner, &dag_from(&b.events)));

    let mut rewinds = 0;
    for seed in 0..200u64 {
        let mut shuffled = b.events.clone();
        shuffled.shuffle(&mut StdRng::seed_from_u64(seed));
        let (rep, dag) = feed(&shuffled, owner);
        assert_eq!(dag.len(), b.events.len());
        assert_eq!(bytes(rep.state()), reference, "divergence at seed {seed}");
        rewinds += rep.stats().rewinds;
    }
    assert!(rewinds > 0, "shuffles never forced a rewind");
}

/// Long linear history. Every 200th event also has a side branch forking 49 events
/// back. That branch sorts among the recent tail, so its arrival forces a rewind
/// near the tip, which can restore from a snapshot.
fn long_chain(n: usize) -> Builder {
    let mut b = Builder::new();
    let g = b.push(
        OWNER,
        ChannelId::CONTROL,
        vec![],
        1,
        Payload::ChannelCreate {
            name: "general".into(),
        },
    );
    let chan = ChannelId(g.0);
    let mut chain: Vec<EventId> = Vec::new();
    let mut prev = g;
    for c in 0..n {
        let who = [OWNER, ADMIN, MOD, USER][c % 4];
        prev = b.push(
            who,
            chan,
            vec![prev],
            2 + c as u64,
            Payload::Message {
                text: format!("m{c}"),
            },
        );
        chain.push(prev);
        if c >= 50 && c % 200 == 199 {
            b.push(
                USER,
                chan,
                vec![chain[c - 49]],
                2 + (c - 48) as u64,
                Payload::Message {
                    text: format!("side {c}"),
                },
            );
        }
    }
    b
}

#[test]
fn replayer_long_history_uses_snapshots() {
    let b = long_chain(2600);
    assert!(b.events.len() > 2600);
    let owner = b.owner();
    let reference = bytes(&replay(owner, &dag_from(&b.events)));

    for seed in 1..=3u64 {
        // Displace each event by up to 6 places, so most arrivals are in order.
        let mut rng = StdRng::seed_from_u64(seed);
        let mut order: Vec<usize> = (0..b.events.len()).collect();
        for i in 0..order.len() {
            let j = (i + rng.random_range(0..=6usize)).min(order.len() - 1);
            order.swap(i, j);
        }
        let arrivals: Vec<Event> = order.iter().map(|&i| b.events[i].clone()).collect();
        let (rep, dag) = feed(&arrivals, owner);

        assert_eq!(dag.len(), b.events.len());
        assert_eq!(bytes(rep.state()), reference, "divergence at seed {seed}");
        let stats = rep.stats();
        assert!(stats.snapshots_taken > 0, "no snapshots taken: {stats:?}");
        assert!(
            stats.snapshot_restores > 0,
            "no rewind used a snapshot: {stats:?}"
        );
    }
}
