//! When each contact was added, and when this device first saw each person. Kept in their own file,
//! so the contact cards and the contact book keep their stored shape.

use std::collections::BTreeMap;

use dissent_identity::RootId;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::{read_bin, write_bin, Paths};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ContactTimes {
    /// When each current contact was added or accepted, in milliseconds. Unknown for contacts added
    /// before this file existed.
    pub since: BTreeMap<RootId, u64>,
    /// The earliest time this device saw each person: a contact, or a conversation with them.
    pub first_seen: BTreeMap<RootId, u64>,
}

impl ContactTimes {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(read_bin(&paths.contact_times())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_bin(&paths.contact_times(), self)
    }

    /// Records that `root` became a contact at `ms`. Adding them again restarts the date.
    pub fn became_contact(&mut self, root: RootId, ms: u64) {
        self.since.insert(root, ms);
        self.note_seen(root, ms);
    }

    /// Records that `root` is no longer a contact. Their first-seen time stays.
    pub fn stopped_contact(&mut self, root: &RootId) {
        self.since.remove(root);
    }

    /// Keeps the earliest time this device saw `root`.
    pub fn note_seen(&mut self, root: RootId, ms: u64) {
        self.first_seen
            .entry(root)
            .and_modify(|seen| *seen = (*seen).min(ms))
            .or_insert(ms);
    }
}
