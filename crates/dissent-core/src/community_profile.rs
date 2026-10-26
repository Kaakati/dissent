//! A community's name, banner and profile, as Server Settings changes them. The name starts as the
//! genesis name. A `CommunityRename` from the owner replaces it, and the founder's device then
//! republishes the DHT record under the new name. The banner is a preset gradient or a 16:9
//! picture, stored, fetched and served like the icon in `avatars`. The profile is a description and
//! up to five traits. Every payload is sealed, so only members see them. See
//! `docs/design/02-communities-and-dms.md`, "Server settings".

use dissent_events::state::{
    valid_community_name, valid_profile, MAX_COMMUNITY_NAME_CHARS, MAX_DESCRIPTION_CHARS,
    MAX_TRAITS, MAX_TRAIT_LABEL_CHARS,
};
use dissent_events::{normalize_trait_emoji, Banner, ChannelId, Payload, Trait, BANNER_PRESETS, MAX_BANNER_BYTES};
use dissent_identity::CommunityId;

use crate::avatars::Owner;
use crate::engine::{clean_text, not_found, Engine};
use crate::error::{CoreError, Result};
use crate::views::EngineEvent;

impl Engine {
    /// Renames a community. Owner only. The name is trimmed, and must be non-empty and at most
    /// `MAX_COMMUNITY_NAME_CHARS` characters. A device that founded the community republishes its
    /// DHT record, so new invites resolve under the new name.
    pub async fn rename_community(&self, community: CommunityId, name: String) -> Result<()> {
        let name = clean_text(&name, "server name")?;
        if !valid_community_name(&name) {
            return Err(CoreError::Invalid(format!(
                "server name is longer than {MAX_COMMUNITY_NAME_CHARS} characters"
            )));
        }
        let (owner, current) = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
            (c.owner(), c.name().to_string())
        };
        if owner != self.inner.me {
            return Err(CoreError::Rejected("NoPermission".into()));
        }
        if current == name {
            return Ok(());
        }
        self.author_publish(community, ChannelId::CONTROL, Payload::CommunityRename { name })
            .await?;
        self.after_rename();
        Ok(())
    }

    /// What a rename asks of this device, whether it authored the rename or received it: the
    /// founder republishes the record, and the views reload the community list.
    pub(crate) fn after_rename(&self) {
        self.spawn_publish_records();
        self.emit(EngineEvent::CommunitiesChanged);
    }

    /// Sets a community's banner to a picture. Owners and admins only. `bytes` is an already-cropped
    /// 16:9 WebP or PNG of at most 512 KB, at most 960 by 540 pixels.
    pub async fn set_community_banner(&self, community: CommunityId, bytes: Vec<u8>) -> Result<()> {
        self.check_can_set_icon(community)?;
        let picture = self.store_avatar_picture(bytes, MAX_BANNER_BYTES).await?;
        self.publish_banner(community, Some(Banner::Image(picture))).await
    }

    /// Sets a community's banner to one of the `BANNER_PRESETS` gradients. Owners and admins only.
    pub async fn set_community_banner_preset(&self, community: CommunityId, preset: u8) -> Result<()> {
        if preset >= BANNER_PRESETS {
            return Err(CoreError::Invalid(format!("a banner preset is below {BANNER_PRESETS}")));
        }
        self.check_can_set_icon(community)?;
        self.publish_banner(community, Some(Banner::Preset(preset))).await
    }

    /// Removes a community's banner. Owners and admins only.
    pub async fn clear_community_banner(&self, community: CommunityId) -> Result<()> {
        self.check_can_set_icon(community)?;
        self.publish_banner(community, None).await
    }

    pub(crate) async fn publish_banner(&self, community: CommunityId, banner: Option<Banner>) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::CommunityBannerSet { banner })
            .await?;
        self.emit_owner(Owner::Community(community));
        Ok(())
    }

    /// Sets a community's description and traits. Owners and admins only. The description and each
    /// label are trimmed, and a trait's emoji, which is optional, is normalised. A profile over a cap
    /// is refused rather than cut, as replay refuses it: at most 300 characters of description, five
    /// traits, and 24 characters of label, each with at most one emoji.
    pub async fn set_community_profile(
        &self,
        community: CommunityId,
        description: String,
        traits: Vec<Trait>,
    ) -> Result<()> {
        self.check_can_set_icon(community)?;
        let description = description.trim().to_string();
        let traits = traits
            .into_iter()
            .map(|t| {
                // The emoji is optional, as in Discord: a trait may be a label alone.
                let emoji = match t.emoji.trim() {
                    "" => String::new(),
                    e => normalize_trait_emoji(e).map_err(|e| CoreError::Invalid(format!("a trait emoji: {e}")))?,
                };
                Ok(Trait {
                    emoji,
                    label: clean_text(&t.label, "trait label")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if !valid_profile(&description, &traits) {
            return Err(CoreError::Invalid(format!(
                "a profile has at most {MAX_DESCRIPTION_CHARS} characters of description and \
                 {MAX_TRAITS} traits of up to {MAX_TRAIT_LABEL_CHARS} characters"
            )));
        }
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::CommunityProfileSet { description, traits },
        )
        .await?;
        self.emit(EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }
}
