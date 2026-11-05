//! Mesh calls over in-memory links. Every test uses synthetic addresses, not sockets.

mod common;

use std::time::Duration;

use common::{assert_from_each, frame, memory_pair, run_mesh_peer, seq_of, MemoryTransport, FRAMES};
use dissent_media::{MediaKey, Mesh, Signal};
use tokio::time::timeout;

type Peer = Mesh<MemoryTransport>;

/// How long a call that has not connected counts as still connecting, in these tests.
const CONNECTING: Duration = Duration::from_secs(10);

fn key(byte: u8) -> MediaKey {
    MediaKey::new([byte; 32])
}

/// Connects every pair of `ids` with an offer and an answer over in-memory links. The lower id
/// offers, so no pair ever produces two offers.
async fn full_mesh(ids: &[u64], media_key: MediaKey) -> Vec<Peer> {
    let mut meshes: Vec<Peer> = ids
        .iter()
        .map(|id| Mesh::new(media_key.clone(), *id).unwrap())
        .collect();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            let (caller_end, callee_end) = memory_pair();
            let (lower, upper) = meshes.split_at_mut(j);
            let (offer, token) = lower[i].offer(ids[j], caller_end).await.unwrap();
            let answer = upper[0].accept(ids[i], callee_end, offer).await.unwrap();
            lower[i].apply(ids[j], token, answer).unwrap();
        }
    }
    meshes
}

#[tokio::test]
async fn four_peer_mesh_delivers_every_frame_to_every_other_peer() {
    let ids = [1u64, 2, 3, 4];
    let mut it = full_mesh(&ids, key(1)).await.into_iter();
    let (a, b, c, d) = (
        it.next().unwrap(),
        it.next().unwrap(),
        it.next().unwrap(),
        it.next().unwrap(),
    );

    let each = 3 * FRAMES as usize;
    let (ra, rb, rc, rd) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_mesh_peer(a, FRAMES, each),
            run_mesh_peer(b, FRAMES, each),
            run_mesh_peer(c, FRAMES, each),
            run_mesh_peer(d, FRAMES, each),
        )
    })
    .await
    .expect("the mesh did not finish in time");

    for (me, (mesh, received)) in ids.into_iter().zip([ra, rb, rc, rd]) {
        let others: Vec<u64> = ids.iter().copied().filter(|id| *id != me).collect();
        assert_from_each(&received, &others, FRAMES);
        for (peer, stats) in mesh.stats() {
            assert_eq!(stats.frames_sent, FRAMES, "peer {me} to {peer}");
            assert_eq!(stats.frames_received, FRAMES, "peer {me} from {peer}");
            assert_eq!(stats.frames_undecryptable, 0, "peer {me} from {peer}");
        }
    }
}

#[tokio::test]
async fn removing_a_peer_leaves_the_rest_of_the_mesh_working() {
    let ids = [1u64, 2, 3, 4];
    let mut meshes = full_mesh(&ids, key(1)).await;
    // Peer 4 leaves. Its own mesh stays alive but is no longer driven.
    let _left_peer = meshes.pop().unwrap();
    for mesh in meshes.iter_mut() {
        assert!(mesh.remove_peer(4));
    }

    let mut it = meshes.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    let each = 2 * FRAMES as usize;
    let (ra, rb, rc) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_mesh_peer(a, FRAMES, each),
            run_mesh_peer(b, FRAMES, each),
            run_mesh_peer(c, FRAMES, each),
        )
    })
    .await
    .expect("the mesh did not finish in time");

    let trio = [1u64, 2, 3];
    for (me, (mesh, received)) in trio.into_iter().zip([ra, rb, rc]) {
        let others: Vec<u64> = trio.iter().copied().filter(|id| *id != me).collect();
        assert_eq!(
            mesh.peers(),
            others,
            "peer {me} still lists the removed peer"
        );
        assert_from_each(&received, &others, FRAMES);
    }
}

#[tokio::test]
async fn removed_peer_cannot_decrypt_frames_after_a_rekey() {
    let ids = [1u64, 2, 3];
    let mut it = full_mesh(&ids, key(1)).await.into_iter();
    let (one, two, three) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());

    // Warm-up under epoch 0: every peer hears the other two.
    let warm = 10u64;
    let ((mut one, _), (mut two, _), (three, _)) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_mesh_peer(one, warm, 2 * warm as usize),
            run_mesh_peer(two, warm, 2 * warm as usize),
            run_mesh_peer(three, warm, 2 * warm as usize),
        )
    })
    .await
    .expect("warm-up did not finish in time");

    // Peer 3 is banned. Peer 1 drops its call, and peers 1 and 2 rekey to epoch 1 without it.
    // Peer 2 never learns of the ban, so its link to peer 3 stays up, and it keeps sending to
    // peer 3 under the new epoch.
    assert!(one.remove_peer(3));
    one.set_key(key(2), 1).unwrap();
    two.set_key(key(2), 1).unwrap();
    assert_eq!(one.epoch(), 1);
    assert_eq!(three.epoch(), 0, "the removed peer never gets the new key");

    let fresh = 50u64;
    let (one_done, _two_done, spy_done) = timeout(Duration::from_secs(60), async {
        tokio::join!(
            run_mesh_peer(one, 0, fresh as usize),
            run_mesh_peer(two, fresh, 0),
            spy(three, Duration::from_secs(2)),
        )
    })
    .await
    .expect("the rekey test did not finish in time");

    // Peer 1 hears every fresh frame from peer 2, decrypted under the new epoch.
    let (_one, one_frames) = one_done;
    assert_from_each(&one_frames, &[2], fresh);

    // The removed peer decrypted nothing new. Every fresh frame counts as undecryptable.
    let (three, delivered) = spy_done;
    assert!(
        delivered.is_empty(),
        "the removed peer decrypted {} frames",
        delivered.len()
    );
    let from_two = three.stats()[&2];
    assert_eq!(
        from_two.frames_undecryptable, fresh,
        "fresh frames must fail for the removed peer"
    );
    assert_eq!(
        from_two.frames_received, warm,
        "only warm-up frames decrypted, under epoch 0"
    );
}

/// Keeps a removed peer running for `window` without letting it send. It records any frame it
/// manages to decrypt.
async fn spy(mut mesh: Peer, window: Duration) -> (Peer, Vec<(u64, Vec<u8>)>) {
    let mut delivered = Vec::new();
    let _ = timeout(window, async {
        while let Ok(Some(item)) = mesh.next_frame().await {
            delivered.push(item);
        }
    })
    .await;
    (mesh, delivered)
}

#[tokio::test]
async fn a_peer_still_connecting_does_not_silence_the_mesh_toward_the_others() {
    let mut a: Peer = Mesh::new(key(1), 1).unwrap();
    let mut b: Peer = Mesh::new(key(1), 2).unwrap();
    let (a_end, b_end) = memory_pair();
    let (offer, token) = a.offer(2, a_end).await.unwrap();
    let answer = b.accept(1, b_end, offer).await.unwrap();
    a.apply(2, token, answer).unwrap();
    // Peer 3 is offered a call it never answers, so that call stays unconnected.
    let (to_3, _never_answered) = memory_pair();
    a.offer(3, to_3).await.unwrap();

    let received = timeout(Duration::from_secs(30), async {
        let sender = async {
            let mut sent = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_millis(20));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        // Before the call to 2 connects there is no one to send to; after, 3 is skipped.
                        if a.send_audio_frame(&frame(sent)).await.is_ok() {
                            sent += 1;
                        }
                    }
                    next = a.next_frame() => { next.unwrap(); }
                }
            }
        };
        let receiver = async {
            let mut got = Vec::new();
            while got.len() < FRAMES as usize {
                let (from, payload) = b.next_frame().await.unwrap().expect("the mesh emptied");
                assert_eq!(from, 1);
                got.push(seq_of(&payload));
            }
            got
        };
        tokio::select! {
            _ = sender => unreachable!("the sender runs until the receiver is done"),
            got = receiver => got,
        }
    })
    .await
    .expect("peer 2 heard nothing while the call to 3 was still connecting");
    assert_eq!(received, (0..FRAMES).collect::<Vec<_>>(), "every frame sent reached 2, in order");
    assert!(a.stats().contains_key(&3), "the unconnected call stays in the mesh");
}

/// A second offer for a peer whose call works is refused, so two offers that cross, or a duplicate, cannot drop the
/// call. A call that has gone quiet is replaced, so a peer back from another room can link again.
#[tokio::test]
async fn a_working_call_is_kept_and_a_quiet_one_is_replaced() {
    let ids = [1u64, 2];
    let mut it = full_mesh(&ids, key(1)).await.into_iter();
    let (a, b) = (it.next().unwrap(), it.next().unwrap());
    let ((mut a, _), (mut b, _)) = timeout(Duration::from_secs(60), async {
        tokio::join!(run_mesh_peer(a, FRAMES, FRAMES as usize), run_mesh_peer(b, FRAMES, FRAMES as usize))
    })
    .await
    .expect("the call did not carry frames in time");
    assert!(b.replace_if_stale(1, Duration::from_secs(3), CONNECTING).is_err(), "a working call is kept");
    assert_eq!(b.peers(), vec![1], "and is still there");
    assert!(a.replace_if_stale(2, Duration::ZERO, CONNECTING).is_ok(), "a call quiet for longer than allowed is replaced");
    assert!(a.peers().is_empty());
}

/// A call still in its handshake is not stale: a second offer for that peer is refused until the call is older than
/// the connect grace, so it cannot orphan the answer on its way.
#[tokio::test]
async fn a_call_still_connecting_is_kept_until_its_grace_runs_out() {
    let mut a: Peer = Mesh::new(key(1), 1).unwrap();
    let (to_2, _unanswered) = memory_pair();
    a.offer(2, to_2).await.unwrap();
    let err = a.replace_if_stale(2, Duration::from_secs(3), CONNECTING).unwrap_err();
    assert!(err.to_string().contains("still connecting"), "{err}");
    assert_eq!(a.peers(), vec![2], "the connecting call stays");
    assert!(a.replace_if_stale(2, Duration::from_secs(3), Duration::ZERO).is_ok(), "past its grace it is replaced");
    assert!(a.peers().is_empty());
}

/// Two offers to one peer cross: the first call is replaced before its answer comes back. That answer is refused and
/// leaves the second call alone, and the second call connects with its own answer.
#[tokio::test]
async fn an_answer_to_a_replaced_offer_is_refused_and_the_new_call_connects() {
    let mut a: Peer = Mesh::new(key(1), 1).unwrap();
    let mut b: Peer = Mesh::new(key(1), 2).unwrap();
    let (a_end, b_end) = memory_pair();
    let (first, first_token) = a.offer(2, a_end).await.unwrap();
    let first_answer = b.accept(1, b_end, first).await.unwrap();

    a.replace_if_stale(2, Duration::from_secs(3), Duration::ZERO).unwrap();
    b.replace_if_stale(1, Duration::from_secs(3), Duration::ZERO).unwrap();
    let (a_end, b_end) = memory_pair();
    let (second, second_token) = a.offer(2, a_end).await.unwrap();
    assert_ne!(first_token, second_token);
    let second_answer = b.accept(1, b_end, second).await.unwrap();

    let err = a.apply(2, first_token, first_answer).unwrap_err();
    assert!(err.to_string().contains("superseded"), "{err}");
    a.apply(2, second_token, second_answer).expect("the second call takes its own answer");
    assert!(a.apply(2, second_token, Signal::Answer(String::new())).is_err(), "an answer is applied once");

    let ((_, from_b), (_, from_a)) = timeout(Duration::from_secs(60), async {
        tokio::join!(run_mesh_peer(a, FRAMES, FRAMES as usize), run_mesh_peer(b, FRAMES, FRAMES as usize))
    })
    .await
    .expect("the second call did not carry frames in time");
    assert_from_each(&from_b, &[2], FRAMES);
    assert_from_each(&from_a, &[1], FRAMES);
}
