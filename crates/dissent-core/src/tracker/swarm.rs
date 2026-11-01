//! The swarm: who holds each listing, and receipts. Holdings are not events. Each device asks the online member devices
//! of a server what they hold (`Request::Holdings`), at most every `HOLDINGS_EVERY`, and keeps the answers for
//! `HOLDINGS_KEPT`. A device answers only active persons and granted storage members of that server. See
//! `docs/design/14-file-tracker.md`, "Swarm state: holdings".

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use dissent_events::{ChannelId, EventId, Payload};
use dissent_identity::{CommunityId, DeviceId, RootId};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::catalog::TRANSFER_PREFIX;
use crate::engine::{now_ms, Engine};
use crate::membership::CommitRules;
use crate::rpc::{Request, Response};

/// How often a device asks the others for their holdings, at most.
pub(crate) const HOLDINGS_EVERY: Duration = Duration::from_secs(30);
/// How long a device's last answer counts toward a listing's seeders.
const HOLDINGS_KEPT_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// How long a partial holding counts as a leecher.
const LEECHING_MS: u64 = 3 * 60 * 1000;
/// How long this device reuses its own holdings answer.
const HOLDINGS_CACHE: Duration = Duration::from_secs(10);
/// How often an open file page asks for holdings, at most, while its file moves. A device's answer is at most
/// `HOLDINGS_CACHE` old, so asking faster would learn nothing.
const HOLDINGS_EVERY_MOVING: Duration = Duration::from_secs(10);
/// How long one device has to answer.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// One blob a device holds: its hash and the ciphertext bytes it has. Whole when that equals the listing's size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Holding {
    pub blob: [u8; 32],
    pub bytes: u64,
}

/// What one device said it holds, and when.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct HeardHoldings {
    pub root: Option<RootId>,
    pub at_ms: u64,
    pub digest: [u8; 32],
    pub holdings: Vec<Holding>,
}

/// The swarm of one listing, as this device knows it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Swarm {
    pub seeders: BTreeSet<RootId>,
    pub leechers: BTreeSet<RootId>,
    /// Devices that hold the whole blob, freshest first, for a download to ask.
    pub devices: Vec<DeviceId>,
    /// Devices that hold part of it.
    pub partial: Vec<DeviceId>,
}

/// An answer to `Request::Holdings`: the responder's digest and `(blob, bytes held)` pairs.
pub(crate) type HoldingsAnswer = ([u8; 32], Vec<([u8; 32], u64)>);

/// Most online holders asked for their piece maps for one file page.
const PIECE_ASKS: usize = 16;

/// One member in a listing's swarm, for the file page.
#[derive(Clone, Debug, Serialize)]
pub struct SwarmPeerView {
    pub root_id: String,
    /// This person, on this device or another.
    pub you: bool,
    pub name: String,
    pub role: String,
    /// A seeder: a storage member, holding the encrypted copy only.
    pub storage: bool,
    pub online: bool,
    /// How much of the file it holds, 0 to 1.
    pub has: f64,
    pub level: u8,
}

/// A listing's swarm, for the file page.
#[derive(Clone, Debug, Serialize)]
pub struct SwarmView {
    pub seeders: u32,
    pub downloading: u32,
    pub peers: Vec<SwarmPeerView>,
    /// For each 64 KiB piece, how many of the asked holders have it.
    pub pieces: Vec<u8>,
    /// How many holders the piece counts come from.
    pub asked: u32,
}

fn digest_of(holdings: &[Holding]) -> [u8; 32] {
    let bytes = postcard::to_allocvec(holdings).unwrap_or_default();
    *blake3::keyed_hash(blake3::hash(b"dissent/tracker-holdings/v1").as_bytes(), &bytes).as_bytes()
}

impl Engine {
    /// What this device holds of `community`'s listings: what it seeds, and its tracker downloads in progress.
    pub(crate) fn my_holdings(&self, community: CommunityId) -> Vec<Holding> {
        // Only what this device is seeding now: a paused, stopped, queued or held-back seed is not announced.
        let seeds: Vec<[u8; 32]> = self
            .seed_states(community)
            .into_iter()
            .filter(|(_, s)| *s == super::seeding::SeedState::Seeding)
            .map(|(b, _)| b)
            .collect();
        let sizes: BTreeMap<[u8; 32], u64> =
            self.opened_listings(community).into_iter().map(|o| (o.body.blob, o.body.cipher_size)).collect();
        let prefix = format!("{TRANSFER_PREFIX}{community}:");
        let w = self.read();
        let mut out: Vec<Holding> =
            seeds.iter().filter_map(|b| sizes.get(b).map(|size| Holding { blob: *b, bytes: *size })).collect();
        for t in w.attach.transfers.values() {
            if t.message_id.starts_with(&prefix) && (t.state == "active" || t.state == "paused") && !seeds.contains(&t.file.hash) {
                out.push(Holding { blob: t.file.hash, bytes: (t.progress * t.file.cipher_size as f64) as u64 });
            }
        }
        out.sort_by_key(|h| h.blob);
        out
    }

    /// `my_holdings`, built at most once per `HOLDINGS_CACHE`, so a member asking in a loop costs little.
    fn cached_holdings(&self, community: CommunityId) -> Vec<Holding> {
        if let Some(cached) = self.with_tracker(community, |l| {
            l.answer.as_ref().filter(|(at, _)| at.elapsed() < HOLDINGS_CACHE).map(|(_, h)| h.clone())
        }) {
            return cached;
        }
        let fresh = self.my_holdings(community);
        self.with_tracker(community, |l| l.answer = Some((std::time::Instant::now(), fresh.clone())));
        fresh
    }

    /// Answers `Holdings`: what this device holds, unless the asker's digest says it already knows. For active persons
    /// and granted storage members of `community` only.
    pub(crate) fn serve_holdings(&self, from: DeviceId, community: CommunityId, digest: Option<[u8; 32]>) -> Response {
        let allowed = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error("unknown community".into());
            };
            w.directory.root_of(&from).is_some_and(|root| {
                let root = c.state().canonical(&root);
                CommitRules::of(c).is_active(&root)
                    && (c.state().members.get(&root).is_some_and(|m| !m.storage) || c.state().storage_granted(&root))
            })
        };
        if !allowed {
            return Response::Error("holdings are for members".into());
        }
        // A restricted category's blobs go only to its readers.
        let hidden: std::collections::BTreeSet<[u8; 32]> = {
            let opened = self.opened_listings(community);
            let w = self.read();
            let root = w.directory.root_of(&from);
            w.communities
                .get(&community)
                .map(|c| {
                    let st = c.state();
                    opened
                        .iter()
                        .filter(|o| st.files.categories.get(&o.category).is_some_and(|cat| cat.visible.is_some()))
                        .filter(|o| !root.is_some_and(|r| st.may_see_files(&r, &o.category)))
                        .map(|o| o.body.blob)
                        .collect()
                })
                .unwrap_or_default()
        };
        let holdings: Vec<Holding> = self.cached_holdings(community).into_iter().filter(|h| !hidden.contains(&h.blob)).collect();
        let mine = digest_of(&holdings);
        if digest == Some(mine) {
            return Response::Holdings(None);
        }
        Response::Holdings(Some((mine, holdings.into_iter().map(|h| (h.blob, h.bytes)).collect())))
    }

    /// Asks the online member devices of `community` what they hold, unless that was done in the last
    /// `HOLDINGS_EVERY`, or `now` asks for it anyway.
    pub(crate) async fn refresh_holdings(&self, community: CommunityId, now: bool) {
        let due = self.with_tracker(community, |l| {
            let due = now || l.holdings_asked.is_none_or(|at| at.elapsed() >= HOLDINGS_EVERY);
            if due {
                l.holdings_asked = Some(std::time::Instant::now());
            }
            due
        });
        if !due {
            return;
        }
        let devices: Vec<(DeviceId, RootId)> = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            if c.state().files.listings.is_empty() {
                return;
            }
            let at = now_ms();
            w.member_devices(c)
                .into_iter()
                .filter_map(|d| w.directory.root_of(&d).map(|r| (d, r)))
                .filter(|(_, r)| w.presence_of(r, at) != crate::presence::Presence::Offline)
                .collect()
        };
        let mut changed = false;
        for (device, root) in devices {
            let known = self.with_tracker(community, |l| l.heard.get(&device).map(|h| h.digest));
            let ask = Request::Holdings { community, digest: known };
            match tokio::time::timeout(ASK_WAIT, self.call(device, &ask)).await {
                Ok(Ok(Response::Holdings(None))) => {
                    self.with_tracker(community, |l| {
                        if let Some(h) = l.heard.get_mut(&device) {
                            h.at_ms = now_ms();
                        }
                    });
                }
                Ok(Ok(Response::Holdings(Some((digest, entries))))) => {
                    let holdings: Vec<Holding> = entries.into_iter().map(|(blob, bytes)| Holding { blob, bytes }).collect();
                    self.with_tracker(community, |l| {
                        l.heard.insert(device, HeardHoldings { root: Some(root), at_ms: now_ms(), digest, holdings });
                    });
                    changed = true;
                }
                Ok(Ok(other)) => debug!(?device, ?other, "no holdings"),
                _ => {}
            }
        }
        if changed {
            self.note_completions(community);
            self.emit(crate::views::EngineEvent::CommunityUpdated { community_id: community.to_string() });
        }
    }

    /// Records which roots were seen holding each listing whole, for its "downloaded completely" count.
    fn note_completions(&self, community: CommunityId) {
        let sizes: BTreeMap<[u8; 32], u64> =
            self.opened_listings(community).into_iter().map(|o| (o.body.blob, o.body.cipher_size)).collect();
        let changed = self.with_tracker(community, |l| {
            let mut changed = false;
            for heard in l.heard.values() {
                let Some(root) = heard.root else { continue };
                for h in &heard.holdings {
                    if sizes.get(&h.blob) == Some(&h.bytes) {
                        changed |= l.book.completed.entry(h.blob).or_default().insert(root);
                    }
                }
            }
            changed
        });
        if changed {
            self.save_tracker_book(community);
        }
    }

    /// The swarms of `community`'s listings, by blob, from the holdings heard lately. `sizes` maps each blob to its
    /// ciphertext size.
    pub(crate) fn swarms(&self, community: CommunityId, sizes: &BTreeMap<[u8; 32], u64>) -> BTreeMap<[u8; 32], Swarm> {
        let now = now_ms();
        let mut heard: Vec<(DeviceId, HeardHoldings)> =
            self.with_tracker(community, |l| l.heard.iter().map(|(d, h)| (*d, h.clone())).collect());
        heard.retain(|(_, h)| now.saturating_sub(h.at_ms) < HOLDINGS_KEPT_MS);
        heard.sort_by_key(|(_, h)| std::cmp::Reverse(h.at_ms));
        let mut out: BTreeMap<[u8; 32], Swarm> = BTreeMap::new();
        for (device, h) in heard {
            let Some(root) = h.root else { continue };
            for holding in h.holdings {
                let Some(size) = sizes.get(&holding.blob) else { continue };
                let swarm = out.entry(holding.blob).or_default();
                if holding.bytes >= *size {
                    swarm.seeders.insert(root);
                    swarm.devices.push(device);
                } else if now.saturating_sub(h.at_ms) < LEECHING_MS {
                    swarm.leechers.insert(root);
                    swarm.partial.push(device);
                }
            }
        }
        out
    }

    /// Publishes the receipt for a finished swarm download: the ciphertext bytes each provider's root delivered,
    /// without the providers that failed a slice and without this person. The receipt names no file: its tag is keyed
    /// with this device's own secret.
    pub(crate) async fn send_receipt(&self, transfer: &str, message_id: &str, failed: &[usize]) {
        let Some((community, listing)) = message_id
            .strip_prefix(TRANSFER_PREFIX)
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(c, l)| Some((c.parse::<CommunityId>().ok()?, l.parse::<EventId>().ok()?)))
        else {
            return;
        };
        let me = self.inner.me;
        let credits: Vec<(RootId, u64)> = {
            let w = self.read();
            let Some(live) = w.source_counts.get(transfer) else {
                return;
            };
            let mut by_root: BTreeMap<RootId, u64> = BTreeMap::new();
            for count in &live.counts {
                if failed.contains(&count.index) || count.bytes == 0 {
                    continue;
                }
                let Some(device) = live.providers.get(count.index).map(|p| p.id) else { continue };
                let Some(root) = w.directory.root_of(&device) else { continue };
                if root != me {
                    *by_root.entry(root).or_default() += count.bytes;
                }
            }
            by_root.into_iter().collect()
        };
        if credits.is_empty() {
            return;
        }
        let secret = self.with_tracker(community, |l| l.book.receipt_secret);
        let keyed = blake3::keyed_hash(&secret, &listing.0);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&keyed.as_bytes()[..16]);
        if let Err(e) = self.author_publish(community, ChannelId::CONTROL, Payload::FileReceipt { tag, credits }).await {
            debug!(error = %e, "receipt not accepted");
        }
    }

    /// The swarm of one listing for its file page: who holds it, how much, and how many online holders have each piece.
    /// Asks up to `PIECE_ASKS` online holders for their piece maps.
    pub async fn listing_swarm(&self, community: CommunityId, listing: EventId) -> crate::error::Result<SwarmView> {
        let opened = self.opened_listings(community);
        let o = opened
            .iter()
            .find(|o| o.id == listing)
            .ok_or_else(|| crate::error::CoreError::NotFound(format!("listing {listing}")))?;
        let (blob, size) = (o.body.blob, o.body.cipher_size);
        let ask_now = self.blob_moving(&blob)
            && self.with_tracker(community, |l| l.holdings_asked.is_none_or(|at| at.elapsed() >= HOLDINGS_EVERY_MOVING));
        self.refresh_holdings(community, ask_now).await;
        let me = self.inner.me;
        let now = now_ms();
        let heard: Vec<(DeviceId, HeardHoldings)> =
            self.with_tracker(community, |l| l.heard.iter().map(|(d, h)| (*d, h.clone())).collect());
        let mut peers: BTreeMap<RootId, SwarmPeerView> = BTreeMap::new();
        let mut online: Vec<DeviceId> = Vec::new();
        {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
            for (device, h) in &heard {
                let Some(root) = h.root else { continue };
                let Some(holding) = h.holdings.iter().find(|x| x.blob == blob) else { continue };
                if now.saturating_sub(h.at_ms) >= HOLDINGS_KEPT_MS {
                    continue;
                }
                if w.presence_of(&root, now) != crate::presence::Presence::Offline {
                    online.push(*device);
                }
                let has = (holding.bytes as f64 / size.max(1) as f64).min(1.0);
                let entry = peers.entry(root).or_insert_with(|| SwarmPeerView {
                    you: root == me,
                    root_id: root.to_string(),
                    name: w.display_name(&root),
                    role: c.role_of(&root).to_string(),
                    storage: c.state().members.get(&root).is_some_and(|m| m.storage),
                    online: false,
                    has: 0.0,
                    level: c.state().files.level_of(&root, now),
                });
                entry.has = entry.has.max(has);
                entry.online |= w.presence_of(&root, now) != crate::presence::Presence::Offline;
            }
            // A lister nobody has heard from yet holds it, as far as anyone knows; this device says for itself below.
            let heard_roots: std::collections::BTreeSet<RootId> = heard.iter().filter_map(|(_, h)| h.root).collect();
            for lister in opened.iter().filter(|x| x.body.blob == blob).map(|x| x.lister) {
                if lister != me && !heard_roots.contains(&lister) {
                    peers.entry(lister).or_insert_with(|| SwarmPeerView {
                        you: false,
                        root_id: lister.to_string(),
                        name: w.display_name(&lister),
                        role: c.role_of(&lister).to_string(),
                        storage: c.state().members.get(&lister).is_some_and(|m| m.storage),
                        online: w.presence_of(&lister, now) != crate::presence::Presence::Offline,
                        has: 1.0,
                        level: c.state().files.level_of(&lister, now),
                    });
                }
            }
        }
        let pieces_total = dissent_files::pieces_of(size) as usize;
        let mut pieces = vec![0u8; pieces_total];
        let node = self.node();
        let peers_addr = self.read().addresses.peers();
        for device in online.iter().take(PIECE_ASKS) {
            let Some(addr) = peers_addr.iter().find(|p| p.id == *device).and_then(|p| p.clone().to_iroh().ok()) else { continue };
            if let Ok(map) = self.inner.files.piece_map(node.endpoint(), addr, blob, size).await {
                for (count, held) in pieces.iter_mut().zip(map) {
                    *count = count.saturating_add(u8::from(held));
                }
            }
        }
        // This device's own copy counts too.
        let mine = self.seed_states(community).get(&blob) == Some(&super::seeding::SeedState::Seeding);
        if mine {
            for count in &mut pieces {
                *count = count.saturating_add(1);
            }
        }
        // This device's own row: whole while it seeds, its progress while it downloads.
        let own_download = {
            let w = self.read();
            let wanted = format!("{TRANSFER_PREFIX}{community}:{listing}");
            w.attach.transfers.values().find(|t| t.message_id == wanted && t.state == "active").map(|t| t.progress)
        };
        if let Some(has) = if mine { Some(1.0) } else { own_download } {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
            let entry = peers.entry(me).or_insert_with(|| SwarmPeerView {
                you: true,
                root_id: me.to_string(),
                name: w.display_name(&me),
                role: c.role_of(&me).to_string(),
                storage: false,
                online: true,
                has: 0.0,
                level: c.state().files.level_of(&me, now),
            });
            entry.has = entry.has.max(has);
            entry.online = true;
        }
        let mut peers: Vec<SwarmPeerView> = peers.into_values().collect();
        peers.sort_by(|a, b| b.online.cmp(&a.online).then(b.has.total_cmp(&a.has)).then_with(|| a.name.cmp(&b.name)));
        Ok(SwarmView {
            seeders: peers.iter().filter(|p| p.has >= 1.0).count() as u32,
            downloading: peers.iter().filter(|p| p.has < 1.0).count() as u32,
            peers,
            pieces,
            asked: online.len().min(PIECE_ASKS) as u32 + u32::from(mine),
        })
    }

    /// Asks for holdings in every community that has listings. Called from the maintenance tick.
    pub(crate) async fn refresh_all_holdings(&self) {
        let communities: Vec<CommunityId> = self
            .read()
            .communities
            .iter()
            .filter(|(_, c)| !c.state().files.listings.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for community in communities {
            self.reconcile_linked(community);
            self.refresh_holdings(community, false).await;
        }
        self.save_dirty_tracker_books();
    }
}
