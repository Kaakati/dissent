//! A leaf built on the epoch before everyone else's. Leaves of a new tier all start at key id 0 with the group's
//! key, so a device one commit behind starts with the old key at id 0, and its rekey then moves it to the commit's id
//! while the others stay on id 0 with that same key. Both sides must still hear each other.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::{auth_set, memory_pair, run_participant, MemoryTransport};
use dissent_media::{Candidate, MediaKey, Mode, Participant, Sfu};
use tokio::time::timeout;

const FORWARDER: u64 = 100;
const LEVEL: u8 = 10;
const STALE: u64 = 3;
/// The stale device's rekey: the MLS epoch of the commit it was behind on.
const EPOCH: u64 = 6;
/// Past the grace period of the key the stale device started with, so a key kept for it cannot be what works.
const RUN: Duration = Duration::from_millis(4_000);

/// Builds the call. `STALE` starts on the older key and rekeys to the current one at `EPOCH`; the ones in `released`
/// have no key fallback, as a client from before it.
async fn call(ids: &[u64], released: &[u64]) -> (Vec<Participant<MemoryTransport>>, tokio::task::JoinHandle<()>) {
    let auth = auth_set(ids);
    let (older, current) = (MediaKey::new([5; 32]), MediaKey::new([6; 32]));
    let candidates = vec![Candidate {
        peer: FORWARDER,
        upload_kbps: 20_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 5,
    }];
    let mut sfu: Sfu<MemoryTransport> = Sfu::new(Mode::AudioTop3);
    let mut participants = Vec::new();
    for id in ids.iter().copied() {
        let key = if id == STALE { older.clone() } else { current.clone() };
        let mut participant: Participant<MemoryTransport> =
            Participant::new_authenticated(key, id, candidates.clone(), ids.len(), Mode::AudioTop3, auth[&id].clone()).unwrap();
        let (participant_end, forwarder_end) = memory_pair();
        let offer = participant.offer_sfu(FORWARDER, participant_end).await.unwrap();
        let answer = sfu.accept(id, forwarder_end, offer).await.unwrap();
        participant.apply_sfu(FORWARDER, answer).unwrap();
        participant.elect().unwrap();
        if released.contains(&id) {
            participant.without_key_fallback();
        }
        if id == STALE {
            participant.set_key(current.clone(), EPOCH).unwrap();
        }
        participants.push(participant);
    }
    let forwarder = tokio::spawn(async move {
        let _ = sfu.run().await;
    });
    (participants, forwarder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_leaf_that_rekeyed_after_the_move_hears_and_is_heard() {
    let ids = [1u64, 2, STALE];
    let (participants, forwarder) = call(&ids, &[]).await;
    let mut it = participants.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    let results = timeout(Duration::from_secs(60), async {
        tokio::join!(run_participant(a, 1, LEVEL, RUN), run_participant(b, 2, LEVEL, RUN), run_participant(c, STALE, LEVEL, RUN))
    })
    .await
    .expect("the call did not finish in time");
    forwarder.abort();

    for (me, (participant, arrivals)) in ids.iter().copied().zip([results.0, results.1, results.2]) {
        let others: BTreeSet<u64> = ids.iter().copied().filter(|id| *id != me).collect();
        let heard: BTreeSet<u64> = arrivals.iter().map(|a| a.sender).collect();
        let stats: Vec<_> = participant.link_stats().into_values().collect();
        assert_eq!(heard, others, "peer {me} hears everyone: {stats:?}");
        assert!(arrivals.iter().all(|a| a.intact), "peer {me} got an altered frame");
        for s in &stats {
            assert_eq!(s.impersonation_detected, 0, "peer {me} muted someone: {s:?}");
            assert_eq!(s.frames_unauthenticated, 0, "peer {me} left honest frames unverified: {s:?}");
            assert_eq!(s.frames_undecryptable, 0, "peer {me} failed a frame: {s:?}");
        }
    }
}

/// With a released client in the call: the repaired device hears it, and it still does not hear the repaired device.
/// A device with the fallback hears everyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_client_still_cannot_hear_the_device_that_rekeyed() {
    const RELEASED: u64 = 1;
    let ids = [RELEASED, 2, STALE];
    let (participants, forwarder) = call(&ids, &[RELEASED]).await;
    let mut it = participants.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    let results = timeout(Duration::from_secs(60), async {
        tokio::join!(run_participant(a, RELEASED, LEVEL, RUN), run_participant(b, 2, LEVEL, RUN), run_participant(c, STALE, LEVEL, RUN))
    })
    .await
    .expect("the call did not finish in time");
    forwarder.abort();
    let heard = |arrivals: &[common::Arrival]| arrivals.iter().map(|a| a.sender).collect::<BTreeSet<u64>>();
    assert_eq!(heard(&results.0 .1), BTreeSet::from([2]), "the released client misses only the device that rekeyed");
    assert_eq!(heard(&results.1 .1), BTreeSet::from([RELEASED, STALE]), "a new client hears everyone");
    assert_eq!(heard(&results.2 .1), BTreeSet::from([RELEASED, 2]), "the repaired device hears everyone");
}

/// The next commit reaches everyone: all move to one id and one key, and nothing is left of the adopted id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_rekey_brings_everyone_back_to_one_id() {
    let ids = [1u64, 2, STALE];
    let (participants, forwarder) = call(&ids, &[]).await;
    let next = MediaKey::new([7; 32]);
    let mut outcome = Vec::new();
    let tasks: Vec<_> = participants
        .into_iter()
        .map(|mut p| {
            let next = next.clone();
            tokio::spawn(async move {
                let id = p.id();
                let (p2, _) = run_participant(p, id, LEVEL, Duration::from_millis(600)).await;
                p = p2;
                p.set_key(next, EPOCH + 1).unwrap();
                run_participant(p, id, LEVEL, Duration::from_millis(1_500)).await
            })
        })
        .collect();
    for task in tasks {
        outcome.push(timeout(Duration::from_secs(60), task).await.expect("in time").unwrap());
    }
    forwarder.abort();
    for (me, (participant, arrivals)) in ids.iter().copied().zip(outcome) {
        let others: BTreeSet<u64> = ids.iter().copied().filter(|id| *id != me).collect();
        let heard: BTreeSet<u64> = arrivals.iter().map(|a| a.sender).collect();
        assert_eq!(heard, others, "peer {me} hears everyone after the rekey");
        for s in participant.link_stats().values() {
            assert_eq!(s.impersonation_detected, 0, "peer {me}: {s:?}");
        }
    }
}
