use super::*;
use dissent_events::{replay, ChannelId, Event, EventBody};
use dissent_identity::{CommunityKey, Identity};

struct Founder {
    identity: Identity,
    key: CommunityKey,
    genesis: Genesis,
}

fn founder() -> Founder {
    let identity = Identity::create(1);
    let key = CommunityKey::generate();
    let genesis = Genesis::create(&key, identity.root_id(), "test".into(), 1);
    Founder {
        identity,
        key,
        genesis,
    }
}

fn signed(
    identity: &Identity,
    community: dissent_identity::CommunityId,
    parents: Vec<EventId>,
    wall: u64,
    payload: Payload,
) -> SignedEvent {
    let body = EventBody {
        community,
        channel: ChannelId::CONTROL,
        author: identity.root_id(),
        device: identity.device_id(),
        parents,
        hlc: Hlc {
            wall_ms: wall,
            counter: 0,
        },
        payload,
    };
    let event = Event::sign(body, &identity.device);
    SignedEvent {
        event,
        cert: identity.cert.clone(),
    }
}

#[test]
fn out_of_order_batch_stores_all_and_stalls_only_when_parents_missing() {
    let f = founder();
    let c_id = f.key.id();
    let join = signed(
        &f.identity,
        c_id,
        vec![],
        1,
        Payload::Join {
            display_name: "A".into(),
        },
    );
    let leave = signed(&f.identity, c_id, vec![join.event.id], 2, Payload::Leave);

    let mut c = Community::new(f.genesis.clone());
    let batch = c.ingest(vec![leave.clone()]);
    assert!(batch.stalled);
    assert!(batch.stored.is_empty());

    let batch = c.ingest(vec![join.clone()]);
    assert!(!batch.stalled);
    assert_eq!(batch.stored.len(), 2, "the waiting child is released too");
    assert!(!c.is_member(&f.identity.root_id()), "Leave applied");
    assert_eq!(c.signed.len(), 2);
    assert_eq!(c.heads(), vec![leave.event.id], "the released child is the only head");
}

#[test]
fn duplicate_events_are_ignored() {
    let f = founder();
    let join = signed(
        &f.identity,
        f.key.id(),
        vec![],
        1,
        Payload::Join {
            display_name: "A".into(),
        },
    );
    let mut c = Community::new(f.genesis.clone());
    assert_eq!(c.ingest(vec![join.clone()]).stored.len(), 1);
    assert!(c.ingest(vec![join]).stored.is_empty());
}

#[test]
fn author_rejects_without_changing_state() {
    let f = founder();
    let c_id = f.key.id();
    let mut c = Community::new(f.genesis.clone());
    // A stranger holds no role, so creating a channel is refused.
    let stranger = Identity::create(2);
    let ch = signed(
        &stranger,
        c_id,
        vec![],
        1,
        Payload::ChannelCreate { name: "x".into() },
    );
    let err = c.author(ch.clone()).unwrap_err();
    assert!(matches!(err, CoreError::Rejected(ref r) if r == "NoPermission"), "{err:?}");
    assert!(c.signed.is_empty());
    assert!(c.state().channels.is_empty());

    // The owner is allowed to join.
    let join = signed(
        &f.identity,
        c_id,
        vec![],
        2,
        Payload::Join {
            display_name: "A".into(),
        },
    );
    c.author(join).unwrap();
    assert!(c.is_member(&f.identity.root_id()));
}

#[test]
fn incremental_state_matches_full_replay() {
    let f = founder();
    let c_id = f.key.id();
    let mut c = Community::new(f.genesis.clone());
    let join = signed(
        &f.identity,
        c_id,
        vec![],
        1,
        Payload::Join {
            display_name: "A".into(),
        },
    );
    c.ingest(vec![join.clone()]);
    let mut prev = join.event.id;
    for wall in 2..40u64 {
        let next = signed(
            &f.identity,
            c_id,
            vec![prev],
            wall,
            Payload::Join {
                display_name: format!("name{wall}"),
            },
        );
        prev = next.event.id;
        c.ingest(vec![next]);
    }
    // A late event that sorts before the applied tail forces a rewind.
    let late = signed(
        &f.identity,
        c_id,
        vec![join.event.id],
        2,
        Payload::Join {
            display_name: "late".into(),
        },
    );
    c.ingest(vec![late]);
    let full = replay(f.genesis.owner, &c.dag);
    assert_eq!(c.state(), &full);
    assert_eq!(c.max_hlc(), Some(Hlc { wall_ms: 39, counter: 0 }));
}
