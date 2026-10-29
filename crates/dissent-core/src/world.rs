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
    MentionView, MessageItem, ReplyPreview, UserView,
};
use crate::wire::SignedAddr;
use dissent_discovery::ContactCard;
use dissent_events::{ChannelId, EventId, Hlc, MessageView, Picture, SignedDm, SignedEvent};
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
    /// Packets of refused messages, kept for retry until they expire.
    pub refused: crate::groups::RefusedBook,
    /// Stored text messages, by conversation.
    pub conv_msgs: BTreeMap<crate::groups::ConvId, Vec<crate::groups::ConvMsg>>,
    /// Stored replies, reactions, edits and deletes, by conversation. See `conv_actions.rs`.
    pub conv_acts: BTreeMap<crate::groups::ConvId, Vec<crate::conv_actions::ConvAct>>,
    /// When each contact was added, and when this device first saw each person.
    pub contact_times: crate::contact_times::ContactTimes,
    pub mention_reads: crate::mention_reads::MentionReads,
    /// Attachment downloads and mirror settings.
    pub attach: crate::attachments::AttachBook,
    /// Attachments whose thumbnail is being fetched now. Not stored: a restart fetches again.
    pub thumb_fetching: std::collections::BTreeSet<String>,
    /// Per-provider counts of the downloads in this run. Not stored: a restart starts them again.
    pub source_counts: BTreeMap<String, crate::attachments::LiveSources>,
    /// Which scopes each blob held here belongs to, for the members-only blob server. Built at start.
    pub blob_scopes: crate::blob_gate::BlobScopes,
    /// Inbox service state: envelopes and key packages held for our root.
    pub inbox: crate::inbox::InboxBook,
    /// This device's own status, from `status.bin`.
    pub own: crate::presence::OwnStatus,
    /// Set by the desktop after a quiet stretch. Turns an online status into idle, and is never stored.
    pub auto_idle: bool,
    /// The newest presence beacon held for each person. See `presence.rs`.
    pub presence: BTreeMap<RootId, crate::presence::Heard>,
    /// The presence window in use. Set from the engine's `Timings` when it starts.
    pub timings: crate::timings::Timings,
    pub directory: Directory,
    pub addresses: AddressBook,
    /// Our dialable address. Set once the endpoint is up.
    pub my_addr: Option<PeerAddr>,
    /// Timestamp of our latest signed address. Each new one is larger.
    pub my_addr_ts: u64,
    /// Our latest signed address, passed on to others and re-broadcast on the community topics.
    pub my_signed: Option<SignedAddr>,
    /// Our address record for peer exchange. Re-signed every ten minutes. Not stored.
    pub own_record: Option<crate::addr_record::AddrRecord>,
    /// Verified address records of other devices that we may share. Not stored; see `addr_record`.
    pub addr_cache: crate::addr_record::RecordCache,
    /// Limits how often each peer may push records to us.
    pub push_limiter: crate::addr_record::PushLimiter,
    /// This device's own profile picture, if it has one.
    pub own_avatar: Option<Picture>,
    /// Pictures from contacts' direct profiles, by root, with the HLC of each.
    pub peer_avatars: BTreeMap<RootId, (Hlc, Option<Picture>)>,
    /// This device's profile banner, and the banners contacts sent directly. See `profile_banners`.
    pub banners: crate::profile_banners::BannerBook,
    /// When each hosted relay last passed this app's check, in ms. In memory: after a restart a host
    /// counts again once its relay passes. See `boosts`.
    pub relay_checks: BTreeMap<String, u64>,
    /// When each hosted relay was last probed, pass or not, so probes take turns. See `boosts`.
    pub relay_probed: BTreeMap<String, u64>,
    /// Relays this engine added to its node, so a reconcile can drop the ones no longer wanted.
    /// Relays the node was configured with are not here. See `relays::reconcile_relays`.
    pub relays_adopted: std::collections::BTreeSet<String>,
    /// Relays from an invite or a community record, with when each arrived, kept for a while before
    /// a community's events name them.
    pub relays_transient: BTreeMap<String, u64>,
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
        let mut communities = load_communities(&paths, &mut directory, vault);
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
        let refused = crate::groups::RefusedBook::load(&paths)?;
        let conv_msgs = crate::groups::load_messages(&paths, &conv_book);
        let conv_acts = crate::conv_actions::load_acts(&paths, &conv_book);
        let contact_times = crate::contact_times::ContactTimes::load(&paths)?;
        let mention_reads = crate::mention_reads::MentionReads::load(&paths)?;
        let inbox = crate::inbox::InboxBook::load(&paths)?;
        let attach = crate::attachments::AttachBook::load(&paths)?;
        let thumb_fetching = std::collections::BTreeSet::new();
        let source_counts = BTreeMap::new();
        let access = crate::access::load(&paths, communities.keys().copied())?;
        for (id, c) in communities.iter_mut() {
            if let Some(keys) = access.get(id) {
                c.set_ring(keys.history.clone());
                match crate::private_keys::load(&paths, vault, id) {
                    Ok(keys) => c.private_keys = keys,
                    Err(e) => warn!(community = %id, error = %e, "could not read the private channel keys"),
                }
                match crate::epoch_keys::load(&paths, vault, id) {
                    Ok(epochs) => {
                        c.set_epoch_ring(epochs.enc_ring());
                        c.epochs = epochs;
                    }
                    Err(e) => warn!(community = %id, error = %e, "could not read the epoch keys"),
                }
                // Startup runs no side effects. Events that were stored before are already applied.
                let _ = c.take_unhandled(&[]);
            }
        }
        let identity_changes: Vec<IdentityChange> =
            crate::store::read_records(&paths.identity_changes()).unwrap_or_default();
        let own = crate::presence::load_own(&paths);
        let own_avatar = crate::avatars::read_sealed::<Option<Picture>>(&paths.own_avatar(), &crate::avatars::own_vault(vault))?
            .flatten();
        let peer_avatars = crate::avatars::read_sealed(&paths.peer_avatars(), &crate::avatars::peers_vault(vault))?
            .unwrap_or_default();
        let banners = crate::avatars::read_sealed(&paths.banners(), &crate::avatars::peers_vault(vault))?
            .unwrap_or_default();
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
            refused,
            conv_msgs,
            conv_acts,
            contact_times,
            mention_reads,
            inbox,
            attach,
            thumb_fetching,
            source_counts,
            blob_scopes: Default::default(),
            own,
            auto_idle: false,
            presence: BTreeMap::new(),
            timings: crate::timings::Timings::default(),
            directory,
            addresses: AddressBook::from_list(addrs),
            my_addr: None,
            my_addr_ts,
            my_signed,
            own_record: None,
            addr_cache: Default::default(),
            push_limiter: Default::default(),
            own_avatar,
            peer_avatars,
            banners,
            relay_checks: BTreeMap::new(),
            relay_probed: BTreeMap::new(),
            relays_adopted: std::collections::BTreeSet::new(),
            relays_transient: BTreeMap::new(),
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
            icon: self.icon_of(c),
            icon_poster: self.icon_poster_of(c),
            mentions: self.community_mentions(c, crate::server_prefs::of(self, c.id()).suppress_everyone),
            can_mention_everyone: c.state().may_mention_everyone(&self.me),
        }
    }

    /// Whether a person can be mentioned in `c` as this device reads it: a member who is not banned.
    fn can_be_mentioned(c: &Community, user: &RootId) -> bool {
        c.is_member(user) && !c.state().banned.contains(user)
    }

    /// What a message mentions, as this device reads it. A person counts only if they can be
    /// mentioned here. `everyone` counts only if the author could mention everyone when the message
    /// was accepted (`mention_all`). Returns whether this device is mentioned, the people who count,
    /// and whether `@everyone` counts.
    pub(crate) fn resolve_mentions(&self, c: &Community, mention_all: bool, plain: &str) -> (bool, Vec<MentionView>, bool) {
        let raw = crate::attachments::mentions_of(plain);
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for bytes in raw.users {
            let user = RootId(bytes);
            if !Self::can_be_mentioned(c, &user) || !seen.insert(user) {
                continue;
            }
            out.push(MentionView {
                id: user.to_string(),
                display_name: self.display_name(&user),
            });
        }
        let everyone = raw.everyone && mention_all;
        let me = seen.contains(&self.me) || (everyone && Self::can_be_mentioned(c, &self.me));
        (me, out, everyone)
    }

    /// Unread mentions of this device in one text channel: messages newer than its last mark-read
    /// that mention it. With `suppress_everyone` an `@everyone` alone does not count.
    fn mention_count(&self, c: &Community, channel: &ChannelId, suppress_everyone: bool) -> u32 {
        let key = crate::mention_reads::key(&c.id().to_string(), &channel.to_string());
        let since = self.mention_reads.at.get(&key).copied().unwrap_or(0);
        let Some(msgs) = c.state().messages.get(channel) else {
            return 0;
        };
        msgs.iter()
            .filter(|m| !m.deleted && m.hlc.wall_ms > since)
            .filter(|m| {
                let text = if m.sealed {
                    c.content.get(&m.id).map(|(_, t)| t.clone()).unwrap_or_default()
                } else {
                    m.text.clone()
                };
                let (me, mentions, everyone) = self.resolve_mentions(c, m.mention_all, &text);
                let by_name = mentions.iter().any(|x| x.id == self.me.to_string());
                me && (by_name || !suppress_everyone || !everyone)
            })
            .count() as u32
    }

    /// Unread mentions of this device in the community's text channels.
    fn community_mentions(&self, c: &Community, suppress_everyone: bool) -> u32 {
        c.state()
            .channels
            .keys()
            .filter(|id| !c.state().voice_channels.contains(id))
            .map(|id| self.mention_count(c, id, suppress_everyone))
            .sum()
    }

    pub fn community_view(&self, c: &Community) -> CommunityView {
        let suppress_everyone = crate::server_prefs::of(self, c.id()).suppress_everyone;
        // With a layout, channels follow it top to bottom. Without one, text channels, then voice channels, each
        // in the server's order.
        let layout = c.state().channel_layout();
        let order: Vec<ChannelId> = match &layout {
            Some(entries) => crate::channel_layout::flatten(entries),
            None => {
                let (text, voice) = c.state().channel_order();
                text.into_iter().chain(voice).collect()
            }
        };
        let can_manage_channels = c.state().may_manage_channels(&self.me);
        let layout = layout.map(|entries| crate::channel_layout::view_for(c.state(), &entries, self.me, can_manage_channels));
        let channels = order
            .iter()
            .filter_map(|id| c.state().channels.get(id).map(|name| (id, name)))
            // A private channel is listed only to the people who can read it.
            .filter(|(id, _)| c.state().can_read(**id, self.me))
            .map(|(id, name)| {
                let voice = c.state().voice_channels.contains(id);
                let mut info = ChannelInfo::new(*id, name.clone(), voice, &c.state().settings(id));
                info.mentions = if voice { 0 } else { self.mention_count(c, id, suppress_everyone) };
                info.private = c.state().is_private(*id);
                info
            })
            .collect();
        let now = crate::engine::now_ms();
        let hosts = self.boost_hosts(c, now);
        let members = c
            .state()
            .members
            .iter()
            .filter(|(_, m)| !m.left)
            .map(|(root, _)| MemberInfo {
                root_id: root.to_string(),
                suffix: root.suffix(),
                display_name: self.display_name(root),
                avatar: self.avatar_of(root),
                handle: c.handle_of(root),
                role: c.role_of(root).to_string(),
                banned: c.state().banned.contains(root),
                status: self.presence_of(root, now),
                relay_host: hosts.contains(root),
                storage: c.state().members.get(root).is_some_and(|m| m.storage),
                storage_granted: c.state().members.get(root).is_some_and(|m| m.storage) && c.state().storage_granted(root),
            })
            .collect();
        CommunityView {
            id: c.id().to_string(),
            name: c.name().to_string(),
            owner: c.owner().to_string(),
            my_role: c.role_of(&self.me).to_string(),
            icon: self.icon_of(c),
            icon_poster: self.icon_poster_of(c),
            can_mention_everyone: c.state().may_mention_everyone(&self.me),
            banner: self.banner_of(c),
            banner_poster: self.banner_poster_of(c),
            banner_preset: crate::invite_preview::preset_of(c.state().banner.and_then(|(_, b)| b)),
            boost: self.boost_view(c, now),
            description: c.state().profile.as_ref().map(|(_, p)| p.description.clone()).unwrap_or_default(),
            traits: c.state().profile.as_ref().map(|(_, p)| p.traits.clone()).unwrap_or_default(),
            created_ms: c.genesis.created_ms,
            channels,
            members,
            relays: c.state().relays.clone(),
            member_only: c.state().member_only,
            history: match c.state().history {
                dissent_events::HistorySetting::All => "all".to_string(),
                dissent_events::HistorySetting::SinceJoin => "since_join".to_string(),
            },
            // Some control events are waiting for a key this device lacks, so the history is not all
            // readable yet. A member that holds the key sends it when asked.
            history_locked: c
                .state()
                .rejected
                .iter()
                .any(|(_, why)| *why == dissent_events::Rejection::Locked),
            // The engine fills in the people in each voice channel, which it hears on the calls side.
            voice_channels: Vec::new(),
            layout,
            can_manage_channels,
        }
    }

    pub fn user_view(&self, user: &RootId) -> UserView {
        let shared = self.shared_with(user);
        let fields = crate::local::user_fields(self, user);
        let (banner, banner_poster, banner_preset) = self.profile_banner_view(user);
        UserView {
            root_id: user.to_string(),
            suffix: user.suffix(),
            display_name: self.display_name(user),
            avatar: self.avatar_of(user),
            banner,
            banner_poster,
            banner_preset,
            is_contact: self.is_contact(user),
            identity_changed_ms: crate::identity_watch::last_change(&self.identity_changes, user),
            key_changed_ms: fields.key_changed_ms,
            verified_ms: fields.verified_ms,
            note: fields.note,
            nickname: fields.nickname,
            ignored: fields.ignored,
            status: self.presence_of(user, crate::engine::now_ms()),
            contact_since_ms: self.contact_times.since.get(user).copied(),
            first_seen_ms: self.contact_times.first_seen.get(user).copied(),
            handles: shared
                .iter()
                .map(|c| HandleView {
                    community_id: c.id().to_string(),
                    community_name: c.name().to_string(),
                    handle: c.handle_of(user),
                })
                .collect(),
            mutual_friends: Vec::new(),
            mutual_communities: shared
                .iter()
                .map(|c| CommunityRef {
                    id: c.id().to_string(),
                    name: c.name().to_string(),
                })
                .collect(),
            hosts_for: self.hosted_by(user, crate::engine::now_ms()),
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
        // A private channel's messages are for its readers only. Others see none.
        if !c.state().can_read(*channel, self.me) {
            return Vec::new();
        }
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
                let joined_ms = c.state().members.get(&self.me).map(|member| member.joined.wall_ms);
                let before_join = locked && joined_ms.is_some_and(|at| m.hlc.wall_ms < at);
                let text = if m.sealed {
                    decrypted.unwrap_or_default()
                } else {
                    m.text.clone()
                };
                let (mentions_me, mentions, mentions_everyone) = self.resolve_mentions(c, m.mention_all, &text);
                MessageItem {
                    mentions_me,
                    mentions,
                    mentions_everyone,
                    id: m.id.to_string(),
                    author: m.author.to_string(),
                    author_name: self.display_name(&m.author),
                    attachments: self.attachment_views(&text),
                    embeds: crate::attachments::embed_views(&self.paths, &text),
                    text: crate::attachments::text_of(&text),
                    ms: m.hlc.wall_ms,
                    edited: m.edited,
                    deleted: m.deleted,
                    locked,
                    before_join,
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
    let valid: Vec<SignedEvent> = crate::store::read_records_skipping::<SignedEvent>(&paths.events_log(id))?
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
        let (dissent_events::Payload::Sealed { kind, .. } | dissent_events::Payload::PrivateSealed { kind, .. }) =
            &se.event.body.payload
        else {
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
