//! Member-sealed control events replay to the same state as plaintext ones.
//!
//! Each generated history is built twice from the same steps: once with control payloads in clear,
//! and once with each sealable payload sealed under the key of its MLS epoch. Replaying the sealed
//! DAG with every epoch key must give the same members, roles, bans, channels, settings and
//! rejections as the plaintext replay. Event ids differ between the two DAGs, so the comparison
//! ignores them, and channels are compared by name.

use std::collections::{BTreeMap, BTreeSet};

use dissent_events::{
    is_sealable, replay, replay_keyed, seal_member, CommunityState, Dag, EpochKey, EpochKeys,
    Event, EventBody, EventId, HistorySetting, Hlc, Payload, Rejection, Role, ChannelId,
};
use dissent_identity::{CommunityId, DeviceKey, RootId, RootKey};
use proptest::prelude::*;

const USERS: usize = 4;
const EPOCHS: usize = 12;

#[derive(Clone, Debug)]
enum Step {
    Join(usize, String),
    Leave(usize),
    Ban(usize, usize),
    Unban(usize, usize),
    Grant(usize, usize, bool),
    Revoke(usize, usize, bool),
    Channel(usize, String),
    Rotate(usize, u64),
    Message(usize, usize, String),
    /// A commit: the next epoch. Not a DAG event, so it changes only the keys.
    Commit,
    MemberOnly(usize, bool),
    History(usize, bool),
}

fn step() -> impl Strategy<Value = Step> {
    let user = 0..USERS;
    let name = "[a-z]{1,8}";
    prop_oneof![
        (user.clone(), name).prop_map(|(u, n)| Step::Join(u, n)),
        user.clone().prop_map(Step::Leave),
        (user.clone(), user.clone()).prop_map(|(a, b)| Step::Ban(a, b)),
        (user.clone(), user.clone()).prop_map(|(a, b)| Step::Unban(a, b)),
        (user.clone(), user.clone(), any::<bool>()).prop_map(|(a, b, m)| Step::Grant(a, b, m)),
        (user.clone(), user.clone(), any::<bool>()).prop_map(|(a, b, m)| Step::Revoke(a, b, m)),
        (user.clone(), "[a-z]{1,8}").prop_map(|(u, n)| Step::Channel(u, n)),
        (user.clone(), 0u64..4).prop_map(|(u, g)| Step::Rotate(u, g)),
        (user.clone(), 0usize..8, "[a-z ]{1,12}").prop_map(|(u, c, t)| Step::Message(u, c, t)),
        Just(Step::Commit),
        (user.clone(), any::<bool>()).prop_map(|(u, on)| Step::MemberOnly(u, on)),
        (user, any::<bool>()).prop_map(|(u, all)| Step::History(u, all)),
    ]
}

/// One user, with the root that owns the community and the device that signs its events.
struct Who {
    root: RootKey,
    device: DeviceKey,
}

impl Who {
    fn id(&self) -> RootId {
        self.root.id()
    }
}

/// Builds the DAG for `steps`. With `sealed`, each sealable payload is sealed under the key of the
/// epoch it was made in. Returns the DAG and the epoch keys a member would hold.
fn build(steps: &[Step], users: &[Who], exports: &[[u8; 32]], sealed: bool) -> (Dag, Vec<EpochKey>) {
    let mut dag = Dag::new();
    let mut epochs: Vec<EpochKey> = Vec::new();
    let mut epoch = 0u64;
    let mut prev: Option<EventId> = None;
    let mut channels: Vec<ChannelId> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let (payload, channel) = match step {
            Step::Commit => {
                epoch = (epoch + 1).min(EPOCHS as u64 - 1);
                continue;
            }
            Step::Join(_, name) => (Payload::Join { display_name: name.clone() }, ChannelId::CONTROL),
            Step::Leave(_) => (Payload::Leave, ChannelId::CONTROL),
            Step::Ban(_, t) => (Payload::Ban { user: users[*t].id() }, ChannelId::CONTROL),
            Step::Unban(_, t) => (Payload::Unban { user: users[*t].id() }, ChannelId::CONTROL),
            Step::Grant(_, t, admin) => (
                Payload::RoleGrant {
                    user: users[*t].id(),
                    role: if *admin { Role::Admin } else { Role::Moderator },
                },
                ChannelId::CONTROL,
            ),
            Step::Revoke(_, t, admin) => (
                Payload::RoleRevoke {
                    user: users[*t].id(),
                    role: if *admin { Role::Admin } else { Role::Moderator },
                },
                ChannelId::CONTROL,
            ),
            Step::Channel(_, name) => (Payload::ChannelCreate { name: name.clone() }, ChannelId::CONTROL),
            Step::Rotate(_, g) => (
                Payload::SecretRotation {
                    generation: *g,
                    covers: vec![],
                    sealed: vec![7; 4],
                },
                ChannelId::CONTROL,
            ),
            Step::Message(_, c, text) => {
                if channels.is_empty() {
                    continue;
                }
                let chan = channels[c % channels.len()];
                (Payload::Message { text: text.clone() }, chan)
            }
            Step::MemberOnly(_, on) => (Payload::MemberOnlyControl { on: *on }, ChannelId::CONTROL),
            Step::History(_, all) => (
                Payload::HistoryVisibility {
                    setting: if *all {
                        HistorySetting::All
                    } else {
                        HistorySetting::SinceJoin
                    },
                },
                ChannelId::CONTROL,
            ),
        };
        let author = match step {
            Step::Join(u, _)
            | Step::Leave(u)
            | Step::Ban(u, _)
            | Step::Unban(u, _)
            | Step::Grant(u, _, _)
            | Step::Revoke(u, _, _)
            | Step::Channel(u, _)
            | Step::Rotate(u, _)
            | Step::Message(u, _, _)
            | Step::MemberOnly(u, _)
            | Step::History(u, _) => *u,
            Step::Commit => unreachable!(),
        };
        let who = &users[author];
        let hlc = Hlc {
            wall_ms: i as u64 + 1,
            counter: 0,
        };
        let parents: Vec<EventId> = prev.into_iter().collect();
        let body = EventBody {
            community: CommunityId([0; 32]),
            channel,
            author: who.id(),
            device: who.device.id(),
            parents,
            hlc,
            payload: payload.clone(),
        };
        let body = if sealed && is_sealable(&payload) {
            let export = exports[epoch as usize];
            let keys = EpochKeys::from_export(&export);
            epochs.push((epoch, keys.enc));
            let sealed_payload = seal_member(&keys, epoch, &body).expect("sealable payload");
            EventBody {
                payload: sealed_payload,
                ..body
            }
        } else {
            body
        };
        let ev = Event::sign(body, &who.device);
        if matches!(step, Step::Channel(..)) {
            channels.push(ChannelId(ev.id.0));
        }
        prev = Some(ev.id);
        dag.insert(ev).expect("fresh event");
    }
    epochs.sort();
    epochs.dedup();
    (dag, epochs)
}

/// The state fields that must match, with event ids and channel ids left out.
#[derive(Debug, PartialEq, Eq)]
struct View {
    owner: RootId,
    roles: BTreeMap<RootId, BTreeSet<Role>>,
    banned: BTreeSet<RootId>,
    handles: BTreeMap<String, RootId>,
    channels: Vec<String>,
    voice_channels: usize,
    members: BTreeMap<RootId, dissent_events::Member>,
    messages: Vec<(RootId, Hlc, String)>,
    revoked_devices: usize,
    member_only: bool,
    history: HistorySetting,
    secret_generation: u64,
    secret_covered: BTreeSet<RootId>,
    rejected: Vec<Rejection>,
}

fn view(st: &CommunityState) -> View {
    let mut channels: Vec<String> = st.channels.values().cloned().collect();
    channels.sort();
    let mut messages: Vec<(RootId, Hlc, String)> = st
        .messages
        .values()
        .flatten()
        .map(|m| (m.author, m.hlc, m.text.clone()))
        .collect();
    messages.sort();
    View {
        owner: st.owner,
        roles: st.roles.clone(),
        banned: st.banned.clone(),
        handles: st.handles.clone(),
        channels,
        voice_channels: st.voice_channels.len(),
        members: st.members.clone(),
        messages,
        revoked_devices: st.revoked_devices.len(),
        member_only: st.member_only,
        history: st.history,
        secret_generation: st.secret_generation,
        secret_covered: st.secret_covered.clone(),
        rejected: st.rejected.iter().map(|(_, why)| *why).collect(),
    }
}

fn exports() -> impl Strategy<Value = Vec<[u8; 32]>> {
    prop::collection::vec(any::<[u8; 32]>(), EPOCHS)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Plaintext and member-sealed histories replay to the same community state.
    #[test]
    fn member_sealed_replay_matches_plaintext(
        steps in prop::collection::vec(step(), 1..48),
        exports in exports(),
    ) {
        let users: Vec<Who> = (0..USERS)
            .map(|_| Who {
                root: RootKey::generate(),
                device: DeviceKey::generate(),
            })
            .collect();
        let owner = users[0].id();

        let (plain, _) = build(&steps, &users, &exports, false);
        let (sealed, epochs) = build(&steps, &users, &exports, true);

        let want = view(&replay(owner, &plain));
        let got = view(&replay_keyed(owner, &sealed, &[], &epochs));
        prop_assert_eq!(got, want);
    }
}
