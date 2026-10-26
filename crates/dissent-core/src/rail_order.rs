//! The order of the server rail on this device. It is this user's own preference, so it lives in a
//! local file beside `local.bin` and is never sent to anyone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dissent_identity::CommunityId;

use crate::engine::Engine;
use crate::error::Result;
use crate::store::{read_bin, write_bin};
use crate::views::CommunitySummary;
use crate::world::World;

fn path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("rail_order.bin")
}

/// The saved order. A missing or unreadable file gives no order, so the default one is used.
fn read_order(path: &Path) -> Vec<CommunityId> {
    read_bin(path).ok().flatten().unwrap_or_default()
}

/// The saved order of this device, for the community list.
pub(crate) fn saved_order(w: &World) -> Vec<CommunityId> {
    read_order(&path(w))
}

/// The ids to save: only communities still joined, each once, in the order given.
fn kept(ids: Vec<CommunityId>, joined: &BTreeSet<CommunityId>) -> Vec<CommunityId> {
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| joined.contains(id) && seen.insert(*id))
        .collect()
}

/// Puts the communities in the saved order. A community that is not saved keeps its place after
/// the saved ones, so a newly joined one comes last in the default order.
pub(crate) fn apply(saved: &[CommunityId], out: &mut [CommunitySummary]) {
    let rank = |id: &str| {
        saved
            .iter()
            .position(|s| s.to_string() == id)
            .unwrap_or(usize::MAX)
    };
    out.sort_by_key(|c| rank(&c.id));
}

impl Engine {
    /// Saves the order of the server rail on this device. Ids this device no longer belongs to are
    /// dropped.
    pub fn set_server_order(&self, ids: Vec<CommunityId>) -> Result<()> {
        let joined: BTreeSet<CommunityId> = self.read().communities.keys().copied().collect();
        let path = path(&self.read());
        let _serial = self.write();
        write_bin(&path, &kept(ids, &joined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> CommunityId {
        CommunityId([byte; 32])
    }

    fn summary(byte: u8, name: &str) -> CommunitySummary {
        CommunitySummary {
            id: id(byte).to_string(),
            name: name.into(),
            owner: String::new(),
            my_role: "member".into(),
            icon: None,
            mentions: 0,
            can_mention_everyone: false,
        }
    }

    /// The communities in name order, as the community list gives them before the saved order.
    fn by_name() -> Vec<CommunitySummary> {
        vec![summary(1, "Alpha"), summary(2, "Bravo"), summary(3, "Charlie")]
    }

    fn names(out: &[CommunitySummary]) -> Vec<&str> {
        out.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_saved_order_comes_first() {
        let mut out = by_name();
        apply(&[id(3), id(1)], &mut out);
        assert_eq!(names(&out), ["Charlie", "Alpha", "Bravo"]);
    }

    #[test]
    fn a_community_not_saved_is_appended_in_its_default_order() {
        // Bravo was joined after the order was saved, so it is not in the saved list.
        let mut out = by_name();
        apply(&[id(3)], &mut out);
        assert_eq!(names(&out), ["Charlie", "Alpha", "Bravo"]);
    }

    #[test]
    fn a_saved_id_with_no_community_is_ignored() {
        let mut out = by_name();
        apply(&[id(9), id(2)], &mut out);
        assert_eq!(names(&out), ["Bravo", "Alpha", "Charlie"]);
    }

    #[test]
    fn a_save_keeps_only_joined_communities_once() {
        let joined: BTreeSet<CommunityId> = [id(1), id(2)].into_iter().collect();
        assert_eq!(kept(vec![id(2), id(9), id(1), id(2)], &joined), vec![id(2), id(1)]);
    }

    #[test]
    fn a_corrupt_or_missing_file_means_the_default_order() {
        let dir = std::env::temp_dir().join(format!("dissent-rail-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rail_order.bin");
        assert!(read_order(&file).is_empty(), "a missing file gives no order");
        std::fs::write(&file, [0xff, 0xff, 0xff]).unwrap();
        assert!(read_order(&file).is_empty(), "a corrupt file gives no order");
        write_bin(&file, &vec![id(3), id(1)]).unwrap();
        assert_eq!(read_order(&file), vec![id(3), id(1)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
