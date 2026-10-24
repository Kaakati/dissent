//! The engine's state: profile, contacts, communities, direct messages and addresses.
//! `Engine` guards one `World` with a lock. Nothing here touches the network.

use crate::community::Community;

/// Characters of a replied-to message shown in its preview.
const REPLY_SNIPPET_CHARS: usize = 120;
/// Reactors named in a reaction tooltip.
const REACTION_NAMES: usize = 3;
use crate::identity_watch::IdentityChange;
use crate::directory::{AddressBook, Directory, StoredAddr};
use crate::error::{CoreError, Result};
use crate::store::{
    content_vault, migrate_plain_log, read_bin, read_records, read_sealed_records, write_bin, Paths,
};
use crate::views::{
    ChannelInfo, CommunityRef, CommunitySummary, CommunityView, HandleView, MemberInfo,
    MessageItem, ReplyPreview, UserView,
};
use crate::wire::SignedAddr;
use dissent_discovery::ContactCard;
use dissent_events::{ChannelId, EventId, Hlc, MessageView, SignedDm, SignedEvent};
use dissent_identity::{CommunityId, DeviceId, Genesis, Identity, RootId, Vault};
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Changes of identity seen for the people we talk to.
    pub identity_changes: Vec<IdentityChange>,
    /// Community secrets, from creating a community or from an invite.
    pub access: BTreeMap<CommunityId, crate::access::Keys>,
    /// Outgoing direct messages not yet acknowledged.
    pub outbox: Vec<SignedDm>,
    /// Contact requests and accepts, the DM privacy policy.
    pub contact_book: crate::contacts::ContactBook,
    /// Conversations and their packet queues.
    pub conv_book: crate::groups::ConvBook,
    /// Stored text messages, by conversation.
    pub conv_msgs: BTreeMap<crate::groups::ConvId, Vec<crate::groups::ConvMsg>>,
    /// Attachment downloads and mirror settings.
    pub attach: crate::attachments::AttachBook,
    /// Inbox service state: envelopes and key packages held for our root.
    pub inbox: crate::inbox::InboxBook,
    /// This device's own status, from `status.bin`.
    pub own: crate::presence::OwnStatus,
    /// Set by the desktop after a quiet stretch. Turns an online status into idle, and is never stored.
    pub auto_idle: bool,
    /// The newest presence beacon held for each person. See `presence.rs`.
    pub presence: BTreeMap<RootId, crate::presence::Heard>,
    pub directory: Directory,
    pub addresses: AddressBook,
    /// Our dialable address. Set once the endpoint is up.
    pub my_addr: Option<PeerAddr>,
    /// Timestamp of our latest signed address. Each new one is larger.
    pub my_addr_ts: u64,
    /// Our latest signed address, passed on to others and re-broadcast on the community topics.
    pub my_signed: Option<SignedAddr>,
}

impl World {
    /// Reads everything under `paths`. Communities that fail to load are skipped with a warning.
    pub fn load(paths: Paths, identity: &Identity, vault: &Vault) -> Result<World> {
        let profile: Profile = read_bin(&paths.profile())?.unwrap_or_default();
        let contacts: Vec<ContactCard> = read_bin(&paths.contacts())?.unwrap_or_default();
        let addrs: Vec<StoredAddr> = read_bin(&paths.addresses())?.unwrap_or_default();
        let my_signed: Option<SignedAddr> = read_bin(&paths.own_addr())?;
        let my_addr_ts = my_signed.as_ref().map_or(0, |own| own.ts_ms);
        let mut directory = Directory::default();
        directory.learn(&identity.cert);
        for card in &contacts {
            directory.learn(&card.cert);
        }
        let communities = load_communities(&paths, &mut directory, vault);
        let dms = load_dms(&paths, &mut directory);
        let outbox: Vec<SignedDm> = read_bin::<Vec<SignedDm>>(&paths.outbox())?
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.verify().is_ok())
            .collect();
        for dm in &outbox {
            directory.learn(&dm.cert);
        }
        let contact_book = crate::contacts::ContactBook::load(&paths)?;
        let conv_book = crate::groups::ConvBook::load(&paths)?;
        let conv_msgs = crate::groups::load_messages(&paths, &conv_book);
        let inbox = crate::inbox::InboxBook::load(&paths)?;
        let attach = crate::attachments::AttachBook::load(&paths)?;
        let access = crate::access::load(&paths, communities.keys().copied())?;
        let identity_changes: Vec<IdentityChange> =
            crate::store::read_records(&paths.identity_changes()).unwrap_or_default();
        let own = crate::presence::load_own(&paths);
        Ok(World {
            me: identity.root_id(),
            device: identity.device_id(),
            paths,
            profile_name: profile.display_name,
            contacts,
            communities,
            dms,
            identity_changes,
            access,
            outbox,
            contact_book,
            conv_book,
            conv_msgs,
            inbox,
            attach,
            own,
            auto_idle: false,
            presence: BTreeMap::new(),
            directory,
            addresses: AddressBook::from_list(addrs),
            my_addr: None,
            my_addr_ts,
            my_signed,
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
                voice: c.state().voice_channels.contains(id),
                id: id.to_string(),
                name: name.clone(),
            })
            .collect();
        let now = crate::engine::now_ms();
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
                status: self.presence_of(root, now),
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
        let fields = crate::local::user_fields(self, user);
        UserView {
            root_id: user.to_string(),
            suffix: user.suffix(),
            display_name: self.display_name(user),
            is_contact: self.is_contact(user),
            identity_changed_ms: crate::identity_watch::last_change(&self.identity_changes, user),
            key_changed_ms: fields.key_changed_ms,
            verified_ms: fields.verified_ms,
            note: fields.note,
            nickname: fields.nickname,
            ignored: fields.ignored,
            status: self.presence_of(user, crate::engine::now_ms()),
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
        let Some(c) = self.communities.get(community) else {
            return Vec::new();
        };
        let Some(msgs) = c.state().messages.get(channel) else {
            return Vec::new();
        };
        let start = msgs.len().saturating_sub(limit);
        let by_id: BTreeMap<EventId, &MessageView> = msgs.iter().map(|m| (m.id, m)).collect();
        let reaction_ops = reaction_index(c);
        msgs[start..]
            .iter()
            .map(|m| {
                // Sealed text is not in state. It comes from this device's content view.
                let decrypted = c.content.get(&m.id).map(|(_, text)| text.clone());
                let locked = m.sealed && !m.deleted && decrypted.is_none();
                let text = if m.sealed {
                    decrypted.unwrap_or_default()
                } else {
                    m.text.clone()
                };
                MessageItem {
                    id: m.id.to_string(),
                    author: m.author.to_string(),
                    author_name: self.display_name(&m.author),
                    attachments: self.attachment_views(&text),
                    embeds: crate::attachments::embeds_of(&text),
                    text: crate::attachments::text_of(&text),
                    ms: m.hlc.wall_ms,
                    edited: m.edited,
                    deleted: m.deleted,
                    locked,
                    reply_to: m
                        .reply_to
                        .and_then(|target| self.reply_preview(c, &by_id, target)),
                    reactions: self.reaction_views(&reaction_ops.get(&m.id).cloned().unwrap_or_default()),
                }
            })
            .collect()
    }

    /// A short view of the message `target`, as this device can read it. `None` when that message
    /// is not in the channel.
    fn reply_preview(
        &self,
        c: &Community,
        by_id: &BTreeMap<EventId, &MessageView>,
        target: EventId,
    ) -> Option<ReplyPreview> {
        let original = by_id.get(&target)?;
        let plain: Option<String> = if original.sealed {
            c.content.get(&target).map(|(_, text)| text.clone())
        } else {
            Some(original.text.clone())
        };
        let locked = original.sealed && !original.deleted && plain.is_none();
        let text = if original.deleted {
            None
        } else {
            plain.map(|t| t.chars().take(REPLY_SNIPPET_CHARS).collect())
        };
        Some(ReplyPreview {
            id: target.to_string(),
            author: original.author.to_string(),
            author_name: self.display_name(&original.author),
            text,
            locked,
        })
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

fn load_communities(
    paths: &Paths,
    directory: &mut Directory,
    vault: &Vault,
) -> BTreeMap<CommunityId, Community> {
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
        match load_community(paths, &id, directory, vault) {
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
    vault: &Vault,
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
    let mut community = Community::load(genesis, valid);
    community.content = load_content(paths, id, vault);
    Ok(community)
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

/// Decrypted content records. Each key keeps the record with the newest hlc.
fn load_content(paths: &Paths, id: &CommunityId, vault: &Vault) -> BTreeMap<EventId, (Hlc, String)> {
    let key = content_vault(vault, id);
    if let Err(e) = migrate_plain_log::<ContentRecord>(
        &paths.legacy_content_log(id),
        &paths.content_log(id),
        &key,
    ) {
        warn!(community = %id, error = %e, "could not migrate the plaintext content log");
    }
    let records: Vec<ContentRecord> = match read_sealed_records(&paths.content_log(id), &key) {
        Ok(records) => records,
        Err(e) => {
            warn!(community = %id, error = %e, "content log could not be read");
            Vec::new()
        }
    };
    let mut out: BTreeMap<EventId, (Hlc, String)> = BTreeMap::new();
    for rec in records {
        let newer = out.get(&rec.key).is_none_or(|(hlc, _)| rec.hlc > *hlc);
        if newer {
            out.insert(rec.key, (rec.hlc, rec.text));
        }
    }
    out
}

/// One line of `content.log`: decrypted text for a content key.
#[derive(Serialize, Deserialize)]
pub(crate) struct ContentRecord {
    pub key: EventId,
    pub hlc: Hlc,
    pub text: String,
}

/// A reaction event, read by members: its time, author, whether it adds the reaction, and the emoji.
pub(crate) type ReactionOp = (Hlc, RootId, bool, String);

/// Every reaction event in a community that this device can read, by the message it targets.
/// Non-members have no emoji for these, so they are left out.
pub(crate) fn reaction_index(c: &Community) -> BTreeMap<EventId, Vec<ReactionOp>> {
    let mut out: BTreeMap<EventId, Vec<ReactionOp>> = BTreeMap::new();
    for se in c.sealed_in_order() {
        let dissent_events::Payload::Sealed { kind, .. } = &se.event.body.payload else {
            continue;
        };
        let (target, react) = match kind {
            dissent_events::SealedKind::React { target } => (*target, true),
            dissent_events::SealedKind::Unreact { target } => (*target, false),
            _ => continue,
        };
        let Some((_, emoji)) = c.content.get(&se.event.id) else {
            continue;
        };
        out.entry(target)
            .or_default()
            .push((se.event.body.hlc, se.event.body.author, react, emoji.clone()));
    }
    out
}

/// The reactions on one message: the latest choice of each person for each emoji, counted, in the
/// order each emoji first gained a reaction.
pub(crate) fn reaction_summary(me: &RootId, ops: &[ReactionOp]) -> Vec<crate::views::ReactionView> {
    let mut latest: BTreeMap<(String, RootId), (Hlc, bool)> = BTreeMap::new();
    let mut first: BTreeMap<String, Hlc> = BTreeMap::new();
    for (hlc, actor, react, emoji) in ops {
        let entry = latest.entry((emoji.clone(), *actor)).or_insert((*hlc, *react));
        if *hlc >= entry.0 {
            *entry = (*hlc, *react);
        }
        if *react {
            let f = first.entry(emoji.clone()).or_insert(*hlc);
            if *hlc < *f {
                *f = *hlc;
            }
        }
    }
    let mut by_emoji: BTreeMap<String, BTreeSet<RootId>> = BTreeMap::new();
    for ((emoji, actor), (_, react)) in latest {
        if react {
            by_emoji.entry(emoji).or_default().insert(actor);
        }
    }
    let mut order: Vec<String> = by_emoji.keys().cloned().collect();
    order.sort_by_key(|e| first.get(e).copied());
    order
        .into_iter()
        .filter_map(|emoji| {
            let reactors = by_emoji.get(&emoji)?;
            Some(crate::views::ReactionView {
                count: reactors.len(),
                me: reactors.contains(me),
                emoji,
                names: Vec::new(),
            })
        })
        .collect()
}

impl World {
    /// The reactions on a message, with the names of up to `REACTION_NAMES` reactors.
    pub(crate) fn reaction_views(&self, ops: &[ReactionOp]) -> Vec<crate::views::ReactionView> {
        let mut views = reaction_summary(&self.me, ops);
        for view in &mut views {
            let reactors: Vec<RootId> = ops
                .iter()
                .filter(|(_, _, react, e)| *react && *e == view.emoji)
                .map(|(_, a, _, _)| *a)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(REACTION_NAMES)
                .collect();
            view.names = reactors.iter().map(|r| self.display_name(r)).collect();
        }
        views
    }
}
