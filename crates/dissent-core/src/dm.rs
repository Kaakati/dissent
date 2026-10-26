//! Direct messages and the conversation list. A 1:1 DM is a sealed MLS conversation (see
//! `groups.rs`). Plaintext DMs written before sealing stay readable, and are merged into the same
//! view under the same conversation id.

use crate::engine::{now_ms, Engine};
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

/// The most unread messages one conversation counts. The rail shows 99+ past this.
pub const UNREAD_CAP: u32 = 100;

/// Unread messages: those from someone other than `me`, after `since_ms`, that are not deleted. Each
/// message is (author, time in ms, deleted). Counting stops at `UNREAD_CAP`.
pub(crate) fn count_unread(me: &RootId, since_ms: u64, messages: impl Iterator<Item = (RootId, u64, bool)>) -> u32 {
    messages
        .filter(|(from, ms, deleted)| *from != *me && *ms > since_ms && !*deleted)
        .take(UNREAD_CAP as usize)
        .count() as u32
}

/// The unread count of a conversation for this device. A 1:1 DM also counts its plaintext messages
/// from before sealing.
fn unread_of(w: &World, meta: &ConvMeta, peer: Option<RootId>) -> u32 {
    let key = match peer {
        Some(p) => format!("dm:{p}"),
        None => format!("group:{}", meta.id),
    };
    let since = crate::local::read_marker_in(w, &key).unwrap_or(0);
    let acts = w.conv_acts.get(&meta.id).map_or(&[][..], Vec::as_slice);
    let msgs = w.conv_msgs.get(&meta.id).map_or(&[][..], Vec::as_slice);
    let deleted = crate::conv_actions::deleted_messages(acts, msgs);
    let sealed = msgs.iter().map(|m| (m.from, m.hlc.wall_ms, deleted.contains(&m.id)));
    let legacy = peer
        .and_then(|p| w.dms.get(&p))
        .into_iter()
        .flatten()
        .map(|sd| (sd.dm.body.from, sd.dm.body.hlc.wall_ms, false));
    count_unread(&w.me, since, sealed.chain(legacy))
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
    let status = peer.map(|p| w.presence_of(&p, now_ms()));
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
                avatar: w.avatar_of(m),
            })
            .collect(),
        name,
        display_name,
        last_text: crate::attachments::text_of(&last_text),
        last_ms,
        status,
        avatar: peer.and_then(|p| w.avatar_of(&p)),
        live_call: None,
        contact_since_ms: peer.and_then(|p| w.contact_times.since.get(&p).copied()),
        first_seen_ms: peer.and_then(|p| w.contact_times.first_seen.get(&p).copied()),
        unread: unread_of(w, meta, peer),
    }
}

/// One summary per conversation, newest first. Plaintext DMs from peers with no sealed
/// conversation yet get a 1:1 summary too, and so does a peer whose DM call is live.
pub(crate) fn conversations(w: &World, live_peers: &[RootId]) -> Vec<ConversationSummary> {
    let mut out: Vec<ConversationSummary> = w
        .conv_book
        .convs
        .values()
        .filter(|m| !m.left)
        .map(|m| summary(w, m))
        .collect();
    let fresh = live_peers.iter().filter(|p| !w.dms.contains_key(p));
    for peer in w.dms.keys().chain(fresh) {
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
        retryable: false,
        attachments: Vec::new(),
        embeds: Vec::new(),
        reply_to: None,
        reactions: Vec::new(),
        edited: false,
        deleted: false,
    }
}

impl Engine {
    /// One summary per conversation: 1:1 DMs and group DMs, newest first.
    pub fn conversations(&self) -> Vec<ConversationSummary> {
        let live_peers = self.live_dm_peers();
        let mut list = conversations(&self.read(), &live_peers);
        for c in &mut list {
            c.live_call = if c.kind == "dm" {
                c.peer.parse().ok().and_then(|peer| self.live_call_of_dm(peer))
            } else {
                c.id.parse().ok().and_then(|conv| self.live_call_of_group(conv))
            };
        }
        list
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

#[cfg(test)]
mod tests {
    use super::*;

    fn who(byte: u8) -> RootId {
        RootId([byte; 32])
    }

    #[test]
    fn only_others_messages_after_the_marker_count() {
        let me = who(1);
        let messages = vec![
            (who(2), 10, false), // before the marker
            (who(1), 20, false), // mine
            (who(2), 30, false), // at the marker: already read
            (who(2), 40, true),  // deleted
            (who(2), 50, false),
            (who(3), 60, false),
        ];
        assert_eq!(count_unread(&me, 30, messages.clone().into_iter()), 2);
        assert_eq!(count_unread(&me, 0, messages.into_iter()), 4);
    }

    #[test]
    fn the_count_stops_at_the_cap() {
        let me = who(1);
        let messages = (1..=250u64).map(|ms| (who(2), ms, false));
        assert_eq!(count_unread(&me, 0, messages), UNREAD_CAP);
    }
}
