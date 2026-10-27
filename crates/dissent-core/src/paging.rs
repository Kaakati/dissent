//! Paging through a conversation's messages by id. A page is the `limit` messages before a given
//! message, oldest first, so a client can ask for older history one page at a time, and a live
//! update asks for the newest page only. The pages are cut from the same ordered list the engine
//! already builds for a conversation, so they never disagree with it.

use dissent_identity::CommunityId;
use dissent_events::ChannelId;

use crate::engine::Engine;
use crate::groups::ConvId;
use crate::views::{DmItem, MessageItem};

/// The page of `limit` items before the item with id `before`, from `items` oldest first. With no
/// `before`, the newest `limit` items. An unknown `before` gives no items, as there is nothing
/// known to page back from.
pub(crate) fn page_before<T>(items: Vec<T>, before: Option<&str>, id: impl Fn(&T) -> &str, limit: usize) -> Vec<T> {
    let end = match before {
        None => items.len(),
        Some(b) => match items.iter().position(|item| id(item) == b) {
            Some(at) => at,
            None => return Vec::new(),
        },
    };
    let start = end.saturating_sub(limit);
    let mut items = items;
    items.truncate(end);
    items.split_off(start)
}

impl Engine {
    /// The `limit` messages in a channel before the message `before`, oldest first. With no `before`,
    /// the newest `limit`. Live updates read the newest page and merge it into what is shown.
    pub fn channel_messages_before(
        &self,
        community: CommunityId,
        channel: ChannelId,
        before: Option<&str>,
        limit: usize,
    ) -> Vec<MessageItem> {
        let all = self.channel_messages(community, channel, usize::MAX);
        page_before(all, before, |m| m.id.as_str(), limit)
    }

    /// The `limit` direct messages with `peer` before the message `before`, oldest first.
    pub fn dm_messages_before(&self, peer: dissent_identity::RootId, before: Option<&str>, limit: usize) -> Vec<DmItem> {
        let all = self.dm_messages(peer, usize::MAX);
        page_before(all, before, |m| m.id.as_str(), limit)
    }

    /// The `limit` messages in a group before the message `before`, oldest first.
    pub fn group_messages_before(&self, conv: ConvId, before: Option<&str>, limit: usize) -> Vec<DmItem> {
        let all = self.conversation_messages(conv, usize::MAX);
        page_before(all, before, |m| m.id.as_str(), limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("m{i:03}")).collect()
    }

    fn page(items: &[String], before: Option<&str>, limit: usize) -> Vec<String> {
        page_before(items.to_vec(), before, |s| s.as_str(), limit)
    }

    #[test]
    fn with_no_before_the_newest_page_comes_back_oldest_first() {
        let all = ids(10);
        assert_eq!(page(&all, None, 3), vec!["m007", "m008", "m009"]);
    }

    #[test]
    fn the_page_before_a_message_is_the_messages_just_older_than_it() {
        let all = ids(10);
        assert_eq!(page(&all, Some("m007"), 3), vec!["m004", "m005", "m006"]);
    }

    #[test]
    fn the_last_page_is_short_and_reaches_the_start() {
        let all = ids(10);
        assert_eq!(page(&all, Some("m002"), 3), vec!["m000", "m001"]);
        assert!(page(&all, Some("m000"), 3).is_empty(), "nothing is older than the first message");
    }

    #[test]
    fn a_limit_larger_than_the_history_gives_all_of_it() {
        let all = ids(4);
        assert_eq!(page(&all, None, 100), all);
    }

    #[test]
    fn an_unknown_before_gives_no_page() {
        let all = ids(4);
        assert!(page(&all, Some("nope"), 2).is_empty());
    }

    #[test]
    fn paging_back_through_the_history_sees_each_message_once() {
        let all = ids(25);
        let mut seen: Vec<String> = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let got = page(&all, before.as_deref(), 7);
            if got.is_empty() {
                break;
            }
            before = Some(got[0].clone());
            seen.splice(0..0, got);
        }
        assert_eq!(seen, all);
    }
}
