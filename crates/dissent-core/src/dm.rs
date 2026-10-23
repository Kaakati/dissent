//! Direct messages: conversation views, and sending with an outbox fallback.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::store::append_record;
use crate::views::{ConversationSummary, DmItem, EngineEvent};
use crate::world::World;
use dissent_events::{DmBody, DmMessage, SignedDm};
use dissent_identity::RootId;

/// One summary per conversation, newest first.
pub(crate) fn conversations(w: &World) -> Vec<ConversationSummary> {
    let mut out: Vec<ConversationSummary> = w
        .dms
        .iter()
        .filter_map(|(peer, log)| {
            let last = log.iter().max_by_key(|d| d.dm.body.hlc)?;
            Some(ConversationSummary {
                peer: peer.to_string(),
                display_name: w.display_name(peer),
                last_text: last.dm.body.text.clone(),
                last_ms: last.dm.body.hlc.wall_ms,
            })
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.last_ms));
    out
}

/// The newest `limit` messages with `peer`, oldest first.
pub(crate) fn messages(w: &World, peer: &RootId, limit: usize) -> Vec<DmItem> {
    let Some(log) = w.dms.get(peer) else {
        return Vec::new();
    };
    let mut sorted: Vec<&SignedDm> = log.iter().collect();
    sorted.sort_by_key(|d| d.dm.body.hlc);
    let start = sorted.len().saturating_sub(limit);
    sorted[start..].iter().map(|d| item(w, d)).collect()
}

pub(crate) fn item(w: &World, sd: &SignedDm) -> DmItem {
    DmItem {
        id: sd.dm.id.to_string(),
        from: sd.dm.body.from.to_string(),
        text: sd.dm.body.text.clone(),
        ms: sd.dm.body.hlc.wall_ms,
        pending: w.outbox.iter().any(|o| o.dm.id == sd.dm.id),
    }
}

impl Engine {
    pub fn conversations(&self) -> Vec<ConversationSummary> {
        conversations(&self.read())
    }

    pub fn dm_messages(&self, peer: RootId, limit: usize) -> Vec<DmItem> {
        messages(&self.read(), &peer, limit)
    }

    /// Sends a direct message. Delivery is tried on every known device of `peer`. If none
    /// acknowledges, the message waits in the outbox and is retried periodically.
    pub async fn send_dm(&self, peer: RootId, text: String) -> Result<DmItem> {
        if peer == self.inner.me {
            return Err(CoreError::Invalid("cannot send a direct message to yourself".into()));
        }
        let text = crate::engine::clean_text(&text, "message")?;
        let sd = {
            let mut guard = self.write();
            let body = DmBody {
                from: self.inner.me,
                to: peer,
                device: self.inner.identity.device_id(),
                hlc: self.stamp(),
                text,
            };
            let dm = DmMessage::sign(body, &self.inner.identity.device);
            let sd = SignedDm {
                dm,
                cert: self.inner.identity.cert.clone(),
            };
            append_record(&guard.paths.dm_log(&peer), &sd)?;
            guard.dms.entry(peer).or_default().push(sd.clone());
            sd
        };

        if !self.deliver(peer, &sd).await {
            let mut guard = self.write();
            guard.outbox.push(sd.clone());
            guard.save_outbox()?;
        }
        self.emit(EngineEvent::Dm {
            peer: peer.to_string(),
        });
        Ok(item(&self.read(), &sd))
    }
}
