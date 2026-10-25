//! Authoring. Each local change becomes a signed event. Replay checks it before it is stored or sent.

use crate::engine::{clean_message, clean_name, clean_text, not_found, Engine};
use crate::error::Result;
use crate::store::append_records;
use crate::views::{ChannelInfo, EngineEvent, MessageItem, ModAction};
use crate::wire::{encode_gossip, GossipMsg};
use crate::world::World;
use dissent_events::state::ChannelSettings;
use dissent_events::{ChannelId, Event, EventBody, EventId, Hlc, Payload, Role, SealedKind, SignedEvent};
use dissent_identity::{CommunityId, RootId};
use tracing::{debug, warn};

impl Engine {
    /// Sets our display name and re-announces it in every community we belong to.
    pub async fn set_display_name(&self, name: String) -> Result<()> {
        let name = clean_name(&name)?;
        let me = self.inner.me;
        let communities: Vec<CommunityId> = {
            let mut guard = self.write();
            guard.profile_name = name.clone();
            guard.save_profile()?;
            guard
                .communities
                .values()
                .filter(|c| c.is_member(&me))
                .map(|c| c.id())
                .collect()
        };
        for id in communities {
            let join = Payload::Join {
                display_name: name.clone(),
            };
            if let Err(e) = self.author_publish(id, ChannelId::CONTROL, join).await {
                debug!(community = %id, error = %e, "could not re-announce display name");
            }
        }
        Ok(())
    }

    pub async fn create_channel(&self, community: CommunityId, name: String) -> Result<ChannelInfo> {
        let name = clean_text(&name, "channel name")?;
        let payload = Payload::ChannelCreate { name: name.clone() };
        let se = self
            .author_publish(community, ChannelId::CONTROL, payload)
            .await?;
        Ok(ChannelInfo::new(
            ChannelId(se.event.id.0),
            name,
            false,
            &ChannelSettings::default(),
        ))
    }

    pub async fn send_message(
        &self,
        community: CommunityId,
        channel: ChannelId,
        text: String,
    ) -> Result<MessageItem> {
        let text = clean_message(&text)?;
        let embeds = self.build_embeds(&text).await;
        let plain = if embeds.is_empty() {
            text
        } else {
            crate::attachments::pack_with_embeds(&text, &[], &embeds)?
        };
        self.send_channel_plain(community, channel, plain).await
    }

    /// Seals a payload for a channel and publishes it. `plain` is text, or packed attachments.
    pub(crate) async fn send_channel_plain(
        &self,
        community: CommunityId,
        channel: ChannelId,
        plain: String,
    ) -> Result<MessageItem> {
        let (epoch, ciphertext) = self.seal(community, &plain)?;
        let ciphertext = crate::attachments::wrap_manifest(&crate::attachments::manifest_of(&plain), ciphertext)?;
        let payload = Payload::Sealed {
            epoch,
            kind: SealedKind::Message,
            ciphertext,
        };
        let se = self.author_publish(community, channel, payload).await?;
        self.store_content(community, se.event.id, se.event.body.hlc, plain.clone());
        let me = self.inner.me;
        let attachments = self.read().attachment_views(&plain);
        Ok(MessageItem {
            id: se.event.id.to_string(),
            author: me.to_string(),
            author_name: self.read().display_name(&me),
            text: crate::attachments::text_of(&plain),
            attachments,
            embeds: crate::attachments::embeds_of(&plain),
            ms: se.event.body.hlc.wall_ms,
            edited: false,
            deleted: false,
            locked: false,
            reply_to: None,
            reactions: Vec::new(),
        })
    }

    pub async fn edit_message(
        &self,
        community: CommunityId,
        channel: ChannelId,
        message: EventId,
        text: String,
    ) -> Result<()> {
        let text = clean_message(&text)?;
        let (epoch, ciphertext) = self.seal(community, &text)?;
        let payload = Payload::Sealed {
            epoch,
            kind: SealedKind::Edit { target: message },
            ciphertext,
        };
        let se = self.author_publish(community, channel, payload).await?;
        self.store_content(community, message, se.event.body.hlc, text);
        Ok(())
    }

    pub async fn delete_message(
        &self,
        community: CommunityId,
        channel: ChannelId,
        message: EventId,
    ) -> Result<()> {
        let payload = Payload::Delete { target: message };
        self.author_publish(community, channel, payload).await?;
        Ok(())
    }

    pub async fn moderate(
        &self,
        community: CommunityId,
        action: ModAction,
        user: RootId,
    ) -> Result<()> {
        let payload = match action {
            ModAction::Ban => Payload::Ban { user },
            ModAction::Unban => Payload::Unban { user },
            ModAction::GrantAdmin => Payload::RoleGrant {
                user,
                role: Role::Admin,
            },
            ModAction::RevokeAdmin => Payload::RoleRevoke {
                user,
                role: Role::Admin,
            },
            ModAction::GrantModerator => Payload::RoleGrant {
                user,
                role: Role::Moderator,
            },
            ModAction::RevokeModerator => Payload::RoleRevoke {
                user,
                role: Role::Moderator,
            },
        };
        self.author_publish(community, ChannelId::CONTROL, payload)
            .await?;
        // A ban takes effect in the group at once, if this device is the committer.
        self.mls_reconcile(community).await;
        self.rotate_if_due(community).await;
        Ok(())
    }

    pub async fn claim_handle(&self, community: CommunityId, handle: String) -> Result<()> {
        let handle = clean_text(&handle, "handle")?;
        let payload = Payload::HandleClaim { handle };
        self.author_publish(community, ChannelId::CONTROL, payload)
            .await?;
        Ok(())
    }

    /// The newest `limit` messages in a channel, oldest first.
    pub fn channel_messages(
        &self,
        community: CommunityId,
        channel: ChannelId,
        limit: usize,
    ) -> Vec<MessageItem> {
        self.read().message_items(&community, &channel, limit)
    }

    /// Signs a local event and checks it by replay. Stores it only if replay accepts it.
    pub(crate) fn author(
        &self,
        community: CommunityId,
        channel: ChannelId,
        payload: Payload,
    ) -> Result<SignedEvent> {
        let mut guard = self.write();
        let World {
            paths,
            communities,
            access,
            ..
        } = &mut *guard;
        let c = communities
            .get_mut(&community)
            .ok_or_else(|| not_found(&community))?;
        // Stamp above every stored event, so the new event sorts last and replay can check it.
        let mut hlc = self.stamp();
        if let Some(max) = c.max_hlc() {
            if hlc <= max {
                hlc = Hlc {
                    wall_ms: max.wall_ms,
                    counter: max.counter.saturating_add(1),
                };
            }
        }
        let mut body = EventBody {
            community,
            channel,
            author: self.inner.me,
            device: self.inner.identity.device_id(),
            parents: c.heads(),
            hlc,
            payload,
        };
        // Control payloads are sealed under the current secret. Without one, nothing is sent.
        if dissent_events::is_sealable(&body.payload) {
            let keys = access.get(&community).ok_or_else(|| not_found(&community))?;
            body.payload = dissent_events::seal(&keys.secret, keys.generation, &body)?;
        }
        let event = Event::sign(body, &self.inner.identity.device);
        let se = SignedEvent {
            event,
            cert: self.inner.identity.cert.clone(),
        };
        let stored = c.author(se.clone())?;
        append_records(&paths.events_log(&community), &stored)?;
        Ok(se)
    }

    /// Authors an event, then emits it locally and broadcasts it to the community.
    pub(crate) async fn author_publish(
        &self,
        community: CommunityId,
        channel: ChannelId,
        payload: Payload,
    ) -> Result<SignedEvent> {
        let se = self.author(community, channel, payload)?;
        self.publish(community, &se).await;
        Ok(se)
    }

    /// Emits a stored event locally and broadcasts it on the community topic.
    pub(crate) async fn publish(&self, community: CommunityId, se: &SignedEvent) {
        self.emit(event_for(&community, &se.event));
        let sender = self
            .read()
            .communities
            .get(&community)
            .and_then(|c| c.sender.clone());
        if let Some(sender) = sender {
            match encode_gossip(&GossipMsg::Event(Box::new(se.clone()))) {
                Ok(bytes) => {
                    if let Err(e) = sender.broadcast(bytes).await {
                        warn!(community = %community, error = %e, "event not sent on gossip; peers fetch it on their next sync");
                    }
                }
                Err(e) => warn!(community = %community, error = %e, "could not encode event"),
            }
        }
    }
}

/// The notification for a stored event. Channel content is a `Message`, anything else is a `CommunityUpdated`.
pub(crate) fn event_for(community: &CommunityId, ev: &Event) -> EngineEvent {
    let community_id = community.to_string();
    match ev.body.payload {
        Payload::Message { .. }
        | Payload::Edit { .. }
        | Payload::Delete { .. }
        | Payload::Reply { .. }
        | Payload::Sealed { .. } => {
            EngineEvent::Message {
                community_id,
                channel_id: ev.body.channel.to_string(),
            }
        }
        _ => EngineEvent::CommunityUpdated { community_id },
    }
}
