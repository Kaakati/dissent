//! Channel categories: the server's layout of channels and categories, which owners and admins arrange. The
//! layout is one sealed control event, `Payload::ChannelLayout`, and the latest wins. A `ChannelOrder` derived
//! from it goes out too, so a client from before layouts keeps roughly the same order. See
//! `docs/design/02-communities-and-dms.md`, "Channel categories".

use std::collections::BTreeSet;

use dissent_events::state::{valid_layout, CommunityState, MAX_CATEGORIES, MAX_CATEGORY_NAME_CHARS, MAX_ORDERED_CHANNELS};
use dissent_events::{ChannelId, LayoutCategory, LayoutEntry, Payload};
use dissent_identity::{CommunityId, RootId};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::views::LayoutEntryView;

/// Every channel of a layout, top to bottom, categories opened in place.
pub(crate) fn flatten(entries: &[LayoutEntry]) -> Vec<ChannelId> {
    entries
        .iter()
        .flat_map(|e| match e {
            LayoutEntry::Channel(id) => vec![*id],
            LayoutEntry::Category(c) => c.channels.clone(),
        })
        .collect()
}

/// A resolved layout as `me` sees it. Channels `me` cannot read are left out, and a category whose channels are
/// all hidden from `me` goes too. An empty category shows only to people who may arrange channels, so they can
/// fill it.
pub(crate) fn view_for(state: &CommunityState, entries: &[LayoutEntry], me: RootId, may_manage: bool) -> Vec<LayoutEntryView> {
    let readable = |id: &ChannelId| state.can_read(*id, me);
    entries
        .iter()
        .filter_map(|e| match e {
            LayoutEntry::Channel(id) => readable(id).then(|| LayoutEntryView::Channel { id: id.to_string() }),
            LayoutEntry::Category(c) => {
                let channels: Vec<String> = c.channels.iter().filter(|id| readable(id)).map(|id| id.to_string()).collect();
                let shown = !channels.is_empty() || (c.channels.is_empty() && may_manage);
                shown.then(|| LayoutEntryView::Category {
                    id: Some(hex(&c.id)),
                    name: c.name.clone(),
                    collapsed: c.collapsed,
                    channels,
                })
            }
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads a category id as `hex` writes it: 32 lowercase hex digits.
fn parse_category_id(text: &str) -> Result<[u8; 16]> {
    let bad = || CoreError::Invalid("a category id is 32 hex digits".into());
    if text.len() != 32 {
        return Err(bad());
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2).ok_or_else(bad)?, 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// The `ChannelOrder` a layout amounts to for a client from before layouts: its channels top to bottom, text and
/// voice apart.
fn derived_order(state: &CommunityState, entries: &[LayoutEntry]) -> Payload {
    let (voice, text): (Vec<ChannelId>, Vec<ChannelId>) =
        flatten(entries).into_iter().partition(|id| state.voice_channels.contains(id));
    Payload::ChannelOrder { text, voice }
}

impl Engine {
    /// Sets the server's channel layout for everyone, top first. Owner and admins only. Every channel id must be a
    /// live channel, listed once. A category without an id is new and gets a random one. A channel left out goes
    /// after the layout, outside any category, as replay places it.
    pub async fn set_channel_layout(&self, community: CommunityId, entries: Vec<LayoutEntryView>) -> Result<()> {
        let (layout, order) = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
            let state = c.state();
            if !state.may_manage_channels(&self.inner.me) {
                return Err(CoreError::Rejected("Only the owner and admins can arrange channels".into()));
            }
            let mut seen = BTreeSet::new();
            let mut channel = |text: &str| -> Result<ChannelId> {
                let id: ChannelId = text.parse().map_err(|_| CoreError::Invalid(format!("bad channel id {text}")))?;
                if !state.channels.contains_key(&id) {
                    return Err(CoreError::NotFound(format!("channel {id}")));
                }
                if !seen.insert(id) {
                    return Err(CoreError::Invalid(format!("channel {id} is placed twice")));
                }
                Ok(id)
            };
            let mut layout = Vec::with_capacity(entries.len());
            for entry in entries {
                layout.push(match entry {
                    LayoutEntryView::Channel { id } => LayoutEntry::Channel(channel(&id)?),
                    LayoutEntryView::Category { id, name, collapsed, channels } => {
                        let id = match id {
                            Some(text) => parse_category_id(&text)?,
                            None => rand::random(),
                        };
                        let name = name.trim().to_string();
                        if name.is_empty() || name.chars().count() > MAX_CATEGORY_NAME_CHARS {
                            return Err(CoreError::Invalid(format!(
                                "a category name is 1 to {MAX_CATEGORY_NAME_CHARS} characters"
                            )));
                        }
                        let channels = channels.iter().map(|c| channel(c)).collect::<Result<Vec<_>>>()?;
                        LayoutEntry::Category(LayoutCategory { id, name, collapsed, channels })
                    }
                });
            }
            if !valid_layout(&layout) {
                return Err(CoreError::Invalid(format!(
                    "a layout holds at most {MAX_CATEGORIES} categories, with distinct ids, and {MAX_ORDERED_CHANNELS} channels"
                )));
            }
            let order = derived_order(state, &layout);
            (layout, order)
        };
        self.author_publish(community, dissent_events::ChannelId::CONTROL, Payload::ChannelLayout { entries: layout })
            .await?;
        // For clients from before layouts. A current client follows the layout and ignores this.
        self.author_publish(community, dissent_events::ChannelId::CONTROL, order).await?;
        self.emit(crate::views::EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_ids_read_back_from_their_hex() {
        let id = [0xab; 16];
        assert_eq!(parse_category_id(&hex(&id)).unwrap(), id);
        assert!(parse_category_id("abc").is_err());
        assert!(parse_category_id(&"zz".repeat(16)).is_err());
    }

    #[test]
    fn a_layout_flattens_top_to_bottom() {
        let (a, b, c) = (ChannelId([1; 32]), ChannelId([2; 32]), ChannelId([3; 32]));
        let entries = vec![
            LayoutEntry::Channel(a),
            LayoutEntry::Category(LayoutCategory { id: [0; 16], name: "x".into(), collapsed: false, channels: vec![c, b] }),
        ];
        assert_eq!(flatten(&entries), vec![a, c, b]);
    }
}
