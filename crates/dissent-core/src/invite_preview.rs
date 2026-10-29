//! Invite previews: what a community looks like before its invite is used. A member answers with
//! the community's name, icon and counts, signed by its device, to anyone who proves the invite
//! secret. The preview reveals name, icon and counts only. It never names a member or shows an id.
//! See `docs/design/06-threat-model.md`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_discovery::InviteLink;
use dissent_events::state::valid_community_name;
use dissent_events::state::{valid_profile, CommunityProfile};
use dissent_events::{AnimatedIcon, Banner, ChannelId, Payload, Picture, SealKey, SignedEvent, Trait};
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
const EXTRA_SIG_DOMAIN: &[u8] = b"dissent/invite-preview-extra/v1";

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

/// What a preview adds for clients that know renames, banners and profiles. It travels beside the
/// `SignedPreview`, whose encoding older clients decode, so that one keeps the genesis name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewExtra {
    /// The owner's rename that set the current name, as stored. The joiner opens it with the invite
    /// secret and checks that the genesis owner signed it, so the owner vouches for the name, not the
    /// answering member. See [`proven_name`].
    pub rename: Option<SignedEvent>,
    /// The community banner: a preset, or a picture reference. Only the answering member vouches for
    /// it, as for the icon.
    pub banner: Option<Banner>,
    /// The description and traits. Only the answering member vouches for them.
    pub profile: Option<CommunityProfile>,
}

/// A preview with its extra, both signed by the answering device. The answer to
/// `Request::InvitePreviewFull`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedPreviewFull {
    pub base: SignedPreview,
    pub extra: PreviewExtra,
    /// The device's signature over the base signature and the extra.
    pub sig: Signature,
}

fn extra_signing_bytes(community: &CommunityId, base: &Signature, extra: &PreviewExtra) -> Result<Vec<u8>> {
    let mut out = EXTRA_SIG_DOMAIN.to_vec();
    out.extend_from_slice(community.as_bytes());
    out.extend(postcard::to_allocvec(&(base, extra)).map_err(|e| CoreError::Encoding(e.to_string()))?);
    Ok(out)
}

impl SignedPreviewFull {
    /// Checks the base preview, the banner's size, and the device's signature over the extra.
    pub(crate) fn verify(&self, community: &CommunityId) -> Result<()> {
        self.base.verify(community)?;
        if self.extra.banner.is_some_and(|banner| !banner.is_valid()) {
            return Err(CoreError::Invalid("invite preview: the banner is out of range".into()));
        }
        if self.extra.profile.as_ref().is_some_and(|p| !valid_profile(&p.description, &p.traits)) {
            return Err(CoreError::Invalid("invite preview: the profile is over its caps".into()));
        }
        self.base
            .cert
            .device
            .verify(&extra_signing_bytes(community, &self.base.sig, &self.extra)?, &self.sig)
            .map_err(|_| CoreError::Invalid("invite preview: the extra's signature does not verify".into()))
    }
}

/// The name a rename proves for the community of `genesis`: the rename must be signed by the
/// genesis owner, on the control channel, and open under `ring` to a valid `CommunityRename`.
/// `None` otherwise, and the genesis name stands. An owner who rotated to a new root proves nothing
/// here, so its renames show as the genesis name until the join.
pub(crate) fn proven_name(rename: &SignedEvent, genesis: &Genesis, ring: &[SealKey]) -> Option<String> {
    rename.verify().ok()?;
    let body = &rename.event.body;
    if body.community != genesis.community || body.channel != ChannelId::CONTROL || body.author != genesis.owner {
        return None;
    }
    let plain = dissent_events::open_event(&rename.event, ring).ok()?;
    match &plain.body.payload {
        Payload::CommunityRename { name } if valid_community_name(name) => Some(name.clone()),
        _ => None,
    }
}

/// The preset id of a banner that is one.
pub(crate) fn preset_of(banner: Option<Banner>) -> Option<u8> {
    match banner {
        Some(Banner::Preset(id)) => Some(id),
        _ => None,
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
    /// For an animated icon, its GIF, when it came with the more part.
    pub icon: Option<String>,
    /// Path of an animated icon's still frame, shown under reduced motion. `None` for a still icon.
    pub icon_poster: Option<String>,
    /// Path of the decrypted banner on this device. `None` when there is none or it is not fetched.
    pub banner: Option<String>,
    /// Path of an animated banner's still frame, shown under reduced motion.
    pub banner_poster: Option<String>,
    /// The preset gradient the banner is, when it is one. The UI draws presets itself.
    pub banner_preset: Option<u8>,
    /// The community's description, empty when it has none.
    pub description: String,
    /// The community's traits, at most five.
    pub traits: Vec<Trait>,
    pub member_count: u32,
    pub online_count: u32,
    /// When the community was created, in ms since the Unix epoch.
    pub created_ms: u64,
    /// One of `live`, `revoked`, `expired`, `unknown`. Show "Invite expired or revoked" for the
    /// first two.
    pub invite: InviteStatus,
    /// True when this device is already in the community. Then "Go to Server" replaces "Join".
    pub already_member: bool,
    /// The server's boost level and its verified relay hosts, as the answering member counts them.
    /// Zero from a member that predates boosts.
    pub boost_level: u8,
    pub boost_hosts: u32,
}

/// A preview's key in the cache: the community and the invite id the requester named.
type PreviewKey = (CommunityId, Option<[u8; 16]>);

/// A verified answer: the preview, its extra when the member sent one, and the part only newer
/// members send.
type Answered = (SignedPreview, PreviewExtra, Option<PreviewMore>);

/// What a preview adds past `PreviewExtra`, whose encoding the released clients decode: the banner
/// in a form they may not know (an animated GIF), and the server's boost. The answer to
/// `Request::InvitePreviewMore`. Only the answering member vouches for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewMore {
    pub banner: Option<Banner>,
    /// The boost level and verified relay hosts, as the answering member counts them.
    pub boost_level: u8,
    pub boost_hosts: u32,
    /// The animated icon, when the community has one. The preview's own icon is its still poster.
    pub icon: Option<AnimatedIcon>,
}

/// A full preview with its more, signed by the same device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedPreviewMore {
    pub full: SignedPreviewFull,
    pub more: PreviewMore,
    pub sig: Signature,
}

const MORE_SIG_DOMAIN: &[u8] = b"dissent/invite-preview-more/v1";

fn more_signing_bytes(community: &CommunityId, full: &Signature, more: &PreviewMore) -> Result<Vec<u8>> {
    let mut out = MORE_SIG_DOMAIN.to_vec();
    out.extend_from_slice(community.as_bytes());
    out.extend(postcard::to_allocvec(&(full, more)).map_err(|e| CoreError::Encoding(e.to_string()))?);
    Ok(out)
}

impl SignedPreviewMore {
    pub(crate) fn verify(&self, community: &CommunityId) -> Result<()> {
        self.full.verify(community)?;
        if self.more.banner.is_some_and(|b| !b.is_valid()) || self.more.icon.is_some_and(|i| !i.is_valid()) || self.more.boost_level > 3 {
            return Err(CoreError::Invalid("invite preview: the more part is out of range".into()));
        }
        self.full
            .base
            .cert
            .device
            .verify(&more_signing_bytes(community, &self.full.sig, &self.more)?, &self.sig)
            .map_err(|_| CoreError::Invalid("invite preview: the more part's signature does not verify".into()))
    }
}

/// A banner as the released `PreviewExtra` can carry it: an animated one as its still poster, which
/// released clients decode, while `PreviewMore` carries the GIF.
fn released_banner(banner: Option<Banner>) -> Option<Banner> {
    match banner {
        Some(Banner::Animated { poster, .. }) => Some(Banner::Image(poster)),
        other => other,
    }
}

/// A cached preview, when it was fetched, and the peer that answered it.
type Cached = (Instant, Answered, PeerAddr);

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
        // The cap is hard: a new requester evicts the one that asked longest ago.
        while asks.len() >= MAX_TRACKED_ASKERS && !asks.contains_key(&from) {
            let oldest = asks.iter().min_by_key(|(_, times)| times.back().copied()).map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    asks.remove(&id);
                }
                None => break,
            }
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

    fn cached(&self, key: &PreviewKey) -> Option<(Answered, PeerAddr)> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .get(key)
            .filter(|(at, _, _)| at.elapsed() < CACHE_FOR)
            .map(|(_, signed, peer)| (signed.clone(), peer.clone()))
    }

    fn remember(&self, key: PreviewKey, signed: &Answered, peer: &PeerAddr) {
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

    /// Answers a full preview request: the preview, with the extra signed beside it. Gated as
    /// [`Engine::answer_invite_preview`] is.
    pub(crate) fn answer_invite_preview_full(
        &self,
        from: DeviceId,
        community: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
    ) -> Response {
        let base = match self.answer_invite_preview(from, community, token, invite_id) {
            Response::InvitePreview(base) => *base,
            other => return other,
        };
        let extra = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error(not_found(&community).to_string());
            };
            PreviewExtra {
                rename: c.latest_rename(),
                banner: released_banner(c.state().banner.and_then(|(_, banner)| banner)),
                profile: c.state().profile.as_ref().map(|(_, p)| p.clone()),
            }
        };
        let bytes = match extra_signing_bytes(&community, &base.sig, &extra) {
            Ok(bytes) => bytes,
            Err(e) => return Response::Error(e.to_string()),
        };
        let sig = self.inner.identity.device.sign(&bytes);
        Response::InvitePreviewFull(Box::new(SignedPreviewFull { base, extra, sig }))
    }

    /// Answers `InvitePreviewMore`: the full preview, and the more part signed beside it.
    pub(crate) fn answer_invite_preview_more(
        &self,
        from: DeviceId,
        community: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
    ) -> Response {
        let full = match self.answer_invite_preview_full(from, community, token, invite_id) {
            Response::InvitePreviewFull(full) => *full,
            other => return other,
        };
        let more = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return Response::Error(not_found(&community).to_string());
            };
            let hosts = w.boost_hosts(c, now_ms()).len();
            PreviewMore {
                banner: c.state().banner.and_then(|(_, banner)| banner),
                boost_level: crate::boosts::level_for(hosts),
                boost_hosts: u32::try_from(hosts).unwrap_or(u32::MAX),
                icon: match c.state().shown_icon() {
                    Some((image, Some(poster))) => Some(AnimatedIcon { image, poster }),
                    _ => None,
                },
            }
        };
        let bytes = match more_signing_bytes(&community, &full.sig, &more) {
            Ok(bytes) => bytes,
            Err(e) => return Response::Error(e.to_string()),
        };
        let sig = self.inner.identity.device.sign(&bytes);
        Response::InvitePreviewMore(Box::new(SignedPreviewMore { full, more, sig }))
    }

    /// This device's signed preview of `community`.
    fn signed_preview(&self, community: CommunityId, invite_id: Option<[u8; 16]>, now: u64) -> Result<SignedPreview> {
        let (genesis, preview) = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
            let (member_count, online_count) = member_counts(&w, c, now);
            let preview = InvitePreview {
                name: c.genesis.name.clone(),
                icon: c.state().shown_icon().map(|(image, poster)| poster.unwrap_or(image)),
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
        let ((signed, mut extra, more), peer) = self.fetched_preview(&link).await?;
        // A newer member sends the banner as it is, an animated GIF included.
        if let Some(m) = &more {
            extra.banner = m.banner;
        }
        // The preview's icon is the still frame, or the still icon. An animated icon's GIF comes in the more part.
        let still = self.icon_path(signed.preview.icon, &peer).await;
        let gif = match more.as_ref().and_then(|m| m.icon) {
            Some(animated) => self.icon_path(Some(animated.image), &peer).await,
            None => None,
        };
        let (icon, icon_poster) = match gif {
            Some(gif) => (Some(gif), still),
            None => (still, None),
        };
        let banner = self.icon_path(extra.banner.and_then(|b| b.picture()), &peer).await;
        let banner_poster = self.icon_path(extra.banner.and_then(|b| b.poster()), &peer).await;
        let profile = extra.profile.clone().unwrap_or_default();
        // The invite holds one secret generation. A rename sealed under it proves the current name.
        let ring = [(link.secret_generation(), link.secret())];
        let name = extra
            .rename
            .as_ref()
            .and_then(|rename| proven_name(rename, &signed.genesis, &ring))
            .unwrap_or_else(|| signed.preview.name.clone());
        let preview = signed.preview;
        Ok(InvitePreviewView {
            community: id.to_string(),
            name,
            icon,
            icon_poster,
            banner,
            banner_poster,
            banner_preset: preset_of(extra.banner),
            description: profile.description,
            traits: profile.traits,
            member_count: preview.member_count,
            online_count: preview.online_count,
            created_ms: preview.created_ms,
            invite: preview.invite,
            already_member: false,
            boost_level: more.as_ref().map_or(0, |m| m.boost_level),
            boost_hosts: more.as_ref().map_or(0, |m| m.boost_hosts),
        })
    }

    /// The preview of a community this device is already in, from its own state.
    fn own_preview(&self, link: &InviteLink) -> Option<InvitePreviewView> {
        let id = link.community();
        let w = self.read();
        let c = w.communities.get(&id)?;
        let now = now_ms();
        let (member_count, online_count) = member_counts(&w, c, now);
        let profile = c.state().profile.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        Some(InvitePreviewView {
            community: id.to_string(),
            name: c.name().to_string(),
            icon: w.icon_of(c),
            icon_poster: w.icon_poster_of(c),
            banner: w.banner_of(c),
            banner_poster: w.banner_poster_of(c),
            banner_preset: preset_of(c.state().banner.and_then(|(_, b)| b)),
            description: profile.description,
            traits: profile.traits,
            member_count,
            online_count,
            created_ms: c.genesis.created_ms,
            invite: invite_status(c, link.invite_id(), now),
            already_member: true,
            boost_level: w.boost_level(c, now),
            boost_hosts: u32::try_from(w.boost_hosts(c, now).len()).unwrap_or(u32::MAX),
        })
    }

    /// A signed preview from a member, cached or fetched, and the peer that answered it.
    async fn fetched_preview(&self, link: &InviteLink) -> Result<(Answered, PeerAddr)> {
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

    /// Remembers the peers of an invite, so they can be dialled, and returns them. Only their public reachability is
    /// kept (see `communities::shareable_addr`): anyone can mint a long invite, and a preview must not make this device
    /// dial into a local network the invite names. Joining still uses every address the invite carries.
    fn learn_peers(&self, peers: &[String]) -> Vec<PeerAddr> {
        let parsed: Vec<PeerAddr> = peers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .map(|p| self.shareable(&p))
            .collect();
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
    ) -> Option<(Answered, PeerAddr)> {
        for peer in peers {
            match self.ask_peer(id, token, invite_id, peer).await {
                Ok(signed) => return Some((signed, peer.clone())),
                Err(e) => debug!(community = %id, peer = %peer.id, error = %e, "invite preview not answered"),
            }
        }
        None
    }

    /// Asks one peer for the full preview. A peer from before renames cannot decode that request
    /// and answers with an error, so the plain preview is asked for then.
    async fn ask_peer(
        &self,
        id: CommunityId,
        token: [u8; 32],
        invite_id: Option<[u8; 16]>,
        peer: &PeerAddr,
    ) -> Result<Answered> {
        let more = Request::InvitePreviewMore {
            community: id,
            token,
            invite_id,
        };
        if let Response::InvitePreviewMore(signed) = self.ask_once(peer, &more).await? {
            signed.verify(&id)?;
            let SignedPreviewMore { full, more, .. } = *signed;
            return Ok((full.base, full.extra, Some(more)));
        }
        let full = Request::InvitePreviewFull {
            community: id,
            token,
            invite_id,
        };
        if let Response::InvitePreviewFull(signed) = self.ask_once(peer, &full).await? {
            signed.verify(&id)?;
            return Ok((signed.base, signed.extra, None));
        }
        let req = Request::InvitePreview {
            community: id,
            token,
            invite_id,
        };
        match self.ask_once(peer, &req).await? {
            Response::InvitePreview(signed) => {
                signed.verify(&id)?;
                Ok((*signed, PreviewExtra::default(), None))
            }
            Response::Refused(why) | Response::Error(why) => Err(CoreError::Rejected(why)),
            _ => Err(CoreError::Invalid("unexpected reply to an invite preview".into())),
        }
    }

    async fn ask_once(&self, peer: &PeerAddr, req: &Request) -> Result<Response> {
        tokio::time::timeout(ASK_WAIT, self.call(peer.id, req))
            .await
            .map_err(|_| CoreError::NotFound("the peer did not answer the invite preview in time".into()))?
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

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn the_requester_table_never_exceeds_its_cap() {
        let state = PreviewState::default();
        let start = Instant::now();
        for n in 0..(MAX_TRACKED_ASKERS as u32 + 200) {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&n.to_le_bytes());
            state.admit(DeviceId(id), start);
        }
        let len = state.asks.lock().unwrap().len();
        assert!(len <= MAX_TRACKED_ASKERS, "{len} requesters tracked");
    }
}
