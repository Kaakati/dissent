//! Cascaded forwarders for calls of 100 participants and more.
//!
//! The forwarding tree has two levels of forwarders, so a frame crosses at most three of them:
//! speaker's child, root, listener's child. The root and its standby come from the same election
//! as a single forwarder, so every peer computes the same tree from the same candidates.

use std::collections::BTreeMap;

use crate::election::{ranked, Candidate, Mode, PeerId};

/// The forwarding tree: a root, a standby root, and the child forwarders under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cascade {
    /// The root forwarder. Every child sends its top speakers up to it.
    pub root: PeerId,
    /// The root's standby. Children are linked to it already and take over if the root fails.
    pub standby_root: PeerId,
    /// The child forwarders, in rank order. Each serves a share of the participants.
    pub children: Vec<PeerId>,
}

/// Chooses the root, its standby, and `child_count` children from the candidates.
///
/// The root must carry one stream per child, and each child must carry its own share of the
/// participants plus a sibling's share, because it takes over a failed sibling's participants.
/// Returns `None` when there are too few candidates for that.
pub fn plan(
    candidates: &[Candidate],
    participants: usize,
    child_count: usize,
    mode: Mode,
) -> Option<Cascade> {
    if child_count < 2 {
        return None;
    }
    let root_order = ranked(candidates, child_count, mode);
    let root = *root_order.first()?;
    let standby_root = *root_order.get(1)?;

    let share = participants.div_ceil(child_count);
    let rest: Vec<Candidate> = candidates
        .iter()
        .filter(|c| c.peer != root && c.peer != standby_root)
        .cloned()
        .collect();
    let children_order = ranked(&rest, 2 * share, mode);
    if children_order.len() < child_count {
        return None;
    }
    Some(Cascade {
        root,
        standby_root,
        children: children_order[..child_count].to_vec(),
    })
}

/// Gives each participant a primary child and a standby child, round robin in ascending id.
/// Participant `i` gets child `i` and the next child after it, so when a child fails, its
/// participants fall to a child that already holds a share.
pub fn assign_forwarders(
    participants: &[PeerId],
    children: &[PeerId],
) -> BTreeMap<PeerId, (PeerId, PeerId)> {
    let mut ids = participants.to_vec();
    ids.sort_unstable();
    let count = children.len();
    ids.iter()
        .enumerate()
        .map(|(index, participant)| {
            (
                *participant,
                (children[index % count], children[(index + 1) % count]),
            )
        })
        .collect()
}

/// How many participants each child is assigned as primary, and as standby.
pub fn load(assignment: &BTreeMap<PeerId, (PeerId, PeerId)>) -> BTreeMap<PeerId, (usize, usize)> {
    let mut load: BTreeMap<PeerId, (usize, usize)> = BTreeMap::new();
    for (primary, standby) in assignment.values() {
        load.entry(*primary).or_default().0 += 1;
        load.entry(*standby).or_default().1 += 1;
    }
    load
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::election::capacity;

    fn forwarder(peer: PeerId, upload_kbps: u32) -> Candidate {
        Candidate {
            peer,
            upload_kbps,
            on_battery: false,
            wired: true,
            cpu_ok: true,
            rtt_ms_median: 20,
        }
    }

    #[test]
    fn plan_picks_a_root_standby_and_children_by_rank() {
        let mut set = vec![forwarder(10, 40_000), forwarder(11, 40_000)];
        set.extend((20..25).map(|p| forwarder(p, 20_000)));
        let tree = plan(&set, 150, 5, Mode::AudioTop3).unwrap();
        assert_eq!(tree.root, 10);
        assert_eq!(tree.standby_root, 11);
        assert_eq!(tree.children, vec![20, 21, 22, 23, 24]);
    }

    #[test]
    fn plan_refuses_when_children_cannot_carry_a_sibling_share() {
        // 150 participants over 5 children is 30 each, and 60 with a sibling's share. 60 listeners
        // need 5760 kbps, which 4000 kbps cannot carry.
        let mut set = vec![forwarder(10, 40_000), forwarder(11, 40_000)];
        set.extend((20..25).map(|p| forwarder(p, 4_000)));
        assert!(plan(&set, 150, 5, Mode::AudioTop3).is_none());
    }

    #[test]
    fn plan_needs_at_least_two_children() {
        let set = vec![
            forwarder(10, 40_000),
            forwarder(11, 40_000),
            forwarder(20, 40_000),
        ];
        assert!(plan(&set, 10, 1, Mode::AudioTop3).is_none());
    }

    #[test]
    fn each_child_has_capacity_for_its_share_and_a_sibling_share() {
        let children: Vec<PeerId> = (20..25).collect();
        let assignment = assign_forwarders(&(1..=150).collect::<Vec<_>>(), &children);
        let load = load(&assignment);
        for child in &children {
            let (primary, standby) = load[child];
            assert_eq!(primary, 30);
            assert_eq!(standby, 30);
            assert!(capacity(20_000, Mode::AudioTop3) >= primary + standby);
        }
    }

    #[test]
    fn assignment_is_deterministic_and_pairs_neighbours() {
        let children = [20u64, 21, 22];
        let a = assign_forwarders(&[3, 1, 2, 4], &children);
        let b = assign_forwarders(&[4, 2, 3, 1], &children);
        assert_eq!(a, b, "the input order does not matter");
        assert_eq!(a[&1], (20, 21));
        assert_eq!(a[&2], (21, 22));
        assert_eq!(a[&3], (22, 20));
        assert_eq!(a[&4], (20, 21));
    }
}
