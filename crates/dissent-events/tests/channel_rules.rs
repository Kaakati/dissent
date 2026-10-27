//! Replay rules for channel settings, deletion, slowmode and invites. Each rule is applied by
//! `replay` to plain events, so a member's attempt is refused the same way on every device.

use dissent_events::state::ChannelSettings;
use dissent_events::{
    replay, ChannelId, ChannelVisibility, CommunityState, Dag, Event, EventBody, EventId, Hlc,
    Payload, Rejection, Role,
};
use dissent_identity::{CommunityKey, DeviceCert, DeviceKey, RootId, RootKey};

const OWNER: usize = 0;
const ADMIN: usize = 1;
const USER: usize = 2;
const NEW: usize = 3;

struct Member {
    root: RootKey,
    device: DeviceKey,
    _cert: DeviceCert,
}

/// Builds signed events for one community, with the wall time of each one given by the caller.
struct Log {
    community: dissent_identity::CommunityId,
    members: Vec<Member>,
    events: Vec<Event>,
    /// The latest event, which every new event names as its parent so the DAG stays one line.
    head: Option<EventId>,
}

impl Log {
    fn new() -> Self {
        let members = (0..4)
            .map(|_| {
                let root = RootKey::generate();
                let device = DeviceKey::generate();
                let cert = root.certify(device.id(), 0);
                Member { root, device, _cert: cert }
            })
            .collect();
        Self {
            community: CommunityKey::generate().id(),
            members,
            events: Vec::new(),
            head: None,
        }
    }

    fn root(&self, who: usize) -> RootId {
        self.members[who].root.id()
    }

    /// Adds an event by `who` at `wall` ms, and returns its id.
    fn push(&mut self, who: usize, chan: ChannelId, wall: u64, payload: Payload) -> EventId {
        let parents = self.head.into_iter().collect();
        let body = EventBody {
            community: self.community,
            channel: chan,
            author: self.root(who),
            device: self.members[who].device.id(),
            parents,
            hlc: Hlc { wall_ms: wall, counter: 0 },
            payload,
        };
        let ev = Event::sign(body, &self.members[who].device);
        let id = ev.id;
        self.events.push(ev);
        self.head = Some(id);
        id
    }

    fn replay(&self) -> CommunityState {
        let mut dag = Dag::new();
        for ev in &self.events {
            dag.insert(ev.clone()).unwrap();
        }
        assert!(dag.missing().is_empty());
        replay(self.root(OWNER), &dag)
    }

    /// The owner makes a channel, and returns its id.
    fn channel(&mut self, name: &str, voice: bool, wall: u64) -> ChannelId {
        let payload = if voice {
            Payload::VoiceChannelCreate { name: name.into() }
        } else {
            Payload::ChannelCreate { name: name.into() }
        };
        ChannelId(self.push(OWNER, ChannelId::CONTROL, wall, payload).0)
    }

    fn grant_admin(&mut self, user: usize, wall: u64) {
        let user = self.root(user);
        self.push(OWNER, ChannelId::CONTROL, wall, Payload::RoleGrant { user, role: Role::Admin });
    }

    fn post(&mut self, who: usize, chan: ChannelId, wall: u64, text: &str) -> EventId {
        self.push(who, chan, wall, Payload::Message { text: text.into() })
    }

    fn update(&mut self, who: usize, wall: u64, change: Payload) -> EventId {
        self.push(who, ChannelId::CONTROL, wall, change)
    }
}

fn update_slowmode(channel: ChannelId, seconds: u32) -> Payload {
    Payload::ChannelUpdate {
        channel,
        name: None,
        slowmode_s: Some(seconds),
        visibility: None,
        bitrate_kbps: None,
        user_limit: None,
        relay: None,
    }
}

fn rejected_as(st: &CommunityState, id: EventId) -> Option<Rejection> {
    st.rejected.iter().find(|(e, _)| *e == id).map(|(_, why)| *why)
}

#[test]
fn a_member_cannot_change_a_channel_but_an_admin_can() {
    let mut log = Log::new();
    let chan = log.channel("chat", false, 1);
    let member_try = log.update(USER, 2,
        Payload::ChannelUpdate {
            channel: chan,
            name: Some("hijacked".into()),
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: None,
            user_limit: None,
            relay: None,
        },
    );
    log.grant_admin(ADMIN, 3);
    let admin_change = log.update(ADMIN, 4,
        Payload::ChannelUpdate {
            channel: chan,
            name: Some("talk".into()),
            slowmode_s: Some(30),
            visibility: Some(ChannelVisibility::Spoiler),
            bitrate_kbps: None,
            user_limit: None,
            relay: None,
        },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, member_try), Some(Rejection::NoPermission));
    assert_eq!(rejected_as(&st, admin_change), None);
    assert_eq!(st.channels[&chan], "talk");
    assert_eq!(
        st.settings(&chan),
        ChannelSettings {
            slowmode_s: 30,
            visibility: ChannelVisibility::Spoiler,
            ..ChannelSettings::default()
        }
    );
}

#[test]
fn slowmode_holds_members_to_one_post_per_interval_and_exempts_admins() {
    let mut log = Log::new();
    let chan = log.channel("chat", false, 1);
    log.grant_admin(ADMIN, 2);
    log.update(OWNER, 3, update_slowmode(chan, 5));
    let first = log.post(USER, chan, 10_000, "first");
    let too_soon = log.post(USER, chan, 14_999, "too soon");
    let on_time = log.post(USER, chan, 15_000, "on time");
    let admin_a = log.post(ADMIN, chan, 15_100, "admin a");
    let admin_b = log.post(ADMIN, chan, 15_200, "admin b");
    let st = log.replay();
    assert_eq!(rejected_as(&st, first), None);
    assert_eq!(rejected_as(&st, too_soon), Some(Rejection::Slowmode));
    assert_eq!(rejected_as(&st, on_time), None, "exactly the interval later is allowed");
    assert_eq!(rejected_as(&st, admin_a), None);
    assert_eq!(rejected_as(&st, admin_b), None, "admins are not held to slowmode");
}

#[test]
fn slowmode_refuses_a_voice_channel_and_an_unlisted_step() {
    let mut log = Log::new();
    let voice = log.channel("lounge", true, 1);
    let text = log.channel("chat", false, 2);
    let on_voice = log.update(OWNER, 3, update_slowmode(voice, 5));
    let odd_step = log.update(OWNER, 4, update_slowmode(text, 7));
    let st = log.replay();
    assert_eq!(rejected_as(&st, on_voice), Some(Rejection::Malformed));
    assert_eq!(rejected_as(&st, odd_step), Some(Rejection::Malformed));
}

#[test]
fn voice_fields_are_refused_on_a_text_channel_and_limited_in_range() {
    let mut log = Log::new();
    let text = log.channel("chat", false, 1);
    let voice = log.channel("lounge", true, 2);
    let bitrate_on_text = log.update(OWNER, 3,
        Payload::ChannelUpdate {
            channel: text,
            name: None,
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: Some(32),
            user_limit: None,
            relay: None,
        },
    );
    let bitrate_too_high = log.update(OWNER, 4,
        Payload::ChannelUpdate {
            channel: voice,
            name: None,
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: Some(300),
            user_limit: None,
            relay: None,
        },
    );
    let limit_too_high = log.update(OWNER, 5,
        Payload::ChannelUpdate {
            channel: voice,
            name: None,
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: None,
            user_limit: Some(100),
            relay: None,
        },
    );
    let relay_not_named = log.update(OWNER, 6,
        Payload::ChannelUpdate {
            channel: voice,
            name: None,
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: None,
            user_limit: None,
            relay: Some("https://elsewhere.example".into()),
        },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, bitrate_on_text), Some(Rejection::Malformed));
    assert_eq!(rejected_as(&st, bitrate_too_high), Some(Rejection::Malformed));
    assert_eq!(rejected_as(&st, limit_too_high), Some(Rejection::Malformed));
    assert_eq!(rejected_as(&st, relay_not_named), Some(Rejection::Malformed));
}

#[test]
fn a_member_cannot_delete_a_channel_and_an_admin_can() {
    let mut log = Log::new();
    let chan = log.channel("doomed", false, 1);
    log.grant_admin(ADMIN, 2);
    let member_delete = log.update(USER, 3, Payload::ChannelDelete { channel: chan });
    let admin_delete = log.update(ADMIN, 4, Payload::ChannelDelete { channel: chan });
    let after = log.post(USER, chan, 5, "into the void");
    let st = log.replay();
    assert_eq!(rejected_as(&st, member_delete), Some(Rejection::NoPermission));
    assert_eq!(rejected_as(&st, admin_delete), None);
    assert!(!st.channels.contains_key(&chan));
    assert_eq!(rejected_as(&st, after), Some(Rejection::UnknownChannel));
}

#[test]
fn an_invite_admits_until_it_is_revoked_or_expires() {
    let mut log = Log::new();
    let invite = [7u8; 16];
    let expiring = [8u8; 16];
    log.push(OWNER, ChannelId::CONTROL, 1, Payload::InviteCreate { invite_id: invite, expires_ms: None });
    log.push(
        OWNER,
        ChannelId::CONTROL,
        2,
        Payload::InviteCreate { invite_id: expiring, expires_ms: Some(20_000) },
    );
    let early = log.push(
        USER,
        ChannelId::CONTROL,
        19_000,
        Payload::JoinVia { display_name: "user".into(), invite_id: expiring },
    );
    let late = log.push(
        NEW,
        ChannelId::CONTROL,
        20_000,
        Payload::JoinVia { display_name: "new".into(), invite_id: expiring },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, early), None, "before the expiry, the invite joins");
    assert_eq!(rejected_as(&st, late), Some(Rejection::InviteExpired));
    assert_eq!(st.invites[&expiring].uses, 1);

    log.push(OWNER, ChannelId::CONTROL, 21_000, Payload::InviteRevoke { invite_id: invite });
    let revoked_join = log.push(
        NEW,
        ChannelId::CONTROL,
        22_000,
        Payload::JoinVia { display_name: "new".into(), invite_id: invite },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, revoked_join), Some(Rejection::InviteRevoked));
    assert!(st.invites[&invite].revoked);
    assert!(!st.members.contains_key(&log.root(NEW)), "neither refused join made NEW a member");
}

#[test]
fn a_join_through_an_unknown_invite_is_refused() {
    let mut log = Log::new();
    let join = log.push(
        NEW,
        ChannelId::CONTROL,
        1,
        Payload::JoinVia { display_name: "new".into(), invite_id: [9u8; 16] },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, join), Some(Rejection::UnknownInvite));
}

#[test]
fn only_an_admin_can_revoke_an_invite() {
    let mut log = Log::new();
    let invite = [5u8; 16];
    log.push(OWNER, ChannelId::CONTROL, 1, Payload::InviteCreate { invite_id: invite, expires_ms: None });
    let member_revoke = log.push(USER, ChannelId::CONTROL, 2, Payload::InviteRevoke { invite_id: invite });
    let st = log.replay();
    assert_eq!(rejected_as(&st, member_revoke), Some(Rejection::NoPermission));
    assert!(!st.invites[&invite].revoked);
}

/// A client whose range stops at 96 kbps (the releases before boosts) rejects a higher setting as
/// malformed. Modelled here with a setting over today's limit: only that one update is dropped, and
/// the updates and posts after it still apply.
#[test]
fn an_out_of_range_bitrate_drops_only_that_update() {
    let mut log = Log::new();
    let voice = log.channel("lounge", true, 1);
    let too_high = log.update(OWNER, 2,
        Payload::ChannelUpdate {
            channel: voice,
            name: Some("too-loud".into()),
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: Some(300),
            user_limit: None,
            relay: None,
        },
    );
    let later = log.update(OWNER, 3,
        Payload::ChannelUpdate {
            channel: voice,
            name: Some("lounge-2".into()),
            slowmode_s: None,
            visibility: None,
            bitrate_kbps: Some(80),
            user_limit: None,
            relay: None,
        },
    );
    let st = log.replay();
    assert_eq!(rejected_as(&st, too_high), Some(Rejection::Malformed));
    assert_eq!(rejected_as(&st, later), None, "the next update is not dragged down with it");
    assert_eq!(st.channels[&voice], "lounge-2");
    assert_eq!(st.settings(&voice).bitrate_kbps, 80);
    assert_eq!(st.rejected.len(), 1);
}
