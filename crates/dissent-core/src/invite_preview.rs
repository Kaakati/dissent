//! Invite previews: what a community looks like before its invite is used. A member answers with
//! the community's name, icon and counts, signed by its device, to anyone who proves the invite
//! secret. The preview reveals name, icon and counts only. It never names a member or shows an id.
//! See `docs/design/06-threat-model.md`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_discovery::InviteLink;
use dissent_events::Picture;
use dissent_identity::{CommunityId, DeviceCert, DeviceId, Genesis, Signature};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::access::community_token;
use crate::community::Community;
use crate::engine::{not_found, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::presence::Presence;
use crate::rpc::{Request, Response};
use crate::world::World;

/// How long a fetched preview is reused. Counts move slowly, and a join checks the invite again.
const CACHE_FOR: Duration = Duration::from_secs(60);
/// Preview requests one requester may make of this device within `ASK_WINDOW`.
const ASKS_PER_WINDOW: usize = 10;
const ASK_WINDOW: Duration = Duration::from_secs(60);
/// Requesters remembered at once. Older ones are dropped once the table grows past this.
const MAX_TRACKED_ASKERS: usize = 1024;
/// How long one peer may take to answer a preview request before the next is tried.
const ASK_WAIT: Duration = Duration::from_secs(10);
/// How long fetching the icon may take. The card shows initials when it runs out.
const ICON_WAIT: Duration = Duration::from_secs(5);
const SIG_DOMAIN: &[u8] = b"dissent/invite-preview/v1";

/// What a member shows of a community to people holding its invite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvitePreview {
    /// The community name, as in its genesis.
    pub name: String,
    /// The community icon, as a picture reference. `None` when there is none.
    pub icon: Option<Picture>,
    /// Members who have not left or been banned.
    pub member_count: u32,
    /// Members of those who show online to the answering device.
    pub online_count: u32,
    /// When the community was created, in ms since the Unix epoch.
    pub created_ms: u64,
    /// Whether the invite the requester named can still be used.
    pub invite: InviteStatus,
}

/// Whether an invite can still be used, as the answering member sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    /// Usable. Also the answer for a link with no invite id, which has no state of its own.
    Live,
    Revoked,
    Expired,
    /// The answering member has no record of this invite yet. The join may still succeed.
    Unknown,
}

/// A preview signed by the answering device. The genesis is the community's own, signed by its key,
/// so the name and creation time check against the community id and not only against the device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedPreview {
    pub preview: InvitePreview,
    pub genesis: Genesis,
    pub cert: DeviceCert,
    pub sig: Signature,
}

fn signing_bytes(community: &CommunityId, preview: &InvitePreview) -> Result<Vec<u8>> {
    let mut out = SIG_DOMAIN.to_vec();
    out.extend_from_slice(community.as_bytes());
    out.extend(postcard::to_allocvec(preview).map_err(|e| CoreError::Encoding(e.to_string()))?);
    Ok(out)
}

impl SignedPreview {
    /// Checks the device cert, the genesis for `community`, that the name and date match it, and
    /// the device's signature.
    pub(crate) fn verify(&self, community: &CommunityId) -> Result<()> {
        let bad = |why: &str| CoreError::Invalid(format!("invite preview: {why}"));
        self.cert
            .verify()
            .map_err(|_| bad("the device certificate does not verify"))?;
        if self.genesis.community != *community {
            return Err(bad("it is for another community"));
        }
        self.genesis
            .verify()
            .map_err(|_| bad("the genesis does not verify"))?;
        if self.preview.name != self.genesis.name || self.preview.created_ms != self.genesis.created_ms {
            return Err(bad("the name or date differs from the genesis"));
        }
        if self.preview.icon.is_some_and(|picture| !picture.is_valid()) {
            return Err(bad("the icon is out of range"));
        }
        self.cert
            .device
            .verify(&signing_bytes(community, &self.preview)?, &self.sig)
            .map_err(|_| bad("the signature does not verify"))
    }
}

/// The preview of an invite as the UI shows it: a member's, or this device's own view when it is
/// already in the community. JSON, with snake_case fields.
#[derive(Clone, Debug, Serialize)]
pub struct InvitePreviewView {
    /// The community id, as base32.
    pub community: String,
    pub name: String,
    /// Path of the decrypted icon on this device. `None` when there is none yet, so show initials.
    pub icon: Option<String>,
    pub member_count: u32,
    pub online_count: u32,
    /// When the community was created, in ms since the Unix epoch.
    pub created_ms: u64,
    /// One of `live`, `revoked`, `expired`, `unknown`. Show "Invite expired or revoked" for the
    /// first two.
    pub invite: InviteStatus,
    /// True when this device is already in the community. Then "Go to Server" replaces "Join".
    pub already_member: bool,
}

/// A preview's key in the cache: the community and the invite id the requester named.
type PreviewKey = (CommunityId, Option<[u8; 16]>);

/// A cached preview, when it was fetched, and the peer that answered it.
type Cached = (Instant, SignedPreview, PeerAddr);

/// Previews this device holds, and how often each requester has asked it. In memory only.
#[derive(Default)]
pub(crate) struct PreviewState {
    /// Previews this device fetched, by community and invite id, for `CACHE_FOR`.
    cache: Mutex<BTreeMap<PreviewKey, Cached>>,
    /// When each requester asked this device, within `ASK_WINDOW`.
    asks: Mutex<BTreeMap<DeviceId, VecDeque<Instant>>>,
}

impl PreviewState {
    /// Records a request from `from` and says whether this device still answers it.
    fn admit(&self, from: DeviceId, now: Instant) -> bool {
        let mut asks = self.asks.lock().unwrap_or_else(PoisonError::into_inner);
        if asks.len() >= MAX_TRACKED_ASKERS {
            asks.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < ASK_WINDOW));
        }
        let times = asks.entry(from).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= ASK_WINDOW) {
            times.pop_front();
        }
        if times.len() >= ASKS_PER_WINDOW {
            return false;
        }
        times.push_back(now);
        true
    }

    fn cached(&self, key: &PreviewKey) -> Option<(SignedPreview, PeerAddr)> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .get(key)
            .filter(|(at, _, _)| at.elapsed() < CACHE_FOR)
            .map(|(_, signed, peer)| (signed.clone(), peer.clone()))
    }

    fn remember(&self, key: PreviewKey, signed: &SignedPreview, peer: &PeerAddr) {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.retain(|_, (at, _, _)| at.elapsed() < CACHE_FOR);
        cache.insert(key, (Instant::now(), signed.clone(), peer.clone()));
    }
}

/// The invite's state in `c`, for the invite a requester names.
fn invite_status(c: &Community, invite_id: Option<[u8; 16]>, now: u64) -> InviteStatus {
    let Some(id) = invite_id else {
        return InviteStatus::Live;
    };
    match c.state().invites.get(&id) {
        None => InviteStatus::Unknown,
        Some(invite) if invite.revoked => InviteStatus::Revoked,
        Some(invite) if invite.expires_ms.is_some_and(|at| now >= at) => InviteStatus::Expired,
        Some(_) => InviteStatus::Live,
    }
}

/// Members who have not left or been banned, and how many of them show online to this device.
fn member_counts(w: &World, c: &Community, now: u64) -> (u32, u32) {
    let state = c.state();
    let members: Vec<_> = state
        .members
        .iter()
        .filter(|(root, m)| !m.left && !state.banned.contains(root))
        .map(|(root, _)| *root)
        .collect();
    let online = members
        .iter()
        .filter(|root| !matches!(w.presence_of(root, now), Presence::Offline))
        .count();
    (
        u32::try_from(members.len()).unwrap_or(u32::MAX),
        u32::try_from(online).unwrap_or(u32::MAX),
    )
}

impl Engine {
    /// Answers a preview request. Refused unless `token` proves the invite secret and the sending
    /// device is not banned there. Requests from the same device are rate-limited.
    pub(crate) fn answer_invite_preview(
        &self,
        from: DeviceId,
        community: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
    ) -> Response {
        let now = now_ms();
        let proven = self
            .access_keys(community)
            .is_some_and(|keys| keys.accepts(&community, Some(token), now));
        if !proven || self.device_banned_in(from, community) {
            return Response::Error("access denied".into());
        }
        if !self.inner.previews.admit(from, Instant::now()) {
            return Response::Refused("too many invite previews; try again in a minute".into());
        }
        match self.signed_preview(community, invite_id, now) {
            Ok(signed) => Response::InvitePreview(Box::new(signed)),
            Err(e) => Response::Error(e.to_string()),
        }
    }

    /// This device's signed preview of `community`.
    fn signed_preview(&self, community: CommunityId, invite_id: Option<[u8; 16]>, now: u64) -> Result<SignedPreview> {
        let (genesis, preview) = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
            let (member_count, online_count) = member_counts(&w, c, now);
            let preview = InvitePreview {
                name: c.genesis.name.clone(),
                icon: c.state().icon.and_then(|(_, picture)| picture),
                member_count,
                online_count,
                created_ms: c.genesis.created_ms,
                invite: invite_status(c, invite_id, now),
            };
            (c.genesis.clone(), preview)
        };
        let sig = self.inner.identity.device.sign(&signing_bytes(&community, &preview)?);
        Ok(SignedPreview {
            preview,
            genesis,
            cert: self.inner.identity.cert.clone(),
            sig,
        })
    }

    /// The preview of an invite. Either form works. Joining is a separate step.
    ///
    /// A community this device is already in is previewed from its own state, with no network.
    /// Otherwise a member is asked: the invite's peers first, then the peers in the DHT record. A
    /// preview is cached for `CACHE_FOR`.
    pub async fn invite_preview(&self, link: &str) -> Result<InvitePreviewView> {
        let link = InviteLink::parse(link)?;
        let id = link.community();
        if let Some(view) = self.own_preview(&link) {
            return Ok(view);
        }
        let (signed, peer) = self.fetched_preview(&link).await?;
        let icon = self.icon_path(signed.preview.icon, &peer).await;
        let preview = signed.preview;
        Ok(InvitePreviewView {
            community: id.to_string(),
            name: preview.name,
            icon,
            member_count: preview.member_count,
            online_count: preview.online_count,
            created_ms: preview.created_ms,
            invite: preview.invite,
            already_member: false,
        })
    }

    /// The preview of a community this device is already in, from its own state.
    fn own_preview(&self, link: &InviteLink) -> Option<InvitePreviewView> {
        let id = link.community();
        let w = self.read();
        let c = w.communities.get(&id)?;
        let now = now_ms();
        let (member_count, online_count) = member_counts(&w, c, now);
        Some(InvitePreviewView {
            community: id.to_string(),
            name: c.genesis.name.clone(),
            icon: c.state().icon.and_then(|(_, picture)| picture).and_then(|p| crate::avatars::picture_file(&w.paths, &p)),
            member_count,
            online_count,
            created_ms: c.genesis.created_ms,
            invite: invite_status(c, link.invite_id(), now),
            already_member: true,
        })
    }

    /// A signed preview from a member, cached or fetched, and the peer that answered it.
    async fn fetched_preview(&self, link: &InviteLink) -> Result<(SignedPreview, PeerAddr)> {
        let id = link.community();
        let key = (id, link.invite_id());
        if let Some(hit) = self.inner.previews.cached(&key) {
            return Ok(hit);
        }
        let token = community_token(&link.secret(), &id);
        let invited = match link {
            InviteLink::Full(invite) => self.learn_peers(&invite.peers),
            InviteLink::Short(_) => Vec::new(),
        };
        let mut found = self.ask_peers(id, token, link.invite_id(), &invited).await;
        if found.is_none() {
            let from_record = self.record_peers(&id, &link.secret()).await;
            found = self.ask_peers(id, token, link.invite_id(), &from_record).await;
        }
        let (signed, peer) = found.ok_or_else(|| {
            CoreError::NotFound("no member answered the invite preview; the invite may be expired or revoked".into())
        })?;
        self.inner.previews.remember(key, &signed, &peer);
        Ok((signed, peer))
    }

    /// Remembers the peers of an invite, so they can be dialled, and returns them.
    fn learn_peers(&self, peers: &[String]) -> Vec<PeerAddr> {
        let parsed: Vec<PeerAddr> = peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()).collect();
        let mut guard = self.write();
        for peer in &parsed {
            self.learn_address(&mut guard, peer.clone());
        }
        parsed
    }

    /// Asks each peer in turn for the preview, and returns the first verified answer.
    async fn ask_peers(
        &self,
        id: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
        peers: &[PeerAddr],
    ) -> Option<(SignedPreview, PeerAddr)> {
        for peer in peers {
            match self.ask_peer(id, token, invite_id, peer).await {
                Ok(signed) => return Some((signed, peer.clone())),
                Err(e) => debug!(community = %id, peer = %peer.id, error = %e, "invite preview not answered"),
            }
        }
        None
    }

    async fn ask_peer(
        &self,
        id: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
        peer: &PeerAddr,
    ) -> Result<SignedPreview> {
        let req = Request::InvitePreview {
            community: id,
            token,
            invite_id,
        };
        let reply = tokio::time::timeout(ASK_WAIT, self.call(peer.id, &req))
            .await
            .map_err(|_| CoreError::NotFound("the peer did not answer the invite preview in time".into()))??;
        match reply {
            Response::InvitePreview(signed) => {
                signed.verify(&id)?;
                Ok(*signed)
            }
            Response::Refused(why) | Response::Error(why) => Err(CoreError::Rejected(why)),
            _ => Err(CoreError::Invalid("unexpected reply to an invite preview".into())),
        }
    }

    /// The local path of a community icon. A missing icon is fetched from `peer`, for up to
    /// `ICON_WAIT`, so a card can show it before the join.
    async fn icon_path(&self, picture: Option<Picture>, peer: &PeerAddr) -> Option<String> {
        let picture = picture?;
        if let Some(path) = crate::avatars::picture_file(&self.read().paths, &picture) {
            return Some(path);
        }
        let _ = tokio::time::timeout(ICON_WAIT, self.fetch_blob(&picture, std::slice::from_ref(peer))).await;
        crate::avatars::picture_file(&self.read().paths, &picture)
    }
}

#[cfg(test)]
mod tests;
