use dissent_identity::{CommunityId, DeviceId};
use futures_lite::StreamExt;
use iroh_gossip::api::{Event, GossipReceiver, GossipSender};
use tracing::{debug, warn};

use crate::error::{NetError, Result};

/// Gossip topic id. One per channel, derived with [`TopicId::for_channel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TopicId(pub [u8; 32]);

impl TopicId {
    /// `blake3("dissent/topic/v1" || community || channel)`.
    pub fn for_channel(community: &CommunityId, channel: &[u8; 32]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"dissent/topic/v1");
        h.update(community.as_bytes());
        h.update(channel);
        Self(*h.finalize().as_bytes())
    }
}

/// A message received on a topic. `from` is the neighbour that delivered it, not the author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipMsg {
    pub from: DeviceId,
    pub bytes: Vec<u8>,
}

/// Sending half of a topic subscription.
///
/// Gossip leaves the topic once both this and the matching [`TopicReceiver`] are dropped.
/// `Node` keeps no references to either half, so dropping them is enough. Use [`Self::leave`]
/// to do that explicitly.
/// Bytes gossip adds around a payload: topic id, enum tags, message id, length prefixes. Padded
/// generously so the payload check below is conservative and never admits an oversize frame.
const GOSSIP_FRAME_OVERHEAD: usize = 256;

pub struct TopicSender {
    inner: GossipSender,
    max_payload: usize,
}

impl TopicSender {
    pub(crate) fn new(inner: GossipSender, gossip_max: usize) -> Self {
        Self { inner, max_payload: gossip_max.saturating_sub(GOSSIP_FRAME_OVERHEAD) }
    }

    /// Largest payload `broadcast` accepts, for this node's gossip limit.
    pub fn max_payload(&self) -> usize {
        self.max_payload
    }

    /// Broadcasts `bytes` to the topic. Fails with `FrameTooLarge` if the payload exceeds
    /// [`Self::max_payload`]. Nothing is sent in that case. Gossip itself would close the
    /// connection to every neighbour instead of reporting an error.
    pub async fn broadcast(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > self.max_payload {
            return Err(NetError::FrameTooLarge { len: bytes.len(), max: self.max_payload });
        }
        Ok(self.inner.broadcast(bytes.into()).await?)
    }

    /// Leaves the topic by dropping both halves. Pass the receiver from the same subscription;
    /// passing another topic's receiver leaves that topic instead.
    pub fn leave(self, rx: TopicReceiver) {
        drop(self);
        drop(rx);
    }
}

/// Receiving half of a topic subscription. Drop it, together with its sender, to leave the topic.
pub struct TopicReceiver(pub(crate) GossipReceiver);

impl TopicReceiver {
    /// Resolves once at least one neighbour is up.
    pub async fn joined(&mut self) -> Result<()> {
        Ok(self.0.joined().await?)
    }

    /// Current direct neighbours. Updated only while this receiver is polled, so keep polling
    /// with [`Self::next`] to observe neighbours going down.
    pub fn neighbors(&self) -> Vec<DeviceId> {
        self.0.neighbors().map(|id| DeviceId(*id.as_bytes())).collect()
    }

    /// Next message, or `None` once the topic is closed. Neighbour events are logged and skipped.
    pub async fn next(&mut self) -> Option<GossipMsg> {
        while let Some(event) = self.0.next().await {
            match event {
                Ok(Event::Received(m)) => {
                    return Some(GossipMsg {
                        from: DeviceId(*m.delivered_from.as_bytes()),
                        bytes: m.content.to_vec(),
                    })
                }
                Ok(Event::NeighborUp(p)) => debug!(peer = %p, "gossip neighbour up"),
                Ok(Event::NeighborDown(p)) => debug!(peer = %p, "gossip neighbour down"),
                Ok(Event::Lagged) => warn!("gossip receiver lagged, messages dropped"),
                Err(e) => {
                    debug!(error = %e, "gossip topic closed");
                    return None;
                }
            }
        }
        None
    }
}
