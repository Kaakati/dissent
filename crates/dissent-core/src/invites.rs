//! Group invite links. The creator of a group signs a link that expires after seven days. A
//! holder of the link sends a signed join request to the creator. The creator approves it, and the
//! usual group add follows. The creator can revoke every outstanding link by bumping a version.
//!
//! Link format: `dissent://group/<base64url of a postcard GroupInvite>`.

use dissent_identity::{DeviceCert, RootId, Signature};
use serde::{Deserialize, Serialize};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::groups::{ConvId, ConvKind};
use crate::views::{ConversationSummary, EngineEvent};
use crate::world::World;

/// How long a group invite link stays valid.
pub const GROUP_INVITE_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const LINK_PREFIX: &str = "dissent://group/";
const INVITE_DOMAIN: &[u8] = b"dissent/group-invite/v1";
const REQUEST_DOMAIN: &[u8] = b"dissent/group-join/v1";

/// A signed link to join a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInvite {
    pub conv: ConvId,
    /// The creator's device, certified by the creator's root.
    pub inviter: DeviceCert,
    /// The invite version. Revoking bumps the creator's version, which voids older links.
    pub version: u64,
    pub expires_ms: u64,
    /// The creator's address, so a holder can reach it.
    pub addr: String,
    pub sig: Signature,
}

fn invite_bytes(conv: &ConvId, version: u64, expires_ms: u64) -> Result<Vec<u8>> {
    let mut out = INVITE_DOMAIN.to_vec();
    out.extend(crate::rpc::encode(&(conv, version, expires_ms))?);
    Ok(out)
}

impl GroupInvite {
    /// Checks the inviter's cert, the signature, and expiry.
    pub fn verify(&self, now: u64) -> Result<()> {
        self.inviter
            .verify()
            .map_err(|e| CoreError::Invalid(format!("cert: {e}")))?;
        self.inviter
            .device
            .verify(&invite_bytes(&self.conv, self.version, self.expires_ms)?, &self.sig)
            .map_err(|e| CoreError::Invalid(format!("signature: {e}")))?;
        if now >= self.expires_ms {
            return Err(CoreError::Rejected("invite has expired".into()));
        }
        Ok(())
    }

    fn to_link(&self) -> Result<String> {
        let bytes = crate::rpc::encode(self)?;
        Ok(format!(
            "{LINK_PREFIX}{}",
            data_encoding::BASE64URL_NOPAD.encode(&bytes)
        ))
    }

    /// Parses a `dissent://group/...` link. Only the shape is checked here. `verify` checks the rest.
    pub fn parse(link: &str) -> Result<Self> {
        let body = link
            .trim()
            .strip_prefix(LINK_PREFIX)
            .ok_or_else(|| CoreError::Invalid("not a group invite link".into()))?;
        let bytes = data_encoding::BASE64URL_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| CoreError::Invalid("invalid group invite link".into()))?;
        crate::rpc::decode(&bytes)
    }
}

/// The signed join request a holder sends to the creator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JoinRequest {
    pub root: RootId,
    pub cert: DeviceCert,
    pub addr: String,
    pub ms: u64,
}

/// A join request waiting for the creator, as the UI lists it.
#[derive(Clone, Debug, Serialize)]
pub struct JoinRequestView {
    pub user: String,
    pub suffix: String,
    pub display_name: String,
    pub ms: u64,
}

fn request_bytes(conv: &ConvId, version: u64) -> Result<Vec<u8>> {
    let mut out = REQUEST_DOMAIN.to_vec();
    out.extend(crate::rpc::encode(&(conv, version))?);
    Ok(out)
}

/// The signature a holder puts on a join request.
pub(crate) fn sign_request(
    device: &dissent_identity::DeviceKey,
    conv: &ConvId,
    version: u64,
) -> Result<Signature> {
    Ok(device.sign(&request_bytes(conv, version)?))
}

/// Checks a join request sent for `conv` against the invite it used.
pub(crate) fn verify_request(
    cert: &DeviceCert,
    conv: &ConvId,
    version: u64,
    sig: &Signature,
) -> Result<()> {
    cert.verify()
        .map_err(|e| CoreError::Invalid(format!("cert: {e}")))?;
    cert.device
        .verify(&request_bytes(conv, version)?, sig)
        .map_err(|e| CoreError::Invalid(format!("signature: {e}")))
}

/// The creator's current invite version for a group.
pub(crate) fn current_version(w: &World, conv: &ConvId) -> u64 {
    w.conv_book.invite_version.get(conv).copied().unwrap_or(0)
}

impl Engine {
    /// A signed link for a group, valid for seven days. Only the creator may issue one.
    pub fn create_group_invite(&self, conv: ConvId) -> Result<String> {
        let (version, addr) = {
            let w = self.read();
            let meta = w
                .conv_book
                .convs
                .get(&conv)
                .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))?;
            if meta.kind != ConvKind::Group || meta.left {
                return Err(CoreError::Invalid("not a group you belong to".into()));
            }
            if meta.creator != self.inner.me {
                return Err(CoreError::Rejected("only the creator issues invites".into()));
            }
            let addr = w
                .my_addr
                .as_ref()
                .ok_or_else(|| CoreError::Invalid("this device has no address yet".into()))?
                .to_string();
            (current_version(&w, &conv), addr)
        };
        let expires_ms = now_ms() + GROUP_INVITE_TTL_MS;
        let sig = self
            .inner
            .identity
            .device
            .sign(&invite_bytes(&conv, version, expires_ms)?);
        GroupInvite {
            conv,
            inviter: self.inner.identity.cert.clone(),
            version,
            expires_ms,
            addr,
            sig,
        }
        .to_link()
    }

    /// Voids every invite issued for a group so far. Only the creator may do this.
    pub fn revoke_group_invites(&self, conv: ConvId) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        let meta = w
            .conv_book
            .convs
            .get(&conv)
            .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))?;
        if meta.creator != self.inner.me {
            return Err(CoreError::Rejected("only the creator revokes invites".into()));
        }
        let next = current_version(w, &conv) + 1;
        w.conv_book.invite_version.insert(conv, next);
        w.conv_book.save(&w.paths)
    }

    /// Asks the creator to add us to a group, using an invite link. The creator still approves.
    pub async fn join_group_by_invite(&self, link: &str) -> Result<()> {
        let invite = GroupInvite::parse(link)?;
        invite.verify(now_ms())?;
        let inviter_addr: dissent_net::PeerAddr = invite
            .addr
            .parse()
            .map_err(|_| CoreError::Invalid("invite has a bad address".into()))?;
        {
            let mut guard = self.write();
            guard.directory.learn(&invite.inviter);
            self.learn_address(&mut guard, inviter_addr.clone());
        }
        {
            let mut guard = self.write();
            guard.conv_book.invite_joins.insert(
                invite.conv,
                InviteJoin {
                    device: invite.inviter.device,
                    root: invite.inviter.root,
                    expires_ms: invite.expires_ms,
                },
            );
            guard.conv_book.save(&guard.paths)?;
        }
        let sig = sign_request(&self.inner.identity.device, &invite.conv, invite.version)?;
        let my_addr = self
            .read()
            .my_addr
            .as_ref()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let packet = crate::groups::ConvPacket::JoinRequest {
            conv: invite.conv,
            version: invite.version,
            cert: self.inner.identity.cert.clone(),
            addr: my_addr,
            sig,
        };
        let bytes = self.request_bytes(&crate::rpc::Request::Conv(Box::new(packet)))?;
        let reply = self
            .node()
            .request(invite.inviter.device, bytes)
            .await
            .map_err(|e| CoreError::Invalid(format!("could not reach the creator: {e}")))?;
        match crate::rpc::decode::<crate::rpc::Response>(&reply)? {
            crate::rpc::Response::Ack => Ok(()),
            crate::rpc::Response::Refused(reason) | crate::rpc::Response::Error(reason) => {
                Err(CoreError::Rejected(reason))
            }
            _ => Err(CoreError::Invalid("unexpected reply from the creator".into())),
        }
    }

    /// Join requests waiting for approval in a group we created.
    pub fn group_join_requests(&self, conv: ConvId) -> Vec<JoinRequestView> {
        let w = self.read();
        w.conv_book
            .join_requests
            .get(&conv)
            .into_iter()
            .flatten()
            .map(|r| JoinRequestView {
                user: r.root.to_string(),
                suffix: r.root.suffix(),
                display_name: w.display_name(&r.root),
                ms: r.ms,
            })
            .collect()
    }

    /// Approves a join request. The person is added to the group, as with `group_add`.
    pub async fn approve_group_join(&self, conv: ConvId, user: RootId) -> Result<ConversationSummary> {
        {
            let mut guard = self.write();
            let w = &mut *guard;
            let list = w.conv_book.join_requests.entry(conv).or_default();
            let before = list.len();
            list.retain(|r| r.root != user);
            if list.len() == before {
                return Err(CoreError::NotFound(format!("join request from {user}")));
            }
            w.conv_book.save(&w.paths)?;
        }
        self.group_add(conv, vec![user]).await
    }

    /// Declines a join request.
    pub fn decline_group_join(&self, conv: ConvId, user: RootId) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        if let Some(list) = w.conv_book.join_requests.get_mut(&conv) {
            list.retain(|r| r.root != user);
        }
        w.conv_book.save(&w.paths)
    }

    /// Handles a join request sent by a holder of an invite. Records it for the creator to approve.
    pub(crate) fn on_join_request(
        &self,
        conv: ConvId,
        version: u64,
        cert: DeviceCert,
        addr: String,
        sig: Signature,
    ) -> Result<()> {
        verify_request(&cert, &conv, version, &sig)?;
        let requester = cert.root;
        let user_event = {
            let mut guard = self.write();
            let w = &mut *guard;
            let meta = w
                .conv_book
                .convs
                .get(&conv)
                .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))?
                .clone();
            if meta.kind != ConvKind::Group || meta.creator != self.inner.me {
                return Err(CoreError::Rejected("only the creator takes join requests".into()));
            }
            if version != current_version(w, &conv) {
                return Err(CoreError::Rejected("the invite was revoked".into()));
            }
            if w.contact_book.blocked.contains(&requester) {
                return Ok(());
            }
            if meta.members.contains(&requester) {
                return Ok(());
            }
            w.directory.learn(&cert);
            if let Ok(peer) = addr.parse::<dissent_net::PeerAddr>() {
                self.learn_address(w, peer);
            }
            let list = w.conv_book.join_requests.entry(conv).or_default();
            list.retain(|r| r.root != requester);
            list.push(JoinRequest {
                root: requester,
                cert: cert.clone(),
                addr,
                ms: now_ms(),
            });
            w.conv_book.save(&w.paths)?;
            requester
        };
        self.emit(EngineEvent::GroupJoinRequest {
            conversation: conv.to_string(),
            user: user_event.to_string(),
        });
        Ok(())
    }
}

/// A join we asked for with an invite. It lets exactly one welcome in: the one from the device the
/// link names, or from the creator's root. It ends when the join completes or the link expires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InviteJoin {
    pub device: dissent_identity::DeviceId,
    pub root: RootId,
    pub expires_ms: u64,
}

/// Whether a welcome's sender is the one the invite names: its device, or the creator's root.
pub(crate) fn join_allowed(join: &InviteJoin, cert: &DeviceCert) -> bool {
    cert.device == join.device || cert.root == join.root
}

impl Engine {
    /// Joins we asked for with an invite and have not completed. Expired ones are dropped first.
    pub fn pending_group_joins(&self) -> usize {
        let now = now_ms();
        let mut guard = self.write();
        let w = &mut *guard;
        w.conv_book.invite_joins.retain(|_, j| now < j.expires_ms);
        w.conv_book.invite_joins.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::{DeviceKey, RootKey};

    fn signed_invite(issued: u64, expires_ms: u64) -> GroupInvite {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), issued);
        let conv = ConvId([4; 32]);
        let sig = device.sign(&invite_bytes(&conv, 0, expires_ms).unwrap());
        GroupInvite {
            conv,
            inviter: cert,
            version: 0,
            expires_ms,
            addr: String::new(),
            sig,
        }
    }

    #[test]
    fn a_live_invite_verifies_and_an_expired_one_is_refused() {
        let now = 1_000_000;
        let invite = signed_invite(now, now + GROUP_INVITE_TTL_MS);
        assert!(invite.verify(now).is_ok());
        assert!(invite.verify(now + GROUP_INVITE_TTL_MS).is_err(), "an expired link is refused");
    }

    #[test]
    fn only_the_named_device_or_the_creators_root_may_welcome() {
        let invite = signed_invite(1, 10);
        let join = InviteJoin {
            device: invite.inviter.device,
            root: invite.inviter.root,
            expires_ms: 10,
        };
        assert!(join_allowed(&join, &invite.inviter));
        let stranger = RootKey::generate().certify(DeviceKey::generate().id(), 1);
        assert!(!join_allowed(&join, &stranger));
    }
}
