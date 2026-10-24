//! Who may reach a community's traffic. Each community has a random 32-byte secret, given out in
//! its invites. The secret salts the gossip topic, so a device without it cannot join the topic.
//! It also keys a token that every community request carries, so a device without it cannot sync
//! with members. See `docs/design/06-threat-model.md`, finding 2.

use std::collections::BTreeMap;

use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::rpc::Request;
use crate::store::{read_bin, write_bin, Paths};

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

/// The stored secret of a community, under its directory.
pub(crate) fn load(
    paths: &Paths,
    ids: impl Iterator<Item = CommunityId>,
) -> Result<BTreeMap<CommunityId, [u8; 32]>> {
    let mut out = BTreeMap::new();
    for id in ids {
        if let Some(secret) = read_bin::<[u8; 32]>(&paths.access(&id))? {
            out.insert(id, secret);
        }
    }
    Ok(out)
}

/// Stores a community secret, so it survives restarts.
pub(crate) fn save(paths: &Paths, id: &CommunityId, secret: &[u8; 32]) -> Result<()> {
    write_bin(&paths.access(id), secret)
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

/// Checks a request's token against our secret for its community. Requests without a community
/// pass unchecked.
pub(crate) fn check(
    secret_of: impl Fn(&CommunityId) -> Option<[u8; 32]>,
    env: &InEnvelope,
) -> Result<()> {
    let Some(community) = env.req.community() else {
        return Ok(());
    };
    let Some(secret) = secret_of(&community) else {
        return Err(CoreError::NotFound(format!("community {community}")));
    };
    if env.token == Some(community_token(&secret, &community)) {
        Ok(())
    } else {
        Err(CoreError::Invalid("access denied".into()))
    }
}

impl crate::engine::Engine {
    /// The community secret we hold, if any. Set at create, or from an invite when joining.
    pub(crate) fn access_secret(&self, community: CommunityId) -> Option<[u8; 32]> {
        self.read().access.get(&community).copied()
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

    /// Checks an inbound request's token against our secret for its community.
    pub(crate) fn check_request(&self, env: &InEnvelope) -> Result<()> {
        check(|c| self.access_secret(*c), env)
    }
}

#[cfg(test)]
mod tests;
