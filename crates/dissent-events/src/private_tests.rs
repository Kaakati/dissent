//! Replay rules for private channels. See `docs/design/11-private-channels.md`.

use crate::state::replay_with;
use crate::testutil::{event, on_channel, User};
use crate::{ChannelId, Dag, EventId, Hlc, Payload, Rejection, Role, SealedKind};

/// Who acts in a test.
#[derive(Clone, Copy)]
enum W {
    Owner,
    Alice,
    Bob,
    Mod,
}

fn at(wall_ms: u64) -> Hlc {
    Hlc {
        wall_ms,
        counter: 0,
    }
}

struct Fixture {
    owner: User,
    alice: User,
    bob: User,
    mod_: User,
    dag: Dag,
    last: Option<EventId>,
    channel: ChannelId,
    t: u64,
}

impl Fixture {
    /// The owner, with alice, bob and `mod_` joined, alice granted admin and `mod_` granted moderator,
    /// and a channel `secret` created by the owner.
    fn new() -> Self {
        let mut f = Fixture {
            owner: User::new(),
            alice: User::new(),
            bob: User::new(),
            mod_: User::new(),
            dag: Dag::new(),
            last: None,
            channel: ChannelId::CONTROL,
            t: 0,
        };
        for w in [W::Alice, W::Bob, W::Mod] {
            f.push(
                w,
                Payload::Join {
                    display_name: "member".into(),
                },
            );
        }
        let (alice, mod_) = (f.alice.id(), f.mod_.id());
        f.push(
            W::Owner,
            Payload::RoleGrant {
                user: alice,
                role: Role::Admin,
            },
        );
        f.push(
            W::Owner,
            Payload::RoleGrant {
                user: mod_,
                role: Role::Moderator,
            },
        );
        let create = f.push(
            W::Owner,
            Payload::ChannelCreate {
                name: "secret".into(),
            },
        );
        f.channel = ChannelId(create.0);
        f
    }

    fn user(&self, w: W) -> &User {
        match w {
            W::Owner => &self.owner,
            W::Alice => &self.alice,
            W::Bob => &self.bob,
            W::Mod => &self.mod_,
        }
    }

    fn next(&mut self) -> Hlc {
        self.t += 1;
        at(self.t)
    }

    /// Adds a control event by `who`, citing the last one, and returns its id.
    fn push(&mut self, who: W, payload: Payload) -> EventId {
        let hlc = self.next();
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = event(self.user(who), parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).unwrap();
        self.last = Some(id);
        id
    }

    /// Adds an event on the private channel by `who`, citing the last one, and returns its id.
    fn push_on_channel(&mut self, who: W, payload: Payload) -> EventId {
        let hlc = self.next();
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = on_channel(self.user(who), self.channel, parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).unwrap();
        self.last = Some(id);
        id
    }

    /// Sets the channel's access, by `who`.
    fn access(&mut self, who: W, private: bool, roles: Vec<Role>, members: Vec<dissent_identity::RootId>) -> EventId {
        let channel = self.channel;
        self.push(
            who,
            Payload::ChannelAccess {
                channel,
                private,
                roles,
                members,
            },
        )
    }

    /// An event on `channel` by `who`, citing the last one.
    fn push_on_channel_at(&mut self, who: W, channel: ChannelId, payload: Payload) -> EventId {
        let hlc = self.next();
        let parents: Vec<EventId> = self.last.into_iter().collect();
        let ev = on_channel(self.user(who), channel, parents, hlc, payload);
        let id = ev.id;
        self.dag.insert(ev).unwrap();
        self.last = Some(id);
        id
    }

    /// A private-channel message by `who`, in the form a client writes.
    fn post(&mut self, who: W, text: &str) -> EventId {
        self.push_on_channel(
            who,
            Payload::PrivateSealed {
                kind: SealedKind::Message,
                generation: 0,
                ciphertext: text.as_bytes().to_vec(),
            },
        )
    }

    fn state(&self) -> crate::CommunityState {
        replay_with(self.owner.id(), &self.dag, &[])
    }

    fn rejection_of(&self, id: EventId) -> Option<Rejection> {
        self.state()
            .rejected
            .iter()
            .find(|(rejected, _)| *rejected == id)
            .map(|(_, why)| *why)
    }
}

#[test]
fn listed_members_and_admins_may_post_and_others_may_not() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let by_bob = f.post(W::Bob, "bob reads this");
    let by_alice = f.post(W::Alice, "an admin reads this");
    let by_mod = f.post(W::Mod, "not listed");
    assert_eq!(f.rejection_of(by_bob), None);
    assert_eq!(f.rejection_of(by_alice), None, "an admin reads every private channel");
    assert_eq!(f.rejection_of(by_mod), Some(Rejection::NoPermission));
}

#[test]
fn a_listed_role_opens_the_channel_to_its_holders() {
    let mut f = Fixture::new();
    f.access(W::Owner, true, vec![Role::Moderator], vec![]);
    let by_mod = f.post(W::Mod, "the moderators");
    assert_eq!(f.rejection_of(by_mod), None);
}

#[test]
fn only_an_owner_or_admin_may_change_access() {
    let mut f = Fixture::new();
    let refused = f.access(W::Mod, true, vec![], vec![]);
    assert_eq!(f.rejection_of(refused), Some(Rejection::NoPermission));
    assert!(!f.state().is_private(f.channel), "the refused change did not apply");
}

#[test]
fn a_plain_or_mls_message_in_a_private_channel_is_refused() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let plain = f.push_on_channel(
        W::Owner,
        Payload::Message {
            text: "in the clear".into(),
        },
    );
    let mls = f.push_on_channel(
        W::Owner,
        Payload::Sealed {
            epoch: 0,
            kind: SealedKind::Message,
            ciphertext: vec![1, 2, 3],
        },
    );
    assert_eq!(f.rejection_of(plain), Some(Rejection::NoPermission));
    assert_eq!(f.rejection_of(mls), Some(Rejection::NoPermission));
}

#[test]
fn a_plain_reply_or_edit_in_a_private_channel_is_refused() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let target = f.post(W::Bob, "hidden");
    let reply = f.push_on_channel(W::Bob, Payload::Reply { target, text: "in the clear".into() });
    assert_eq!(f.rejection_of(reply), Some(Rejection::NoPermission));
    let edit = f.push_on_channel(W::Bob, Payload::Edit { target, text: "in the clear".into() });
    assert_eq!(f.rejection_of(edit), Some(Rejection::NoPermission));
}

#[test]
fn a_private_message_in_a_public_channel_is_malformed() {
    let mut f = Fixture::new();
    let ev = f.post(W::Bob, "no access was ever set");
    assert_eq!(f.rejection_of(ev), Some(Rejection::Malformed));
}

#[test]
fn removing_a_reader_starts_a_new_key_generation() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    assert_eq!(f.state().key_generation.get(&f.channel), None, "granting access starts no new key");
    let removed = f.access(W::Owner, true, vec![], vec![]);
    assert_eq!(f.rejection_of(removed), None);
    assert_eq!(f.state().key_generation.get(&f.channel), Some(&1));
    let after = f.post(W::Bob, "after the removal");
    assert_eq!(f.rejection_of(after), Some(Rejection::NoPermission));
}

#[test]
fn a_change_that_only_adds_readers_keeps_the_generation() {
    let mut f = Fixture::new();
    let (bob, mod_) = (f.bob.id(), f.mod_.id());
    f.access(W::Owner, true, vec![], vec![bob]);
    f.access(W::Owner, true, vec![], vec![bob, mod_]);
    assert_eq!(f.state().key_generation.get(&f.channel).copied().unwrap_or(0), 0);
}

#[test]
fn adding_a_reader_under_since_join_starts_a_new_generation() {
    let mut f = Fixture::new();
    let (bob, mod_) = (f.bob.id(), f.mod_.id());
    f.push(W::Owner, Payload::HistoryVisibility { setting: crate::HistorySetting::SinceJoin });
    f.access(W::Owner, true, vec![], vec![bob]);
    assert_eq!(f.state().key_generation.get(&f.channel).copied().unwrap_or(0), 0, "the first grant starts no new key");
    f.access(W::Owner, true, vec![], vec![bob, mod_]);
    assert_eq!(f.state().key_generation.get(&f.channel), Some(&1), "the newcomer gets a generation of its own");
}

#[test]
fn the_latest_access_change_wins() {
    let mut f = Fixture::new();
    f.access(W::Owner, true, vec![], vec![]);
    f.access(W::Owner, false, vec![], vec![]);
    let by_mod = f.post(W::Mod, "public again");
    assert_eq!(f.rejection_of(by_mod), Some(Rejection::Malformed), "a public channel takes no private messages");
    assert!(!f.state().is_private(f.channel));
}

#[test]
fn a_banned_listed_member_cannot_post() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    f.push(W::Owner, Payload::Ban { user: bob });
    let by_bob = f.post(W::Bob, "banned");
    assert_eq!(f.rejection_of(by_bob), Some(Rejection::Banned));
}

#[test]
fn an_accepted_private_message_is_listed_like_any_sealed_message() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let msg = f.post(W::Bob, "listed");
    let st = f.state();
    let listed = st.messages.get(&f.channel).map(|m| m.iter().any(|v| v.id == msg)).unwrap_or(false);
    assert!(listed, "a private message has a view in its channel, with the text left out");
}

#[test]
fn every_message_kind_is_rate_limited_by_slowmode() {
    use crate::ChannelVisibility;
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let private = f.channel;
    // A public channel with the same 60 s slowmode, for the forms a public channel takes.
    let open_create = f.push(
        W::Owner,
        Payload::ChannelCreate {
            name: "open".into(),
        },
    );
    let open = ChannelId(open_create.0);
    for channel in [private, open] {
        f.push(
            W::Owner,
            Payload::ChannelUpdate {
                channel,
                name: None,
                slowmode_s: Some(60),
                visibility: None::<ChannelVisibility>,
                bitrate_kbps: None,
                user_limit: None,
                relay: None,
            },
        );
    }
    // Each form, on the channel it takes: the first post in the window is accepted, the second is refused.
    let forms: Vec<(&str, ChannelId, Payload)> = vec![
        (
            "private message",
            private,
            Payload::PrivateSealed {
                kind: SealedKind::Message,
                generation: 0,
                ciphertext: vec![1],
            },
        ),
        (
            "plain message",
            open,
            Payload::Message {
                text: "plain".into(),
            },
        ),
        (
            "sealed message",
            open,
            Payload::Sealed {
                epoch: 0,
                kind: SealedKind::Message,
                ciphertext: vec![1],
            },
        ),
        (
            "history message",
            open,
            Payload::Sealed {
                epoch: 0,
                kind: SealedKind::HistoryMessage,
                ciphertext: vec![1],
            },
        ),
    ];
    for (name, channel, payload) in forms {
        let first = f.push_on_channel_at(W::Bob, channel, payload.clone());
        assert_eq!(f.rejection_of(first), None, "{name}: the first post in the window is accepted");
        let second = f.push_on_channel_at(W::Bob, channel, payload);
        assert_eq!(f.rejection_of(second), Some(Rejection::Slowmode), "{name}: the second post waits out slowmode");
        // Let the window pass before the next form.
        f.t += 61_000;
    }
}

#[test]
fn the_history_form_is_refused_in_a_private_channel() {
    let mut f = Fixture::new();
    let bob = f.bob.id();
    f.access(W::Owner, true, vec![], vec![bob]);
    let history = f.push_on_channel(
        W::Bob,
        Payload::PrivateSealed {
            kind: SealedKind::HistoryMessage,
            generation: 0,
            ciphertext: vec![1],
        },
    );
    assert_eq!(f.rejection_of(history), Some(Rejection::Malformed));
}
