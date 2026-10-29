//! Letting go of blobs nothing refers to any more (`docs/design/06-threat-model.md`, "Blob fetch: who gets a file's
//! ciphertext").
//!
//! A blob stays in the store while it is pinned. `Files::add` pins what this device shares (files, thumbnails,
//! preview pictures, avatars), and `Files::mirror` pins what it keeps for others. Nothing unpinned them, so a deleted
//! message's file was kept, and served to members, for as long as the device ran.
//!
//! Now a blob is unpinned once no live reference to it is left on this device: a message that is not deleted, in a
//! server we are in or a conversation we have not left; a manifest of such a message; a picture in use, of a server
//! we are in or of someone we still share something with; a hint a seeder accepted. The store's garbage collection
//! then removes its bytes on its next run (`dissent_files::DEFAULT_GC_INTERVAL`).
//!
//! When it runs:
//! - A deleted message, ours or arriving from its author: its blobs are the candidates.
//! - A left server or conversation, and a removed contact: the blobs they named are the candidates.
//! - At start, every pinned blob is a candidate, which picks up whatever changed while the device was off. A file
//!   shared in the last day is kept then, so a message still on its way out never loses its file.

use std::collections::HashSet;
use std::time::Duration;

use dissent_events::{EventId, Payload};
use dissent_identity::{CommunityId, RootId};
use tracing::debug;

use crate::blob_gate::message_blobs;
use crate::engine::{now_ms, Engine};
use crate::groups::ConvId;
use crate::world::World;

/// How long a file this device shared is kept by the start-up sweep, whatever refers to it.
const FRESH_UPLOAD: Duration = Duration::from_secs(24 * 60 * 60);

/// The blobs a community message names: those in its text, and its manifest's, for a device that cannot read it.
pub(crate) fn message_blobs_in(c: &crate::community::Community, message: EventId) -> Vec<[u8; 32]> {
    let mut out = c.content.get(&message).map(|(_, text)| message_blobs(text)).unwrap_or_default();
    for se in c.signed_for(&[message]) {
        if let Payload::Sealed { ciphertext, .. } = &se.event.body.payload {
            out.extend(crate::attachments::split_manifest(ciphertext).0.into_iter().map(|e| e.hash));
        }
    }
    out
}

impl World {
    /// Every blob something live on this device refers to. See the module documentation.
    pub(crate) fn referenced_blobs(&self) -> HashSet<[u8; 32]> {
        let mut live = HashSet::new();
        for c in self.communities.values() {
            let s = c.state();
            let deleted: HashSet<EventId> = s.messages.values().flatten().filter(|m| m.deleted).map(|m| m.id).collect();
            for (id, (_, text)) in &c.content {
                if !deleted.contains(id) {
                    live.extend(message_blobs(text));
                }
            }
            for se in c.sealed_in_order() {
                if deleted.contains(&se.event.id) {
                    continue;
                }
                if let Payload::Sealed { ciphertext, .. } = &se.event.body.payload {
                    live.extend(crate::attachments::split_manifest(ciphertext).0.into_iter().map(|e| e.hash));
                }
            }
        }
        for (conv, msgs) in &self.conv_msgs {
            if self.conv_book.convs.get(conv).is_some_and(|meta| meta.left) {
                continue;
            }
            let acts = self.conv_acts.get(conv).map_or(&[][..], Vec::as_slice);
            let deleted = crate::conv_actions::deleted_messages(acts, msgs);
            for m in msgs.iter().filter(|m| !deleted.contains(&m.id)) {
                live.extend(message_blobs(&m.text));
            }
        }
        // Messages not stored yet: bodies waiting for their conversation's group.
        for body in &self.conv_book.unsent {
            if let crate::groups::Action::Text(text) | crate::groups::Action::Reply { text, .. } = &body.action {
                live.extend(message_blobs(text));
            }
        }
        live.extend(self.pictures_in_use());
        live.extend(self.blob_scopes.hinted_blobs());
        live
    }

    /// Pictures this device shows: its own, its servers', and those of people it still shares something with.
    fn pictures_in_use(&self) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        out.extend(self.own_avatar.map(|p| p.hash));
        out.extend(self.banners.own.iter().flat_map(|b| b.pictures()).map(|p| p.hash));
        for (root, (_, picture)) in &self.peer_avatars {
            if self.still_known(root) {
                out.extend(picture.map(|p| p.hash));
            }
        }
        for (root, (_, banner)) in &self.banners.peers {
            if self.still_known(root) {
                out.extend(banner.iter().flat_map(|b| b.pictures()).map(|p| p.hash));
            }
        }
        for c in self.communities.values() {
            let s = c.state();
            out.extend(s.avatars.values().filter_map(|(_, p)| p.map(|p| p.hash)));
            out.extend(s.profile_banners.values().flat_map(|(_, b)| b.iter().flat_map(|b| b.pictures())).map(|p| p.hash));
            out.extend(s.icon.and_then(|(_, p)| p).map(|p| p.hash));
            out.extend(s.animated_icon.and_then(|(_, i)| i).into_iter().flat_map(|i| [i.image.hash, i.poster.hash]));
            out.extend(s.banner.and_then(|(_, b)| b).iter().flat_map(|b| b.pictures()).map(|p| p.hash));
        }
        out
    }

    /// Whether `root` is a contact or shares a server or a conversation with us, so its direct pictures are shown.
    fn still_known(&self, root: &RootId) -> bool {
        self.is_contact(root)
            || self.communities.values().any(|c| c.is_member(root))
            || self.conv_book.convs.values().any(|m| !m.left && m.members.contains(root))
    }

    /// The blobs a community message names: those in its text, and its manifest's, for a device that cannot read it.
    pub(crate) fn blobs_of_community_message(&self, community: CommunityId, message: EventId) -> Vec<[u8; 32]> {
        self.communities.get(&community).map(|c| message_blobs_in(c, message)).unwrap_or_default()
    }

    /// The blobs a conversation message names.
    pub(crate) fn blobs_of_conv_message(&self, conv: ConvId, message: [u8; 32]) -> Vec<[u8; 32]> {
        self.conv_msgs
            .get(&conv)
            .and_then(|msgs| msgs.iter().find(|m| m.id == message))
            .map(|m| message_blobs(&m.text))
            .unwrap_or_default()
    }

    /// Every blob a server's content, manifests and pictures name, for when we leave it.
    pub(crate) fn blobs_of_community(&self, community: CommunityId) -> Vec<[u8; 32]> {
        let Some(c) = self.communities.get(&community) else {
            return Vec::new();
        };
        let mut out: Vec<[u8; 32]> = c.content.values().flat_map(|(_, text)| message_blobs(text)).collect();
        for se in c.sealed_in_order() {
            if let Payload::Sealed { ciphertext, .. } = &se.event.body.payload {
                out.extend(crate::attachments::split_manifest(ciphertext).0.into_iter().map(|e| e.hash));
            }
        }
        let s = c.state();
        out.extend(s.avatars.values().filter_map(|(_, p)| p.map(|p| p.hash)));
        out.extend(s.profile_banners.values().flat_map(|(_, b)| b.iter().flat_map(|b| b.pictures())).map(|p| p.hash));
        out.extend(s.icon.and_then(|(_, p)| p).map(|p| p.hash));
        out.extend(s.banner.and_then(|(_, b)| b).iter().flat_map(|b| b.pictures()).map(|p| p.hash));
        out
    }

    /// Every blob a conversation's messages name, for when we leave it.
    pub(crate) fn blobs_of_conv(&self, conv: ConvId) -> Vec<[u8; 32]> {
        self.conv_msgs.get(&conv).into_iter().flatten().flat_map(|m| message_blobs(&m.text)).collect()
    }

    /// A person's direct pictures, for when they stop being a contact.
    pub(crate) fn pictures_of(&self, root: &RootId) -> Vec<[u8; 32]> {
        let avatar = self.peer_avatars.get(root).and_then(|(_, p)| p.map(|p| p.hash));
        let banner = self.banners.peers.get(root).into_iter().flat_map(|(_, b)| b.iter().flat_map(|b| b.pictures()));
        avatar.into_iter().chain(banner.map(|p| p.hash)).collect()
    }
}

impl Engine {
    /// Unpins those of `candidates` that nothing live refers to any more, in the background. Their bytes go with the
    /// store's next garbage collection.
    pub(crate) fn release_blobs(&self, candidates: Vec<[u8; 32]>) {
        if candidates.is_empty() {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            engine.unpin_unreferenced(candidates, false).await;
        });
    }

    /// Unpins every pinned blob nothing refers to. Run once at start, so deletions and departures that happened while
    /// this device was off are picked up. Files shared in the last day are kept.
    pub(crate) fn release_unreferenced_at_start(&self) {
        let engine = self.clone();
        self.spawn_task(async move {
            match engine.inner.files.pinned_hashes().await {
                Ok(pinned) => {
                    engine.unpin_unreferenced(pinned, true).await;
                }
                Err(e) => debug!(error = %e, "could not list pinned blobs"),
            }
        });
    }

    /// Unpins the candidates that are not referenced, and forgets them as mirrored. Returns how many were unpinned.
    pub(crate) async fn unpin_unreferenced(&self, candidates: Vec<[u8; 32]>, keep_fresh_uploads: bool) -> usize {
        let release: Vec<[u8; 32]> = {
            let w = self.read();
            let mut live = w.referenced_blobs();
            if keep_fresh_uploads {
                let since = now_ms().saturating_sub(FRESH_UPLOAD.as_millis() as u64);
                live.extend(
                    w.attach.transfers.values().filter(|t| t.direction == "upload" && t.started_ms >= since).map(|t| t.file.hash),
                );
            }
            let mut seen = HashSet::new();
            candidates.into_iter().filter(|h| !live.contains(h) && seen.insert(*h)).collect()
        };
        let mut unpinned = 0;
        for hash in &release {
            match self.inner.files.unpin(*hash).await {
                Ok(true) => unpinned += 1,
                Ok(false) => {}
                Err(e) => debug!(error = %e, "could not unpin a blob"),
            }
        }
        if !release.is_empty() {
            let mut guard = self.write();
            for hash in &release {
                let id = data_encoding::BASE32_NOPAD.encode(hash).to_lowercase();
                guard.attach.mirrored.remove(&id);
            }
            if let Err(e) = guard.attach.save(&guard.paths) {
                debug!(error = %e, "could not save the attachment book");
            }
        }
        unpinned
    }
}
