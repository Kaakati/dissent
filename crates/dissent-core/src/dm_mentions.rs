//! Mentions in direct messages and group DMs.
//!
//! They travel as in a channel: the text stays plain `@Name`, and the people it mentions ride in the sealed payload's
//! `mentions` trailer extension (see `attachments.rs`). A DM's text was already a sealed payload whenever it carried
//! files or link previews, so older clients show a message with mentions as its text. The wire format is unchanged.
//!
//! - **Who counts.** A person counts only if they are a member of the conversation, as this device reads it. The
//!   author mentioning themselves does not highlight their own message.
//! - **No `@everyone`.** A conversation's members all see every message, so the flag is dropped on send and ignored
//!   on arrival.
//! - **Edits.** An edit is plain text, so a message keeps the mentions it was sent with.
//! - **Counts.** A conversation's unread mention count is the number of unread messages (see `dm::unread_of`) that
//!   mention this device. Marking the conversation read clears both, so nothing new is stored.

use std::collections::BTreeSet;

use dissent_identity::RootId;

use crate::attachments::{mentions_of, Mentions};
use crate::views::MentionView;
use crate::world::World;

/// The mentions a conversation message is sent with: the people, once each, and never `@everyone`.
pub(crate) fn for_conversation(m: Mentions) -> Mentions {
    let mut seen = BTreeSet::new();
    Mentions {
        users: m.users.into_iter().filter(|u| seen.insert(*u)).collect(),
        everyone: false,
    }
}

/// What a conversation message mentions, as this device reads it: whether it mentions this device, and the members it
/// mentions. `members` are the conversation's members and `author` wrote the message.
pub(crate) fn resolve(w: &World, members: &[RootId], author: RootId, plain: &str) -> (bool, Vec<MentionView>) {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for bytes in mentions_of(plain).users {
        let user = RootId(bytes);
        if !members.contains(&user) || !seen.insert(user) {
            continue;
        }
        out.push(MentionView {
            id: user.to_string(),
            display_name: w.display_name(&user),
        });
    }
    (author != w.me && seen.contains(&w.me), out)
}

/// Whether a conversation message mentions this device. Cheaper than `resolve`: no names are looked up.
pub(crate) fn mentions_me(me: &RootId, members: &[RootId], author: RootId, plain: &str) -> bool {
    author != *me && members.contains(me) && mentions_of(plain).users.iter().any(|u| u == &me.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(byte: u8) -> RootId {
        RootId([byte; 32])
    }

    fn sealed(users: &[RootId], everyone: bool) -> String {
        let m = Mentions { users: users.iter().map(|u| u.0).collect(), everyone };
        crate::attachments::pack_all("hi @Bob", &[], &[], &[], Some(&m)).unwrap()
    }

    #[test]
    fn a_conversation_send_drops_everyone_and_repeats() {
        let m = for_conversation(Mentions { users: vec![[2; 32], [3; 32], [2; 32]], everyone: true });
        assert_eq!(m, Mentions { users: vec![[2; 32], [3; 32]], everyone: false });
        assert!(for_conversation(Mentions { users: vec![], everyone: true }).is_empty());
    }

    #[test]
    fn a_reader_that_knows_no_mentions_still_reads_the_text() {
        // Older clients read a DM through `text_of`, as they do a DM with files: the mentions are skipped.
        assert_eq!(crate::attachments::text_of(&sealed(&[who(2)], false)), "hi @Bob");
    }

    #[test]
    fn only_a_member_mentioned_by_someone_else_counts() {
        let (me, bob, carol, stranger) = (who(1), who(2), who(3), who(9));
        let members = [me, bob, carol];
        assert!(mentions_me(&me, &members, bob, &sealed(&[me], false)));
        // Mentioning yourself does not highlight your own message.
        assert!(!mentions_me(&me, &members, me, &sealed(&[me], false)));
        // Someone else, or nobody.
        assert!(!mentions_me(&me, &members, bob, &sealed(&[carol], false)));
        assert!(!mentions_me(&me, &members, bob, "plain @Me"));
        // `@everyone` means nothing in a conversation.
        assert!(!mentions_me(&me, &members, bob, &sealed(&[], true)));
        // A conversation this device is not a member of.
        assert!(!mentions_me(&stranger, &members, bob, &sealed(&[stranger], false)));
    }
}
