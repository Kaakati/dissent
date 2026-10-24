//! Direct messages and the conversation list. A 1:1 DM is a sealed MLS conversation (see
//! `groups.rs`). Plaintext DMs written before sealing stay readable, and are merged into the same
//! view under the same conversation id.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::groups::{dm_conv_id, ConvId, ConvKind, ConvMeta};
use crate::views::{ConvMember, ConversationSummary, DmItem};
use crate::world::World;
use dissent_events::{Hlc, SignedDm};
use dissent_identity::RootId;

/// The other party of a 1:1 conversation, if `meta` is one.
fn other_party(me: &RootId, meta: &ConvMeta) -> Option<RootId> {
    if meta.kind != ConvKind::Dm {
        return None;
    }
    meta.members.iter().copied().find(|m| m != me)
}

/// The summary of a conversation. The last text comes from its sealed messages, or from the
/// plaintext log when the conversation is a 1:1 DM that still has old messages.
pub(crate) fn summary(w: &World, meta: &ConvMeta) -> ConversationSummary {
    let peer = other_party(&w.me, meta);
    let mut last: Option<(Hlc, String)> = w
        .conv_msgs
        .get(&meta.id)
        .and_then(|log| log.iter().max_by_key(|m| m.hlc))
        .map(|m| (m.hlc, m.text.clone()));
    if let Some(peer) = peer {
        if let Some(sd) = w.dms.get(&peer).and_then(|log| log.iter().max_by_key(|d| d.dm.body.hlc)) {
            let newer = last.as_ref().is_none_or(|(h, _)| sd.dm.body.hlc > *h);
            if newer {
                last = Some((sd.dm.body.hlc, sd.dm.body.text.clone()));
            }
        }
    }
    let (last_text, last_ms) = match last {
        Some((hlc, text)) => (text, hlc.wall_ms),
        None => (String::new(), 0),
    };
    let (name, display_name, peer_str) = match peer {
        Some(peer) => {
            let shown = w.display_name(&peer);
            (shown.clone(), shown, peer.to_string())
        }
        None => (meta.name.clone(), meta.name.clone(), String::new()),
    };
    ConversationSummary {
        id: meta.id.to_string(),
        kind: meta.kind.as_str().to_string(),
        peer: peer_str,
        members: meta
            .members
            .iter()
            .map(|m| ConvMember {
                root_id: m.to_string(),
                display_name: w.display_name(m),
                suffix: m.suffix(),
            })
            .collect(),
        name,
        display_name,
        last_text,
        last_ms,
    }
}

/// One summary per conversation, newest first. Plaintext DMs from peers with no sealed
/// conversation yet get a 1:1 summary too.
pub(crate) fn conversations(w: &World) -> Vec<ConversationSummary> {
    let mut out: Vec<ConversationSummary> = w
        .conv_book
        .convs
        .values()
        .filter(|m| !m.left)
        .map(|m| summary(w, m))
        .collect();
    for peer in w.dms.keys() {
        let conv = dm_conv_id(&w.me, peer);
        if w.conv_book.convs.contains_key(&conv) {
            continue;
        }
        let meta = ConvMeta {
            id: conv,
            kind: ConvKind::Dm,
            creator: if w.me <= *peer { w.me } else { *peer },
            name: String::new(),
            name_hlc: Hlc::default(),
            members: {
                let mut m = vec![w.me, *peer];
                m.sort();
                m
            },
            ready: false,
            left: false,
            created_ms: 0,
        };
        out.push(summary(w, &meta));
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.last_ms));
    out
}

/// A plaintext DM from before sealing, as a message view.
pub(crate) fn legacy_item(w: &World, sd: &SignedDm) -> DmItem {
    DmItem {
        id: sd.dm.id.to_string(),
        from: sd.dm.body.from.to_string(),
        text: sd.dm.body.text.clone(),
        ms: sd.dm.body.hlc.wall_ms,
        pending: w.outbox.iter().any(|o| o.dm.id == sd.dm.id),
        delivery: if w.outbox.iter().any(|o| o.dm.id == sd.dm.id) { "sending" } else { "delivered" }.to_string(),
        queued: false,
        reason: None,
    }
}

impl Engine {
    /// One summary per conversation: 1:1 DMs and group DMs, newest first.
    pub fn conversations(&self) -> Vec<ConversationSummary> {
        conversations(&self.read())
    }

    /// The newest `limit` messages with `peer`, sealed and plaintext, oldest first.
    pub fn dm_messages(&self, peer: RootId, limit: usize) -> Vec<DmItem> {
        let conv = dm_conv_id(&self.inner.me, &peer);
        let mut items = self.conversation_messages(conv, usize::MAX);
        {
            let w = self.read();
            if let Some(log) = w.dms.get(&peer) {
                items.extend(log.iter().map(|sd| legacy_item(&w, sd)));
            }
        }
        items.sort_by_key(|m| m.ms);
        let start = items.len().saturating_sub(limit);
        items.split_off(start)
    }

    /// Sends a direct message to `peer`, sealed in their 1:1 conversation. A conversation that
    /// is not ready yet keeps the text until it is.
    pub async fn send_dm(&self, peer: RootId, text: String) -> Result<DmItem> {
        if peer == self.inner.me {
            return Err(CoreError::Invalid("cannot send a direct message to yourself".into()));
        }
        let conv = self.ensure_dm(peer)?;
        self.send_to_conversation(conv, text).await
    }

    /// Makes sure the 1:1 conversation with `peer` is known, and returns its id.
    pub(crate) fn ensure_dm(&self, peer: RootId) -> Result<ConvId> {
        let me = self.inner.me;
        let conv = dm_conv_id(&me, &peer);
        let mut guard = self.write();
        if let std::collections::btree_map::Entry::Vacant(slot) = guard.conv_book.convs.entry(conv) {
            let mut members = vec![me, peer];
            members.sort();
            slot.insert(
                ConvMeta {
                    id: conv,
                    kind: ConvKind::Dm,
                    creator: members[0],
                    name: String::new(),
                    name_hlc: Hlc::default(),
                    members,
                    ready: false,
                    left: false,
                    created_ms: crate::engine::now_ms(),
                },
            );
            guard.conv_book.save(&guard.paths)?;
        }
        Ok(conv)
    }
}
