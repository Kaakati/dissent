//! Channel settings and invite management. Changes are sealed control events that every member
//! replays under the same rules, so the owner, admins and moderators decide what a channel is.
//! The engine checks the same rules first, so a device gets a clear error instead of a silent
//! no-op. Replay still decides for every device.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dissent_events::state::{BITRATE_RANGE, MAX_ORDERED_CHANNELS, MAX_USER_LIMIT, SLOWMODE_STEPS};
use dissent_events::{ChannelId, ChannelVisibility, Payload};
use dissent_identity::CommunityId;
use serde::{Deserialize, Serialize};

use crate::engine::{clean_text, now_ms, Engine};
use crate::error::{CoreError, Result};
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
    w.paths.read_bin(&invites_path(w)).ok().flatten().unwrap_or_default()
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
            return Err(CoreError::Invalid(format!(
                "bitrate must be between {} and {} kbps",
                BITRATE_RANGE.start(),
                BITRATE_RANGE.end()
            )));
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
        for payload in split_for_older_clients(payload) {
            self.author_publish(community, ChannelId::CONTROL, payload).await?;
        }
        self.emit(crate::views::EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

}

/// The most a voice channel's bitrate could be set to before boosts. A client from before them
/// rejects a higher setting as malformed. See `docs/design/12-boosts.md`.
pub(crate) const LEGACY_MAX_BITRATE_KBPS: u32 = 96;

/// An update that raises the bitrate above what older clients accept, and also changes other
/// fields, becomes two: the other fields first, then the bitrate alone. An older client then
/// rejects only the bitrate event and still applies the rest. Every other update stays one event.
pub(crate) fn split_for_older_clients(payload: Payload) -> Vec<Payload> {
    let Payload::ChannelUpdate {
        channel,
        name,
        slowmode_s,
        visibility,
        bitrate_kbps: Some(bitrate),
        user_limit,
        relay,
    } = payload
    else {
        return vec![payload];
    };
    let others = name.is_some() || slowmode_s.is_some() || visibility.is_some() || user_limit.is_some() || relay.is_some();
    if bitrate <= LEGACY_MAX_BITRATE_KBPS || !others {
        return vec![Payload::ChannelUpdate {
            channel,
            name,
            slowmode_s,
            visibility,
            bitrate_kbps: Some(bitrate),
            user_limit,
            relay,
        }];
    }
    vec![
        Payload::ChannelUpdate {
            channel,
            name,
            slowmode_s,
            visibility,
            bitrate_kbps: None,
            user_limit,
            relay,
        },
        Payload::ChannelUpdate {
            channel,
            name: None,
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: Some(bitrate),
            user_limit: None,
            relay: None,
        },
    ]
}

impl Engine {
    /// Sets the order the server lists its channels in, for everyone: `text` and `voice` top first. Owner and
    /// admins only. Each id must be a channel of that kind. A channel left out keeps its place after the listed
    /// ones, as replay places it.
    pub async fn reorder_channels(&self, community: CommunityId, text: Vec<ChannelId>, voice: Vec<ChannelId>) -> Result<()> {
        self.may_manage(community)?;
        {
            let w = self.read();
            let c = w
                .communities
                .get(&community)
                .ok_or_else(|| crate::engine::not_found(&community))?;
            let state = c.state();
            if text.len() + voice.len() > MAX_ORDERED_CHANNELS {
                return Err(CoreError::Invalid(format!("an order lists at most {MAX_ORDERED_CHANNELS} channels")));
            }
            let mut seen = std::collections::BTreeSet::new();
            for (ids, is_voice) in [(&text, false), (&voice, true)] {
                for id in ids {
                    if !state.channels.contains_key(id) {
                        return Err(CoreError::NotFound(format!("channel {id}")));
                    }
                    if state.voice_channels.contains(id) != is_voice {
                        return Err(CoreError::Invalid("text and voice channels are ordered apart".into()));
                    }
                    if !seen.insert(*id) {
                        return Err(CoreError::Invalid(format!("channel {id} is listed twice")));
                    }
                }
            }
        }
        self.author_publish(community, ChannelId::CONTROL, Payload::ChannelOrder { text, voice })
            .await?;
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
        let (paths, path) = (w.paths.clone(), invites_path(&w));
        let mut all = local_invites(&w);
        drop(w);
        all.entry(community).or_default().push(LocalInvite { id, link, created_ms });
        paths.write_bin(&path, &all)?;
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

#[cfg(test)]
mod split_tests {
    use super::*;
    use dissent_events::{replay, Dag, Event, EventBody, Hlc};
    use dissent_identity::{CommunityId, DeviceKey, RootKey};

    fn update(name: Option<&str>, bitrate: Option<u32>, channel: ChannelId) -> Payload {
        Payload::ChannelUpdate {
            channel,
            name: name.map(Into::into),
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: bitrate,
            user_limit: None,
            relay: None,
        }
    }

    #[test]
    fn only_a_raised_bitrate_with_other_fields_is_split() {
        let ch = ChannelId([1; 32]);
        assert_eq!(split_for_older_clients(update(Some("a"), Some(96), ch)).len(), 1, "96 is fine for older clients");
        assert_eq!(split_for_older_clients(update(None, Some(128), ch)).len(), 1, "a bitrate alone stays one event");
        assert_eq!(split_for_older_clients(update(Some("a"), None, ch)).len(), 1);
        assert_eq!(
            split_for_older_clients(update(Some("a"), Some(128), ch)),
            vec![update(Some("a"), None, ch), update(None, Some(128), ch)],
            "the other fields first, then the bitrate alone"
        );
    }

    /// A client from before boosts rejects any update with a bitrate over 96. Replay keeps going after
    /// a rejected event, so dropping those updates models it.
    fn older_client_keeps(payload: &Payload) -> bool {
        !matches!(payload, Payload::ChannelUpdate { bitrate_kbps: Some(b), .. } if *b > LEGACY_MAX_BITRATE_KBPS)
    }

    #[test]
    fn a_rename_sent_with_128_kbps_still_renames_on_an_older_client() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let community = CommunityId([2; 32]);
        let mut parents = Vec::new();
        let mut wall = 0;
        let mut sign = |payload: Payload, parents: &mut Vec<dissent_events::EventId>| {
            wall += 1;
            let ev = Event::sign(
                EventBody {
                    community,
                    channel: ChannelId::CONTROL,
                    author: root.id(),
                    device: device.id(),
                    parents: std::mem::take(parents),
                    hlc: Hlc { wall_ms: wall, counter: 0 },
                    payload,
                },
                &device,
            );
            parents.push(ev.id);
            ev
        };
        let join = sign(Payload::Join { display_name: "owner".into() }, &mut parents);
        let create = sign(Payload::VoiceChannelCreate { name: "lounge".into() }, &mut parents);
        let voice = ChannelId(create.id.0);
        let split: Vec<Event> = split_for_older_clients(update(Some("lounge-hifi"), Some(128), voice))
            .into_iter()
            .map(|p| sign(p, &mut parents))
            .collect();
        assert_eq!(split.len(), 2);

        let build = |keep: &dyn Fn(&Payload) -> bool| {
            let mut dag = Dag::new();
            for ev in [&join, &create].into_iter().chain(split.iter()) {
                if keep(&ev.body.payload) {
                    dag.insert(ev.clone()).unwrap();
                }
            }
            replay(root.id(), &dag)
        };
        // The older client drops only the bitrate event, and keeps the rename.
        let old = build(&older_client_keeps);
        assert_eq!(old.channels[&voice], "lounge-hifi");
        assert_eq!(old.settings(&voice).bitrate_kbps, 64, "the old default stays");
        // A current client applies both.
        let new = build(&|_| true);
        assert_eq!(new.channels[&voice], "lounge-hifi");
        assert_eq!(new.settings(&voice).bitrate_kbps, 128);
        assert!(new.rejected.is_empty());
    }
}
