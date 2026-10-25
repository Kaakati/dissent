//! Channel settings and invite management. Changes are sealed control events that every member
//! replays under the same rules, so the owner, admins and moderators decide what a channel is.
//! The engine checks the same rules first, so a device gets a clear error instead of a silent
//! no-op. Replay still decides for every device.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dissent_events::state::{BITRATE_RANGE, MAX_USER_LIMIT, SLOWMODE_STEPS};
use dissent_events::{ChannelId, ChannelVisibility, Payload};
use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use crate::engine::{clean_text, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, write_bin};
use crate::views::InviteView;
use crate::world::World;

/// A change to a channel. Fields left `None` stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub slowmode_s: Option<u32>,
    pub visibility: Option<ChannelVisibility>,
    pub bitrate_kbps: Option<u32>,
    pub user_limit: Option<u32>,
    /// A relay from the community's relays, or empty for automatic.
    pub relay: Option<String>,
}

/// An invite this device made. Kept here because its link carries the community secret, which
/// the community's replay does not hold in a form it can list.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LocalInvite {
    id: [u8; 16],
    link: String,
    created_ms: u64,
}

type LocalInvites = BTreeMap<CommunityId, Vec<LocalInvite>>;

fn invites_path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("local_invites.bin")
}

fn local_invites(w: &World) -> LocalInvites {
    read_bin(&invites_path(w)).ok().flatten().unwrap_or_default()
}

fn owner_or_admin(role: &str) -> bool {
    role == "owner" || role == "admin"
}

impl Engine {
    /// True when this device may change channels and invites, by its replayed role.
    fn may_manage(&self, community: CommunityId) -> Result<()> {
        let w = self.read();
        let c = w
            .communities
            .get(&community)
            .ok_or_else(|| crate::engine::not_found(&community))?;
        if owner_or_admin(c.role_of(&self.inner.me)) {
            Ok(())
        } else {
            Err(CoreError::Rejected("Only the owner and admins can change this".into()))
        }
    }

    /// Changes a channel's settings. Only the owner and admins may. The values are checked here
    /// the same way replay checks them.
    pub async fn update_channel(
        &self,
        community: CommunityId,
        channel: ChannelId,
        patch: ChannelPatch,
    ) -> Result<()> {
        self.may_manage(community)?;
        let is_voice = {
            let w = self.read();
            let c = w
                .communities
                .get(&community)
                .ok_or_else(|| crate::engine::not_found(&community))?;
            if !c.state().channels.contains_key(&channel) {
                return Err(CoreError::NotFound(format!("channel {channel}")));
            }
            c.state().voice_channels.contains(&channel)
        };
        let name = match patch.name {
            Some(name) => Some(clean_text(&name, "channel name")?),
            None => None,
        };
        if let Some(seconds) = patch.slowmode_s {
            if is_voice || !SLOWMODE_STEPS.contains(&seconds) {
                return Err(CoreError::Invalid("slowmode is for text channels, in a listed step".into()));
            }
        }
        let voice_field = patch.bitrate_kbps.is_some() || patch.user_limit.is_some() || patch.relay.is_some();
        if voice_field && !is_voice {
            return Err(CoreError::Invalid("bitrate, user limit and relay are for voice channels".into()));
        }
        if patch.bitrate_kbps.is_some_and(|b| !BITRATE_RANGE.contains(&b)) {
            return Err(CoreError::Invalid("bitrate must be between 8 and 96 kbps".into()));
        }
        if patch.user_limit.is_some_and(|u| u > MAX_USER_LIMIT) {
            return Err(CoreError::Invalid("user limit must be at most 99".into()));
        }
        let payload = Payload::ChannelUpdate {
            channel,
            name,
            slowmode_s: patch.slowmode_s,
            visibility: patch.visibility,
            bitrate_kbps: patch.bitrate_kbps,
            user_limit: patch.user_limit,
            relay: patch.relay,
        };
        self.author_publish(community, ChannelId::CONTROL, payload).await?;
        self.emit(crate::views::EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

    /// Deletes a channel and its messages for everyone. Owner and admins only.
    pub async fn delete_channel(&self, community: CommunityId, channel: ChannelId) -> Result<()> {
        self.may_manage(community)?;
        self.author_publish(community, ChannelId::CONTROL, Payload::ChannelDelete { channel })
            .await?;
        self.emit(crate::views::EngineEvent::CommunitiesChanged);
        Ok(())
    }

    /// Stops new joins through an invite. Members already in the community stay. The link can
    /// still have leaked with the secret it carries, so the note in the UI says to ban or rotate.
    pub async fn revoke_invite(&self, community: CommunityId, invite_id: [u8; 16]) -> Result<()> {
        self.may_manage(community)?;
        self.author_publish(community, ChannelId::CONTROL, Payload::InviteRevoke { invite_id })
            .await?;
        Ok(())
    }

    /// The invites this device made for `community`, with their uses and state as the replay
    /// holds them.
    pub fn list_invites(&self, community: CommunityId) -> Result<Vec<InviteView>> {
        let w = self.read();
        let c = w
            .communities
            .get(&community)
            .ok_or_else(|| crate::engine::not_found(&community))?;
        let mine = local_invites(&w).remove(&community).unwrap_or_default();
        Ok(mine
            .into_iter()
            .map(|inv| {
                let state = c.state().invites.get(&inv.id);
                InviteView {
                    id: hex(&inv.id),
                    link: inv.link,
                    created_ms: inv.created_ms,
                    expires_ms: state.and_then(|s| s.expires_ms),
                    uses: state.map_or(0, |s| s.uses),
                    revoked: state.is_some_and(|s| s.revoked),
                }
            })
            .collect())
    }

    /// Remembers an invite this device made, so it can be listed later.
    pub(crate) fn record_invite(&self, community: CommunityId, id: [u8; 16], link: String, created_ms: u64) -> Result<()> {
        let w = self.read();
        let path = invites_path(&w);
        let mut all = local_invites(&w);
        drop(w);
        all.entry(community).or_default().push(LocalInvite { id, link, created_ms });
        write_bin(&path, &all)?;
        Ok(())
    }

    /// Refuses to join through an invite this device can already see is revoked or expired. The
    /// members refuse it too, by replay, so this only spares a join that would be ignored.
    pub(crate) fn refuse_known_dead_invite(&self, community: CommunityId, id: [u8; 16]) -> Result<()> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Ok(());
        };
        let Some(invite) = c.state().invites.get(&id) else {
            return Ok(());
        };
        if invite.revoked {
            return Err(CoreError::Rejected("This invite was revoked".into()));
        }
        if invite.expires_ms.is_some_and(|at| now_ms() >= at) {
            return Err(CoreError::Rejected("This invite has expired".into()));
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads an invite id as [`InviteView::id`] writes it: 32 lowercase hex digits.
pub fn parse_invite_id(text: &str) -> Result<[u8; 16]> {
    let bad = || CoreError::Invalid("an invite id is 32 hex digits".into());
    if text.len() != 32 {
        return Err(bad());
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2).ok_or_else(bad)?, 16).map_err(|_| bad())?;
    }
    Ok(out)
}
