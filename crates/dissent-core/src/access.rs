//! Who may reach a community's traffic. Each community has a random 32-byte secret, given out in
//! its invites. The secret salts the gossip topic, so a device without it cannot join the topic.
//! It also keys a token that every community request carries, so a device without it cannot sync
//! with members. Bans rotate the secret (see `membership`), and a retired secret keeps working for
//! a grace period, but never for a banned device. See `docs/design/06-threat-model.md`, finding 2.

use std::collections::BTreeMap;

use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::rpc::Request;
use crate::store::{write_bin, Paths};

/// How long a retired secret keeps working after a rotation. Invites made before it stay valid for
/// this long, and then have to be re-issued.
pub(crate) const GRACE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Derives the gossip topic salt from a community secret.
pub(crate) fn topic_salt(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("dissent/topic-salt/v2", secret)
}

/// The token a community request carries: a keyed hash of the community id under a key derived
/// from the secret.
pub(crate) fn community_token(secret: &[u8; 32], community: &CommunityId) -> [u8; 32] {
    let key = blake3::derive_key("dissent/rpc-token/v2", secret);
    *blake3::keyed_hash(&key, community.as_bytes()).as_bytes()
}

/// A fresh community secret.
pub(crate) fn random_secret() -> [u8; 32] {
    rand::random()
}

/// A secret that has been replaced, and when it stops being accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retired {
    pub secret: [u8; 32],
    pub until_ms: u64,
}

/// What this device knows of a community's secret: the current one, its generation, and the
/// retired ones still inside their grace period.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Keys {
    pub generation: u64,
    pub secret: [u8; 32],
    pub retired: Vec<Retired>,
    /// The rotation event that installed this secret. All zero for a secret taken from an invite,
    /// whose source is unknown.
    pub source: [u8; 32],
}

impl Keys {
    /// Keys from an invite, at the generation the invite names.
    pub fn from_invite(generation: u64, secret: [u8; 32]) -> Self {
        Self {
            generation,
            secret,
            retired: Vec::new(),
            source: [0; 32],
        }
    }

    /// Installs a secret from rotation event `source`. It replaces the current one when it is
    /// newer, or when it is a rival at the same generation (a concurrent rotation that won the
    /// tie-break). The one it replaces keeps working until `now + GRACE_MS`. Returns whether it was
    /// installed.
    pub fn install(&mut self, generation: u64, secret: [u8; 32], source: [u8; 32], now: u64) -> bool {
        let rival = generation == self.generation
            && self.source != [0; 32]
            && source != self.source;
        if generation < self.generation || (generation == self.generation && !rival) {
            return false;
        }
        self.retired.push(Retired {
            secret: self.secret,
            until_ms: now.saturating_add(GRACE_MS),
        });
        self.retired.retain(|r| r.until_ms > now);
        self.generation = generation;
        self.secret = secret;
        self.source = source;
        true
    }

    /// Whether `token` proves knowledge of the current secret, or of a retired one still in grace.
    pub fn accepts(&self, community: &CommunityId, token: Option<[u8; 32]>, now: u64) -> bool {
        let Some(token) = token else {
            return false;
        };
        token == community_token(&self.secret, community)
            || self
                .retired
                .iter()
                .any(|r| r.until_ms > now && token == community_token(&r.secret, community))
    }
}

/// The stored keys of each community, under its directory.
pub(crate) fn load(
    paths: &Paths,
    ids: impl Iterator<Item = CommunityId>,
) -> Result<BTreeMap<CommunityId, Keys>> {
    let mut out = BTreeMap::new();
    for id in ids {
        match std::fs::read(paths.access(&id)) {
            Ok(bytes) => {
                out.insert(id, decode_keys(&bytes)?);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Reads a stored `Keys`, accepting the formats older versions wrote, so an upgrade never fails
/// to start. v0.1.1 stored the bare 32-byte invite secret (generation 0). Builds before the
/// rotation tie-break stored `Keys` without `source`.
fn decode_keys(bytes: &[u8]) -> Result<Keys> {
    #[derive(Deserialize)]
    struct KeysV2 {
        generation: u64,
        secret: [u8; 32],
        retired: Vec<Retired>,
    }
    if let Ok(keys) = postcard::from_bytes::<Keys>(bytes) {
        return Ok(keys);
    }
    if let Ok(v2) = postcard::from_bytes::<KeysV2>(bytes) {
        return Ok(Keys { generation: v2.generation, secret: v2.secret, retired: v2.retired, source: [0; 32] });
    }
    let secret: [u8; 32] = postcard::from_bytes(bytes)
        .map_err(|e| CoreError::Encoding(format!("community access file: {e}")))?;
    Ok(Keys::from_invite(0, secret))
}

/// Stores a community's keys, so they survive restarts.
pub(crate) fn save(paths: &Paths, id: &CommunityId, keys: &Keys) -> Result<()> {
    write_bin(&paths.access(id), keys)
}

/// Sender side of a request: the request plus the token for its community, if it has one.
#[derive(Serialize)]
pub(crate) struct OutEnvelope<'a> {
    pub token: Option<[u8; 32]>,
    pub req: &'a Request,
}

/// Responder side of a request.
#[derive(Deserialize)]
pub(crate) struct InEnvelope {
    pub token: Option<[u8; 32]>,
    pub req: Request,
}

/// Checks a request's token. Requests without a community pass unchecked. A banned device is
/// refused for every community request, even with a valid token.
pub(crate) fn check(
    keys_of: impl Fn(&CommunityId) -> Option<Keys>,
    banned_for: impl Fn(&CommunityId) -> bool,
    env: &InEnvelope,
    now: u64,
) -> Result<()> {
    let Some(community) = env.req.community() else {
        return Ok(());
    };
    let Some(keys) = keys_of(&community) else {
        return Err(CoreError::NotFound(format!("community {community}")));
    };
    if banned_for(&community) || !keys.accepts(&community, env.token, now) {
        return Err(CoreError::Invalid("access denied".into()));
    }
    Ok(())
}

impl crate::engine::Engine {
    /// The keys we hold for a community, if any.
    pub(crate) fn access_keys(&self, community: CommunityId) -> Option<Keys> {
        self.read().access.get(&community).cloned()
    }

    /// The current community secret we hold, if any. Set at create, or from an invite when joining.
    pub(crate) fn access_secret(&self, community: CommunityId) -> Option<[u8; 32]> {
        self.access_keys(community).map(|k| k.secret)
    }

    /// Encodes a request for a peer, with the token for its community. Every outbound request goes
    /// through here.
    pub(crate) fn request_bytes(&self, req: &Request) -> Result<Vec<u8>> {
        let token = req
            .community()
            .and_then(|c| self.access_secret(c).map(|s| community_token(&s, &c)));
        postcard::to_allocvec(&OutEnvelope { token, req })
            .map_err(|e| CoreError::Encoding(e.to_string()))
    }

    /// Checks an inbound request's token, and that the sending device is not banned.
    pub(crate) fn check_request(&self, env: &InEnvelope, from: dissent_identity::DeviceId) -> Result<()> {
        let now = crate::engine::now_ms();
        check(
            |c| self.access_keys(*c),
            |c| self.device_banned_in(from, *c),
            env,
            now,
        )
    }

    /// Whether `device` belongs to a banned root of `community`.
    pub(crate) fn device_banned_in(&self, device: dissent_identity::DeviceId, community: CommunityId) -> bool {
        let w = self.read();
        let Some(root) = w.directory.root_of(&device) else {
            return false;
        };
        w.communities
            .get(&community)
            .is_some_and(|c| c.state().banned.contains(&root))
    }
}

#[cfg(test)]
mod tests;
