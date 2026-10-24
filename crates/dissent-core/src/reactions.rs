//! Replies and reactions. A reply seals its text under the community's MLS group, and its target
//! stays in clear. Reactions carry the emoji in clear for now. See `docs/design/05-encrypted-servers.md`.

use dissent_events::{normalize_emoji, ChannelId, EventId, Payload, SealedKind};
use dissent_identity::CommunityId;

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
        let text = clean_message(&text)?;
        let (epoch, ciphertext) = self.seal(community, &text)?;
        let payload = Payload::Sealed {
            epoch,
            kind: SealedKind::Reply { target },
            ciphertext,
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

    /// Adds our reaction `emoji` to `target`. The emoji is checked and normalised first.
    pub async fn react(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        emoji: String,
    ) -> Result<()> {
        let emoji = normalize_emoji(&emoji)
            .map_err(|e| CoreError::Invalid(format!("emoji: {e:?}")))?;
        self.author_publish(community, channel, Payload::React { target, emoji })
            .await?;
        Ok(())
    }

    /// Removes our reaction `emoji` from `target`. A no-op if we had not reacted with it.
    pub async fn unreact(
        &self,
        community: CommunityId,
        channel: ChannelId,
        target: EventId,
        emoji: String,
    ) -> Result<()> {
        let emoji = normalize_emoji(&emoji)
            .map_err(|e| CoreError::Invalid(format!("emoji: {e:?}")))?;
        self.author_publish(community, channel, Payload::Unreact { target, emoji })
            .await?;
        Ok(())
    }
}
