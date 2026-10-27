//! Private channels, on the core side. Each generation of a private channel has its own random key.
//! A reader holds the keys it has been given. A message is sealed under the key of the generation it
//! was made in, and the author's device seals it, so a non-reader never receives the key. The keys are
//! kept sealed under the vault. See `docs/design/11-private-channels.md`.

use std::collections::BTreeMap;
use std::time::Duration;

use dissent_events::{
    carry_private, open_private_text, private_aad, seal_private_text, ChannelId, Payload, Role, SealedKind, SignedEvent,
};
use dissent_identity::{CommunityId, DeviceId, RootId, Vault};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::membership::CommitRules;
use crate::rpc::{Request, Response};
use crate::store::{write_bytes_atomic, Paths};
use crate::views::ChannelInfo;

/// Refusal for an attachment sent to a private channel. Files are not built for them yet.
pub(crate) const PRIVATE_FILES_REFUSED: &str = "Files aren't supported in private channels yet";
/// The first wait after a key request fails, then doubled up to the cap.
const PRIVATE_KEY_RETRY_FIRST: Duration = Duration::from_secs(1);
const PRIVATE_KEY_RETRY_CAP: Duration = Duration::from_secs(60);

pub(crate) fn private_retry_after(wait: Duration) -> Duration {
    (wait * 2).min(PRIVATE_KEY_RETRY_CAP)
}

const PURPOSE: &[u8] = b"dissent/private-keys/v1";
const AAD_DOMAIN: &[u8] = b"dissent/private-keys-aad/v1";

/// The key of each generation of each private channel this device holds.
pub(crate) type PrivateKeys = BTreeMap<(ChannelId, u64), [u8; 32]>;

#[derive(Serialize, Deserialize)]
struct Entry {
    channel: ChannelId,
    generation: u64,
    key: [u8; 32],
}

fn aad(id: &CommunityId) -> Vec<u8> {
    [AAD_DOMAIN, id.as_bytes()].concat()
}

/// Reads a community's private-channel keys. A missing file is none.
pub(crate) fn load(paths: &Paths, vault: &Vault, id: &CommunityId) -> Result<PrivateKeys> {
    match std::fs::read(paths.private_keys(id)) {
        Ok(sealed) => {
            let plain = vault.subkey(PURPOSE).open_sealed(&aad(id), &sealed)?;
            let entries: Vec<Entry> =
                postcard::from_bytes(&plain).map_err(|e| CoreError::Encoding(e.to_string()))?;
            Ok(entries.into_iter().map(|e| ((e.channel, e.generation), e.key)).collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PrivateKeys::new()),
        Err(e) => Err(e.into()),
    }
}

/// Writes a community's private-channel keys, sealed.
pub(crate) fn save(paths: &Paths, vault: &Vault, id: &CommunityId, keys: &PrivateKeys) -> Result<()> {
    let entries: Vec<Entry> = keys
        .iter()
        .map(|((channel, generation), key)| Entry {
            channel: *channel,
            generation: *generation,
            key: *key,
        })
        .collect();
    let plain = postcard::to_allocvec(&entries).map_err(|e| CoreError::Encoding(e.to_string()))?;
    let sealed = vault.subkey(PURPOSE).seal(&aad(id), &plain);
    write_bytes_atomic(&paths.private_keys(id), &sealed)
}

impl Engine {
    /// Whether `channel` is a private channel, by its latest accepted access change.
    pub(crate) fn is_private_channel(&self, community: CommunityId, channel: ChannelId) -> bool {
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().is_private(channel))
    }

    /// The generation a private channel's messages are sealed under now.
    pub(crate) fn current_key_generation(&self, community: CommunityId, channel: ChannelId) -> u64 {
        self.read()
            .communities
            .get(&community)
            .and_then(|c| c.state().key_generation.get(&channel).copied())
            .unwrap_or(0)
    }

    /// The key of `channel`'s `generation`, if this device holds it.
    pub(crate) fn private_key(&self, community: CommunityId, channel: ChannelId, generation: u64) -> Option<[u8; 32]> {
        self.read()
            .communities
            .get(&community)
            .and_then(|c| c.private_keys.get(&(channel, generation)).copied())
    }

    /// Stores keys this device was given or made, and saves them. Returns whether anything was new.
    pub(crate) fn remember_private_keys(&self, community: CommunityId, entries: Vec<(ChannelId, u64, [u8; 32])>) -> bool {
        let mut w = self.write();
        let paths = w.paths.clone();
        let Some(c) = w.communities.get_mut(&community) else {
            return false;
        };
        let mut changed = false;
        for (channel, generation, key) in entries {
            if let std::collections::btree_map::Entry::Vacant(slot) = c.private_keys.entry((channel, generation)) {
                slot.insert(key);
                changed = true;
            }
        }
        if changed {
            if let Err(e) = save(&paths, &self.inner.vault, &community, &c.private_keys) {
                warn!(community = %community, error = %e, "could not save the private channel keys");
            }
        }
        changed
    }

    /// Asks the community's member devices for the keys of a private channel this device may read. A
    /// device answers only a reader, so anyone else gets nothing. Returns whether any key was new.
    pub(crate) async fn fetch_private_keys(&self, community: CommunityId, channel: ChannelId) -> bool {
        let devices = {
            let w = self.read();
            match w.communities.get(&community) {
                Some(c) => w.member_devices(c),
                None => return false,
            }
        };
        for device in devices {
            if let Ok(Response::ChannelKeys(pairs)) = self
                .call(device, &Request::ChannelKeys { community, channel })
                .await
            {
                let entries: Vec<_> = pairs.into_iter().map(|(generation, key)| (channel, generation, key)).collect();
                if self.remember_private_keys(community, entries) {
                    self.unseal_pass(community, false);
                    return true;
                }
            }
        }
        false
    }

    /// The payload for a `kind` of entry in a private channel: its text sealed under the current generation's
    /// key. A device without that key asks its readers for it first.
    pub(crate) async fn private_payload(
        &self,
        community: CommunityId,
        channel: ChannelId,
        kind: SealedKind,
        plain: &str,
    ) -> Result<Payload> {
        let generation = self.current_key_generation(community, channel);
        let key = match self.private_key(community, channel, generation) {
            Some(key) => key,
            None => {
                self.fetch_private_keys(community, channel).await;
                self.private_key(community, channel, generation).ok_or_else(|| {
                    CoreError::Invalid("this device has no key for the private channel yet".into())
                })?
            }
        };
        let aad = private_aad(&community, &channel, generation, &self.inner.me);
        let text = postcard::to_allocvec(&plain.to_string()).map_err(|e| CoreError::Encoding(e.to_string()))?;
        let ciphertext = seal_private_text(&key, &aad, &text)?;
        Ok(carry_private(kind, generation, ciphertext)?)
    }

    /// Who may read a channel now, from its latest accepted access change.
    pub fn channel_access(&self, community: CommunityId, channel: ChannelId) -> Result<crate::views::ChannelAccessView> {
        let r = self.read();
        let c = r.communities.get(&community).ok_or_else(|| CoreError::Invalid("unknown community".into()))?;
        let state = c.state();
        // The reader list of a private channel is shown only to its readers and to whoever manages access.
        if !state.can_read(channel, self.inner.me) && !state.may_manage_access(self.inner.me) {
            return Err(CoreError::Invalid("only a reader or an admin sees who reads this channel".into()));
        }
        let (private, roles, members) = match state.channel_access.get(&channel) {
            Some((_, spec)) => (
                spec.private,
                spec.roles
                    .iter()
                    .map(|role| match role {
                        dissent_events::Role::Admin => "admin",
                        dissent_events::Role::Moderator => "moderator",
                    })
                    .collect(),
                spec.members.iter().map(|m| m.to_string()).collect(),
            ),
            None => (false, Vec::new(), Vec::new()),
        };
        Ok(crate::views::ChannelAccessView { private, roles, members })
    }

    /// Creates a private channel in two events: the channel, then its access. Posts to it are refused until
    /// its access is in local state. If the access event is refused, the channel is deleted again, so it is not
    /// left open to everyone. Owner and admin only, as replay checks.
    pub async fn create_private_channel(
        &self,
        community: CommunityId,
        name: String,
        roles: Vec<Role>,
        members: Vec<RootId>,
    ) -> Result<ChannelInfo> {
        let name = crate::engine::clean_text(&name, "channel name")?;
        let created = self
            .author_publish(community, ChannelId::CONTROL, Payload::ChannelCreate { name: name.clone() })
            .await?;
        let channel = ChannelId(created.event.id.0);
        self.set_pending_private(community, channel, true);
        if let Err(access_err) = self.publish_channel_access(community, channel, roles, members).await {
            let undone = self
                .author_publish(community, ChannelId::CONTROL, Payload::ChannelDelete { channel })
                .await;
            match undone {
                // Once the delete is in, the channel is gone, and the pending mark has no more to guard.
                Ok(_) => self.set_pending_private(community, channel, false),
                // The channel stays pending, so nothing is posted to it, but it is listed until the delete goes through.
                Err(undo_err) => {
                    warn!(community = %community, error = %undo_err, "could not delete a private channel whose access failed")
                }
            }
            self.emit(crate::views::EngineEvent::CommunitiesChanged);
            return Err(access_err);
        }
        self.set_pending_private(community, channel, false);
        self.ensure_private_key(community, channel);
        self.emit(crate::views::EngineEvent::CommunitiesChanged);
        let mut info = ChannelInfo::new(channel, name, false, &dissent_events::state::ChannelSettings::default());
        info.private = true;
        Ok(info)
    }

    /// The second step of `create_private_channel`.
    async fn publish_channel_access(
        &self,
        community: CommunityId,
        channel: ChannelId,
        roles: Vec<Role>,
        members: Vec<RootId>,
    ) -> Result<()> {
        #[cfg(test)]
        if self.inner.access_fault.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(CoreError::Invalid("injected access failure".into()));
        }
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::ChannelAccess {
                channel,
                private: true,
                roles,
                members,
            },
        )
        .await?;
        Ok(())
    }

    /// Makes the key of the channel's current generation, if this device has none for it yet.
    pub(crate) fn ensure_private_key(&self, community: CommunityId, channel: ChannelId) {
        let generation = self.current_key_generation(community, channel);
        if self.private_key(community, channel, generation).is_none() {
            let key: [u8; 32] = rand::random();
            self.remember_private_keys(community, vec![(channel, generation, key)]);
        }
    }

    /// Sets who may read a channel, and makes the key of the generation this change starts, if this
    /// device is the one making it. Owner and admin only, as replay checks. Removing a reader starts a
    /// new generation, and its key is held here until the remaining readers ask for it.
    pub async fn set_channel_access(
        &self,
        community: CommunityId,
        channel: ChannelId,
        private: bool,
        roles: Vec<dissent_events::Role>,
        members: Vec<RootId>,
    ) -> Result<()> {
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::ChannelAccess {
                channel,
                private,
                roles,
                members,
            },
        )
        .await?;
        if private {
            self.ensure_private_key(community, channel);
        }
        Ok(())
    }

    /// Asks for a channel's keys until this device holds the generation, or stops being a reader. A failed
    /// attempt waits 1, 2, 4 s and then up to 60 s. A member coming online or a change of access wakes it early.
    pub(crate) async fn retry_private_keys(&self, community: CommunityId, channel: ChannelId, generation: u64) {
        let mut wait = PRIVATE_KEY_RETRY_FIRST;
        loop {
            // Both wake-ups are armed before the attempt, so one that fires during it is not lost.
            let seen = self.inner.member_seen.notified();
            let changed = self.inner.access_changed.notified();
            tokio::pin!(seen);
            tokio::pin!(changed);
            self.fetch_private_keys(community, channel).await;
            if self.private_key(community, channel, generation).is_some() || !self.may_read_private(community, channel) {
                return;
            }
            warn!(community = %community, "no reader answered for a private channel's keys yet; will retry");
            tokio::select! {
                _ = tokio::time::sleep(wait) => wait = private_retry_after(wait),
                _ = &mut seen => wait = PRIVATE_KEY_RETRY_FIRST,
                _ = &mut changed => wait = PRIVATE_KEY_RETRY_FIRST,
            }
        }
    }

    /// Whether this device may read `channel` now. A community this device has left reads as no.
    fn may_read_private(&self, community: CommunityId, channel: ChannelId) -> bool {
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().can_read(channel, self.inner.me))
    }

    /// Whether `channel` is being set up and must take no posts yet.
    pub(crate) fn is_pending_private(&self, community: CommunityId, channel: ChannelId) -> bool {
        self.inner
            .private_pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(community, channel))
    }

    pub(crate) fn set_pending_private(&self, community: CommunityId, channel: ChannelId, pending: bool) {
        let mut set = self
            .inner
            .private_pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending {
            set.insert((community, channel));
        } else {
            set.remove(&(community, channel));
        }
    }

    /// Asks the readers for a channel's keys in the background. A request already in flight for the same
    /// channel and generation is not repeated, so a backlog of undecryptable messages asks once.
    fn request_private_keys(&self, community: CommunityId, channel: ChannelId, generation: u64) {
        let fresh = self
            .inner
            .private_fetching
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((community, channel, generation));
        if !fresh {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            engine.retry_private_keys(community, channel, generation).await;
            engine
                .inner
                .private_fetching
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&(community, channel, generation));
        });
    }

    /// Opens a private message's text with the key of its generation, and stores it as the message's content.
    pub(crate) fn unseal_private(&self, community: CommunityId, se: &SignedEvent, generation: u64, ciphertext: &[u8]) {
        let channel = se.event.body.channel;
        let Some(key) = self.private_key(community, channel, generation) else {
            self.request_private_keys(community, channel, generation);
            return;
        };
        let aad = private_aad(&community, &channel, generation, &se.event.body.author);
        let Some(bytes) = open_private_text(&key, &aad, ciphertext) else {
            return;
        };
        let Ok(text) = postcard::from_bytes::<String>(&bytes) else {
            return;
        };
        if let Some((id, hlc)) = crate::sealed::content_key(se) {
            self.store_content(community, id, hlc, text);
        }
    }

    /// Answers `ChannelKeys`. Only a reader in this device's DAG gets keys, and a reader gets the keys its
    /// history setting allows: every generation under `All`, the current one only under `SinceJoin`.
    pub(crate) fn serve_channel_keys(&self, from: DeviceId, community: CommunityId, channel: ChannelId) -> Response {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Response::Error("unknown community".into());
        };
        let reader = w.directory.root_of(&from).is_some_and(|root| {
            CommitRules::of(c).is_person(&root) && c.state().can_read(channel, root)
        });
        if !reader {
            return Response::Error("not a reader of this channel".into());
        }
        let current = c.state().key_generation.get(&channel).copied().unwrap_or(0);
        let all = matches!(c.state().history, dissent_events::HistorySetting::All);
        let keys = c
            .private_keys
            .iter()
            .filter(|((ch, generation), _)| *ch == channel && (all || *generation == current))
            .map(|((_, generation), key)| (*generation, *key))
            .collect();
        Response::ChannelKeys(keys)
    }
}
