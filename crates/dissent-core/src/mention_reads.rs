//! When each channel's mentions were last read on this device. A channel's mention count counts the
//! mentioning messages newer than this time. Kept in their own file, so the other stores keep their
//! stored shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{now_ms, Engine};
use crate::error::Result;
use crate::store::Paths;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct MentionReads {
    /// Milliseconds at which a channel's mentions were last marked read, by `community/channel`.
    pub at: BTreeMap<String, u64>,
}

impl MentionReads {
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(paths.read_bin(&paths.mention_reads())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.write_bin(&paths.mention_reads(), self)
    }
}

/// The key a channel's read time is stored under.
pub(crate) fn key(community: &str, channel: &str) -> String {
    format!("{community}/{channel}")
}

impl Engine {
    /// Marks a channel's mentions read now. Its mention count drops to zero.
    pub fn mark_mentions_read(&self, community: dissent_identity::CommunityId, channel: dissent_events::ChannelId) -> Result<()> {
        let mut w = self.write();
        w.mention_reads.at.insert(key(&community.to_string(), &channel.to_string()), now_ms());
        w.mention_reads.save(&w.paths)
    }
}
