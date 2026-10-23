//! The engine's state: profile, contacts, communities, direct messages and addresses.
//! `Engine` guards one `World` with a lock. Nothing here touches the network.

use crate::community::Community;
use crate::directory::{AddressBook, Directory, StoredAddr};
use crate::error::{CoreError, Result};
use crate::store::{read_bin, read_records, write_bin, Paths};
use crate::views::{
    ChannelInfo, CommunityRef, CommunitySummary, CommunityView, HandleView, MemberInfo,
    MessageItem, UserView,
};
use crate::wire::SignedAddr;
use dissent_discovery::ContactCard;
use dissent_events::{ChannelId, Hlc, SignedDm, SignedEvent};
use dissent_identity::{CommunityId, DeviceId, Genesis, Identity, RootId};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::str::FromStr;
use tracing::warn;

#[derive(Serialize, Deserialize, Default)]
struct Profile {
    display_name: String,
}

pub(crate) struct World {
    pub me: RootId,
    pub device: DeviceId,
    pub paths: Paths,
    /// The name we chose. Empty until set, in which case `my_name` falls back to `anon·<suffix>`.
    pub profile_name: String,
    pub contacts: Vec<ContactCard>,
    pub communities: BTreeMap<CommunityId, Community>,
    /// Direct messages keyed by the other party, in either direction.
    pub dms: BTreeMap<RootId, Vec<SignedDm>>,
    /// Outgoing direct messages not yet acknowledged.
    pub outbox: Vec<SignedDm>,
    pub directory: Directory,
    pub addresses: AddressBook,
    /// Our own dialable address. Set once the endpoint is up.
    pub my_addr: Option<PeerAddr>,
    /// Timestamp of our latest signed address. Each new one is larger.
    pub my_addr_ts: u64,
}

impl World {
    /// Reads everything under `paths`. Communities that fail to load are skipped with a warning.
    pub fn load(paths: Paths, identity: &Identity) -> Result<World> {
        let profile: Profile = read_bin(&paths.profile())?.unwrap_or_default();
        let contacts: Vec<ContactCard> = read_bin(&paths.contacts())?.unwrap_or_default();
        let addrs: Vec<StoredAddr> = read_bin(&paths.addresses())?.unwrap_or_default();
        let my_addr_ts = read_bin::<SignedAddr>(&paths.own_addr())?
            .map(|own| own.ts_ms)
            .unwrap_or(0);
        let mut directory = Directory::default();
        directory.learn(&identity.cert);
        for card in &contacts {
            directory.learn(&card.cert);
        }
        let communities = load_communities(&paths, &mut directory);
        let dms = load_dms(&paths, &mut directory);
        let outbox: Vec<SignedDm> = read_bin::<Vec<SignedDm>>(&paths.outbox())?
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.verify().is_ok())
            .collect();
        for dm in &outbox {
            directory.learn(&dm.cert);
        }
        Ok(World {
            me: identity.root_id(),
            device: identity.device_id(),
            paths,
            profile_name: profile.display_name,
            contacts,
            communities,
            dms,
            outbox,
            directory,
            addresses: AddressBook::from_list(addrs),
            my_addr: None,
            my_addr_ts,
        })
    }

    /// The newest HLC among all stored events, if any. Seeds the clock on startup.
    pub fn max_hlc(&self) -> Option<Hlc> {
        self.communities.values().filter_map(|c| c.max_hlc()).max()
    }

    pub fn save_profile(&self) -> Result<()> {
        write_bin(
            &self.paths.profile(),
            &Profile {
                display_name: self.profile_name.clone(),
            },
        )
    }

    pub fn save_contacts(&self) -> Result<()> {
        write_bin(&self.paths.contacts(), &self.contacts)
    }

    pub fn save_addresses(&self) -> Result<()> {
        write_bin(&self.paths.addresses(), &self.addresses.all())
    }

    pub fn save_outbox(&self) -> Result<()> {
        write_bin(&self.paths.outbox(), &self.outbox)
    }

    pub fn anon_name(&self) -> String {
        format!("anon·{}", self.me.suffix())
    }

    /// Our display name, or the anonymous name until we set one.
    pub fn my_name(&self) -> String {
        let trimmed = self.profile_name.trim();
        if trimmed.is_empty() {
            self.anon_name()
        } else {
            trimmed.to_string()
        }
    }

    pub fn is_contact(&self, user: &RootId) -> bool {
        self.contacts.iter().any(|c| c.cert.root == *user)
    }

    /// The name shown for `user`. Uses our own name for us. Otherwise it is the latest Join
    /// in a community we share, then the contact card name, then `anon·<suffix>`.
    pub fn display_name(&self, user: &RootId) -> String {
        if *user == self.me {
            return self.my_name();
        }
        let mut best: Option<(Hlc, &str)> = None;
        for c in self.shared_with(user) {
            let (Some(member), Some(hlc)) = (c.state().members.get(user), c.join_hlc.get(user))
            else {
                continue;
            };
            match best {
                Some((known, _)) if known >= *hlc => {}
                _ => best = Some((*hlc, member.display_name.as_str())),
            }
        }
        if let Some((_, name)) = best {
            return name.to_string();
        }
        if let Some(card) = self.contacts.iter().find(|c| c.cert.root == *user) {
            return card.display_name.clone();
        }
        format!("anon·{}", user.suffix())
    }

    /// Communities where both we and `user` are current members.
    fn shared_with(&self, user: &RootId) -> Vec<&Community> {
        self.communities
            .values()
            .filter(|c| c.is_member(&self.me) && c.is_member(user))
            .collect()
    }

    pub fn summary(&self, c: &Community) -> CommunitySummary {
        CommunitySummary {
            id: c.id().to_string(),
            name: c.name().to_string(),
            owner: c.owner().to_string(),
            my_role: c.role_of(&self.me).to_string(),
        }
    }

    pub fn community_view(&self, c: &Community) -> CommunityView {
        let channels = c
            .state()
            .channels
            .iter()
            .map(|(id, name)| ChannelInfo {
                id: id.to_string(),
                name: name.clone(),
            })
            .collect();
        let members = c
            .state()
            .members
            .iter()
            .filter(|(_, m)| !m.left)
            .map(|(root, _)| MemberInfo {
                root_id: root.to_string(),
                suffix: root.suffix(),
                display_name: self.display_name(root),
                handle: c.handle_of(root),
                role: c.role_of(root).to_string(),
                banned: c.state().banned.contains(root),
            })
            .collect();
        CommunityView {
            id: c.id().to_string(),
            name: c.name().to_string(),
            owner: c.owner().to_string(),
            my_role: c.role_of(&self.me).to_string(),
            channels,
            members,
        }
    }

    pub fn user_view(&self, user: &RootId) -> UserView {
        let shared = self.shared_with(user);
        UserView {
            root_id: user.to_string(),
            suffix: user.suffix(),
            display_name: self.display_name(user),
            is_contact: self.is_contact(user),
            handles: shared
                .iter()
                .map(|c| HandleView {
                    community_id: c.id().to_string(),
                    community_name: c.name().to_string(),
                    handle: c.handle_of(user),
                })
                .collect(),
            mutual_communities: shared
                .iter()
                .map(|c| CommunityRef {
                    id: c.id().to_string(),
                    name: c.name().to_string(),
                })
                .collect(),
        }
    }

    /// The newest `limit` messages in a channel, oldest first. Empty if the channel is unknown.
    pub fn message_items(
        &self,
        community: &CommunityId,
        channel: &ChannelId,
        limit: usize,
    ) -> Vec<MessageItem> {
        let Some(msgs) = self
            .communities
            .get(community)
            .and_then(|c| c.state().messages.get(channel))
        else {
            return Vec::new();
        };
        let start = msgs.len().saturating_sub(limit);
        msgs[start..]
            .iter()
            .map(|m| MessageItem {
                id: m.id.to_string(),
                author: m.author.to_string(),
                author_name: self.display_name(&m.author),
                text: m.text.clone(),
                ms: m.hlc.wall_ms,
                edited: m.edited,
                deleted: m.deleted,
            })
            .collect()
    }

    /// Devices of the current members of `c`, excluding our own root and device.
    pub fn member_devices(&self, c: &Community) -> Vec<DeviceId> {
        c.state()
            .members
            .iter()
            .filter(|(root, m)| !m.left && **root != self.me)
            .flat_map(|(root, _)| self.directory.devices_of(root))
            .filter(|device| *device != self.device)
            .collect()
    }
}

fn load_communities(paths: &Paths, directory: &mut Directory) -> BTreeMap<CommunityId, Community> {
    let mut out = BTreeMap::new();
    let Ok(entries) = fs::read_dir(paths.communities_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(id) = CommunityId::from_str(&name) else {
            warn!(dir = %name, "skipping community directory with an invalid name");
            continue;
        };
        match load_community(paths, &id, directory) {
            Ok(c) => {
                out.insert(id, c);
            }
            Err(e) => warn!(community = %id, error = %e, "skipping community that failed to load"),
        }
    }
    out
}

fn load_community(
    paths: &Paths,
    id: &CommunityId,
    directory: &mut Directory,
) -> Result<Community> {
    let genesis: Genesis = read_bin(&paths.genesis(id))?
        .ok_or_else(|| CoreError::NotFound(format!("genesis for community {id}")))?;
    genesis.verify()?;
    if genesis.community != *id {
        return Err(CoreError::Invalid("genesis belongs to another community".into()));
    }
    let valid: Vec<SignedEvent> = read_records::<SignedEvent>(&paths.events_log(id))?
        .into_iter()
        .filter(|se| se.verify().is_ok())
        .collect();
    for se in &valid {
        directory.learn(&se.cert);
    }
    Ok(Community::load(genesis, valid))
}

fn load_dms(paths: &Paths, directory: &mut Directory) -> BTreeMap<RootId, Vec<SignedDm>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = fs::read_dir(paths.dms_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "log") {
            continue;
        }
        let Some(peer) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| RootId::from_str(s).ok())
        else {
            continue;
        };
        match read_records::<SignedDm>(&path) {
            Ok(records) => {
                let valid: Vec<SignedDm> = records.into_iter().filter(|d| d.verify().is_ok()).collect();
                for dm in &valid {
                    directory.learn(&dm.cert);
                }
                if !valid.is_empty() {
                    out.insert(peer, valid);
                }
            }
            Err(e) => warn!(path = %path.display(), error = %e, "skipping unreadable DM log"),
        }
    }
    out
}
