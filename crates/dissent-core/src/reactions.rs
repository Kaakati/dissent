//! Replies and reactions. A reply seals its text under the community's MLS group, and its target
//! stays in clear. A reaction seals its emoji the same way, so only members learn which emoji it
//! is. Members count reactions from the decrypted emoji. See `docs/design/05-encrypted-servers.md`.

use dissent_events::{normalize_emoji, ChannelId, EventId, Payload, SealedKind};
use dissent_identity::CommunityId;

use crate::attachments::Mentions;
use crate::engine::{clean_message, Engine};
use crate::error::{CoreError, Result};
use crate::views::MessageItem;

/// Most messages scanned to find a reply just stored.
const REPLY_LOOKUP: usize = 200;

impl Engine {
    /// Replies to `target` with `text`. The text is sealed like any message. The target stays in
    /// clear, so permission checks and the reply link need no key.
    pub async fn send_reply(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        text: String,
    ) -> Result<MessageItem> {
        self.send_reply_with(community, channel, target, text, Mentions::default()).await
    }

    /// Replies to `target`, mentioning the people in `mentions`. See `send_message_with`.
    pub async fn send_reply_with(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        text: String,
        mentions: Mentions,
    ) -> Result<MessageItem> {
        let text = clean_message(&text)?;
        let text = if mentions.is_empty() {
            text
        } else {
            crate::attachments::pack_all(&text, &[], &[], &[], Some(&mentions))?
        };
        let kind = SealedKind::Reply { target };
        let payload = if self.is_private_channel(community, channel) {
            self.private_payload(community, channel, kind, &text).await?
        } else {
            let (epoch, ciphertext) = self.seal(community, &text)?;
            Payload::Sealed { epoch, kind, ciphertext }
        };
        let se = self.author_publish(community, channel, payload).await?;
        self.store_content(community, se.event.id, se.event.body.hlc, text);
        let id = se.event.id.to_string();
        self.read()
            .message_items(&community, &channel, REPLY_LOOKUP)
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| CoreError::Invalid("the reply is not in its channel".into()))
    }

    /// Adds our reaction `emoji` to `target`. The emoji is checked and normalised, then sealed.
    pub async fn react(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        emoji: String,
    ) -> Result<()> {
        self.publish_reaction(community, channel, target, emoji, true).await
    }

    /// Removes our reaction `emoji` from `target`. A no-op if we had not reacted with it.
    pub async fn unreact(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        emoji: String,
    ) -> Result<()> {
        self.publish_reaction(community, channel, target, emoji, false).await
    }

    async fn publish_reaction(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        emoji: String,
        react: bool,
    ) -> Result<()> {
        let emoji = normalize_emoji(&emoji)
            .map_err(|e| CoreError::Invalid(format!("emoji: {e:?}")))?;
        let kind = if react {
            SealedKind::React { target }
        } else {
            SealedKind::Unreact { target }
        };
        let payload = if self.is_private_channel(community, channel) {
            self.private_payload(community, channel, kind, &emoji).await?
        } else {
            let (epoch, ciphertext) = self.seal(community, &emoji)?;
            Payload::Sealed { epoch, kind, ciphertext }
        };
        let se = self.author_publish(community, channel, payload).await?;
        self.store_content(community, se.event.id, se.event.body.hlc, emoji);
        Ok(())
    }
}
