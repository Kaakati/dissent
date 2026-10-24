//! An authenticated mesh with honest members. Every frame is verified, and nothing is flagged.

mod common;

use std::time::Duration;

use common::{assert_from_each, auth_set, memory_pair, run_mesh_peer, MemoryTransport, FRAMES};
use dissent_media::{MediaKey, Mesh};
use tokio::time::timeout;

#[tokio::test]
async fn honest_authenticated_mesh_verifies_every_frame() {
    let ids = [1u64, 2, 3];
    let auth = auth_set(&ids);
    let media_key = MediaKey::new([6; 32]);
    let mut meshes: Vec<Mesh<MemoryTransport>> = ids
        .iter()
        .map(|id| Mesh::new_authenticated(media_key.clone(), *id, auth[id].clone()).unwrap())
        .collect();

    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            let (caller_end, callee_end) = memory_pair();
            let (lower, upper) = meshes.split_at_mut(j);
            let offer = lower[i].offer(ids[j], caller_end).await.unwrap();
            let answer = upper[0].accept(ids[i], callee_end, offer).await.unwrap();
            lower[i].apply(ids[j], answer).unwrap();
        }
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

    for (me, (mesh, received)) in ids.iter().copied().zip([ra, rb, rc]) {
        let others: Vec<u64> = ids.iter().copied().filter(|id| *id != me).collect();
        assert_from_each(&received, &others, FRAMES);
        for (peer, stats) in mesh.stats() {
            assert_eq!(stats.frames_unauthenticated, 0, "peer {me} from {peer}");
            assert_eq!(stats.impersonation_detected, 0, "peer {me} from {peer}");
            assert_eq!(stats.frames_replayed, 0, "peer {me} from {peer}");
            assert_eq!(stats.frames_undecryptable, 0, "peer {me} from {peer}");
        }
    }
}
