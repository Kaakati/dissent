//! An authenticated call through a forwarder with honest participants. The digests must pass
//! through the forwarder for every frame to verify.

mod common;

use std::time::Duration;

use common::{auth_set, memory_pair, run_participant, MemoryTransport};
use dissent_media::{Candidate, MediaKey, Mode, Participant, Sfu};
use tokio::time::timeout;

const FORWARDER: u64 = 100;
const SPEAKER_LEVEL: u8 = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn digests_pass_through_the_forwarder_and_every_frame_verifies() {
    let ids = [1u64, 2, 3];
    let auth = auth_set(&ids);
    let key = MediaKey::new([7; 32]);
    let candidates = vec![Candidate {
        peer: FORWARDER,
        upload_kbps: 20_000,
        on_battery: false,
        wired: true,
        cpu_ok: true,
        rtt_ms_median: 5,
    }];

    let mut sfu: Sfu<MemoryTransport> = Sfu::new(Mode::AudioTop3);
    let mut participants: Vec<Participant<MemoryTransport>> = ids
        .iter()
        .map(|id| {
            Participant::new_authenticated(
                key.clone(),
                *id,
                candidates.clone(),
                ids.len(),
                Mode::AudioTop3,
                auth[id].clone(),
            )
            .unwrap()
        })
        .collect();
    for participant in participants.iter_mut() {
        let (participant_end, forwarder_end) = memory_pair();
        let offer = participant
            .offer_sfu(FORWARDER, participant_end)
            .await
            .unwrap();
        let answer = sfu
            .accept(participant.id(), forwarder_end, offer)
            .await
            .unwrap();
        participant.apply_sfu(FORWARDER, answer).unwrap();
        participant.elect().unwrap();
    }

    let forwarder = tokio::spawn(async move { sfu.run().await });
    let mut it = participants.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    let results = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_participant(a, 1, SPEAKER_LEVEL, Duration::from_millis(1500)),
            run_participant(b, 2, SPEAKER_LEVEL, Duration::from_millis(1500)),
            run_participant(c, 3, SPEAKER_LEVEL, Duration::from_millis(1500)),
        )
    })
    .await
    .expect("the call did not finish in time");
    forwarder.abort();

    for (me, (participant, arrivals)) in ids.iter().copied().zip([results.0, results.1, results.2])
    {
        assert!(
            arrivals.iter().all(|a| a.intact),
            "peer {me} got an altered frame"
        );
        let others: Vec<u64> = ids.iter().copied().filter(|id| *id != me).collect();
        let senders: std::collections::BTreeSet<u64> = arrivals.iter().map(|a| a.sender).collect();
        assert_eq!(senders.into_iter().collect::<Vec<_>>(), others, "peer {me}");
        assert!(!arrivals.is_empty());

        let mut unauthenticated = 0;
        let mut impersonations = 0;
        let mut replayed = 0;
        for stats in participant.link_stats().values() {
            unauthenticated += stats.frames_unauthenticated;
            impersonations += stats.impersonation_detected;
            replayed += stats.frames_replayed;
            assert_eq!(stats.frames_undecryptable, 0, "peer {me}");
        }
        assert_eq!(
            unauthenticated, 0,
            "peer {me}: honest frames left unauthenticated"
        );
        assert_eq!(impersonations, 0, "peer {me}");
        assert_eq!(replayed, 0, "peer {me}");
    }
}
