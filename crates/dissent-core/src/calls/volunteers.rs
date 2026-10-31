//! Volunteer forwarders: members who are not in a voice channel call and forward it for the people
//! who are. See `docs/design/13-forwarded-calls.md`, "Who forwards" and phase 3.
//!
//! - A member turns on "Help forward calls in this server" for one server
//!   (`Engine::set_forward_calls`). It is off by default, and kept on this device only.
//! - While that device is in no call itself, it offers to forward one call it hears of in that
//!   server (`CallMsg::ForwardOffer`), to every device in the call. The offer is signed, renewed
//!   every `forward_offer_every`, and lapses after three of those. Leaving withdraws it.
//! - Each device in the call checks an offer before it keeps it: a current, unbanned member of the
//!   server, in its MLS group, and not a participant. It checks again before it links to one.
//! - The election ranks the participants and the live offers together. A volunteer gets a small,
//!   capped bonus, since it spends no upload on media of its own. Hosting a relay only breaks a tie
//!   between volunteers. Volunteers count only while every device in the call sent
//!   `FEATURE_VOLUNTEERS`, so a released client never sees one.
//! - A leaf links to an elected volunteer as to a participant forwarder, but never lists it as a
//!   participant. The volunteer builds its relay when a device of the call names it in a signed
//!   tier message. The relay has no leaf and holds no media key: SFrame stays end to end.
//! - When the volunteer's offer is withdrawn or lapses, or it is banned, each device moves the call
//!   to a participant forwarder at once, through the same migration as any tier change.
//! - A seeder forwards too, as a volunteer, once the server's owner allows it ("Seeder may forward
//!   calls", `Payload::SeederForwardsCalls`). It is outside the MLS group, so it has no leaf index:
//!   it takes the sender id `device_sid` gives its device, which the call's devices keep beside the
//!   group's ids while its offer holds, and each leaf tells it the leaf's own id (`RelayLeaf`).
//!   Seeders count only while every device in the call sent `FEATURE_SEEDER_FORWARDERS`.
//! - A volunteer that stays linked but stops forwarding is caught by its leaves: a leaf that hears no
//!   audio through it for `STALL_WINDOW` tells every device in the call (`ForwarderStalled`). Once
//!   two leaves report it, and at least one of them was speaking all along, every device excludes
//!   that volunteer for the rest of the call and fails over, as for a lapse.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use dissent_identity::{CommunityId, DeviceId, DeviceKey, RootId, Signature};
use dissent_media::{capacity, compare, Candidate, MediaKey, Mesh, Mode, Sfu};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use tracing::debug;

use super::{
    candidate, device_sid, elect_forwarder, gen_id, lock, pump_forwarded, CallMsg, CallReply, Forwarding, Inner, Role, Room,
    RoomId, Scope, Tier, TierMsg, CAPACITY_FRESH, DEFAULT_KBPS, FEATURES, FEATURE_SEEDER_FORWARDERS, FEATURE_VOLUNTEERS, FRAME_MS,
    MEMBERSHIP_EVERY,
};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::timings::millis;
use crate::world::World;

/// Domain tag of a signed forward offer.
const OFFER_DOMAIN: &[u8] = b"dissent/call-forward-offer/v1";
/// The longest a received offer holds, whatever it asks for, in ms.
const MAX_OFFER_TTL_MS: u32 = 60_000;
/// An offer lapses after this many renewal intervals.
const OFFER_LIFETIMES: u32 = 3;
/// The fewest people a call needs before a volunteer offers to forward it. A smaller call is a
/// mesh, which no forwarder serves.
const MIN_PEOPLE: usize = 6;
/// Whether a server's seeders hear of a voice call with `people` in it. Only while the owner lets seeders forward calls
/// (`seeders_forward`), and
/// only for a call big enough to need a forwarder, since that is all a seeder hears a call for. The empty roster that
/// ends a call goes to them too, so a seeder forgets it at once.
pub(crate) fn seeders_hear_call(seeders_forward: bool, people: usize) -> bool {
    seeders_forward && (people == 0 || people >= MIN_PEOPLE)
}

/// A roster as a seeder gets it: who is in the call, which is what it counts, without whether each is muted or deafened.
pub(crate) fn for_seeder(members: &[super::PresenceMember]) -> Vec<super::PresenceMember> {
    members
        .iter()
        .map(|m| super::PresenceMember { root: m.root, muted: false, deafened: false })
        .collect()
}

/// A volunteer's bonus in the ranking is this share of its capacity...
const BONUS_DIVISOR: u32 = 10;
/// ...and at most this, so a strong participant still wins over a weak volunteer.
const BONUS_CAP_KBPS: u32 = 1_000;
/// How long a volunteer keeps a relay that no leaf is linked to.
const RELAY_IDLE: Duration = Duration::from_secs(10);
/// How many relays a volunteer runs at once: the call's room and the room it migrates to.
const MAX_RELAYS: usize = 2;
/// How long shutting down waits to withdraw this device's offers.
const WITHDRAW_WAIT: Duration = Duration::from_secs(2);
/// A leaf of a volunteer that hears no audio through it for this long reports it stalled.
pub(crate) const STALL_WINDOW: Duration = Duration::from_secs(5);
/// How long a stall report counts. Short, so a speaker's report and a listener's cover the same
/// stretch of time: with both fresh, the speaker spoke for at least two seconds of the listener's
/// silence.
const STALL_REPORT_FRESH: Duration = Duration::from_secs(3);
/// The longest pause in a leaf's audio that still counts as one stretch of speech.
const SPEECH_GAP: Duration = Duration::from_secs(1);
/// How often a leaf that still hears nothing reports again.
const STALL_REPORT_EVERY: Duration = Duration::from_secs(2);
/// Reports from this many leaves exclude a volunteer. A forwarded call has at least seven devices,
/// so this is never more than a majority.
const STALL_REPORTERS: usize = 2;

/// A leaf's view of its volunteer forwarder: when it last heard and sent audio, and the stall
/// reports it holds.
pub(crate) struct StallWatch {
    /// When the room started. A leaf that never heard anything counts from here.
    since: Instant,
    /// The last audio frame received through the forwarder, from anyone.
    pub heard: Option<Instant>,
    /// The last audio frame this device sent.
    spoke: Option<Instant>,
    /// When this device's current stretch of speech began.
    speaking_since: Option<Instant>,
    /// When this device last reported the forwarder stalled.
    reported: Option<Instant>,
    /// Reports by sender id: whether that leaf was speaking, and when the report came.
    reports: BTreeMap<u64, (bool, Instant)>,
}

impl StallWatch {
    pub(crate) fn new() -> Self {
        Self { since: Instant::now(), heard: None, spoke: None, speaking_since: None, reported: None, reports: BTreeMap::new() }
    }

    /// A new room's watch, with this one's stall reports.
    pub(crate) fn carried(&self) -> Self {
        Self { reports: self.reports.clone(), ..Self::new() }
    }

    /// Records an audio frame this device sent at `now`.
    pub(crate) fn note_spoke(&mut self, now: Instant) {
        if self.spoke.is_none_or(|t| now.duration_since(t) > SPEECH_GAP) {
            self.speaking_since = Some(now);
        }
        self.spoke = Some(now);
    }

    /// Whether this leaf has heard no audio for `STALL_WINDOW`, and if so whether it has been
    /// speaking all that time, up to now. A leaf that only just started, or just stopped, did not.
    fn deaf(&self, now: Instant) -> Option<bool> {
        let quiet = now.duration_since(self.heard.unwrap_or(self.since)) >= STALL_WINDOW;
        let speaking = self.spoke.is_some_and(|t| now.duration_since(t) <= SPEECH_GAP)
            && self.speaking_since.is_some_and(|t| now.duration_since(t) >= STALL_WINDOW);
        quiet.then_some(speaking)
    }
}

/// Whether the stall reports exclude the forwarder: enough distinct leaves heard nothing, and one
/// of them was speaking, so the others should have heard it. In a quiet call nobody speaks, so
/// leaves that hear nothing never add up to a stall. `reports` holds whether each reporter spoke.
pub(crate) fn stall_confirmed(reports: &[bool]) -> bool {
    reports.len() >= STALL_REPORTERS && reports.iter().any(|spoke| *spoke)
}

/// A member's offer to forward a call it is not in, or the withdrawal of one. Signed by its device
/// key. Appended to `CallMsg`, so only devices that sent `FEATURE_VOLUNTEERS` use it; a released one
/// fails to decode it and answers with an error.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ForwardOffer {
    pub room: RoomId,
    /// The upload the volunteer offers, in kbps.
    pub capacity_kbps: u32,
    pub wired: bool,
    pub on_battery: bool,
    /// The volunteer's `FEATURE_*` bits, which it cannot send as `Features` since it is not live.
    pub features: u64,
    /// How long the offer holds from its arrival, in ms. Zero withdraws it. Relative, so the
    /// devices' clocks need not agree.
    pub ttl_ms: u32,
    pub sig: Signature,
}

fn offer_bytes(room: &RoomId, capacity_kbps: u32, wired: bool, on_battery: bool, features: u64, ttl_ms: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(OFFER_DOMAIN.len() + 32 + 4 + 2 + 8 + 4);
    bytes.extend_from_slice(OFFER_DOMAIN);
    bytes.extend_from_slice(room);
    bytes.extend_from_slice(&capacity_kbps.to_le_bytes());
    bytes.push(u8::from(wired));
    bytes.push(u8::from(on_battery));
    bytes.extend_from_slice(&features.to_le_bytes());
    bytes.extend_from_slice(&ttl_ms.to_le_bytes());
    bytes
}

impl ForwardOffer {
    pub(crate) fn sign(key: &DeviceKey, room: RoomId, capacity_kbps: u32, features: u64, ttl_ms: u32) -> Self {
        // The app does not read the power source or the link type yet, so a volunteer reports a
        // wired device on mains, as the election assumes of every participant.
        let (wired, on_battery) = (true, false);
        let sig = key.sign(&offer_bytes(&room, capacity_kbps, wired, on_battery, features, ttl_ms));
        Self { room, capacity_kbps, wired, on_battery, features, ttl_ms, sig }
    }

    pub(crate) fn verify(&self, sender: &DeviceId) -> Result<()> {
        let bytes = offer_bytes(&self.room, self.capacity_kbps, self.wired, self.on_battery, self.features, self.ttl_ms);
        sender
            .verify(&bytes, &self.sig)
            .map_err(|_| CoreError::Rejected("bad forward offer signature".into()))
    }
}

/// A live offer, as a device in the call keeps it.
#[derive(Clone, Debug)]
pub(crate) struct VolunteerOffer {
    pub device: DeviceId,
    pub root: RootId,
    pub kbps: u32,
    pub wired: bool,
    pub on_battery: bool,
    /// Whether the volunteer is a seeder, a storage member outside the MLS group.
    pub storage: bool,
    /// When the offer lapses unless renewed.
    pub until: Instant,
}

/// A volunteer as the election sees it.
#[derive(Clone, Debug)]
pub(crate) struct Volunteer {
    pub sid: u64,
    pub offer: VolunteerOffer,
    /// Whether the volunteer hosts a relay for the server. A tie-break between volunteers only.
    pub relay_host: bool,
}

/// The bonus a volunteer gets in the ranking. See `BONUS_DIVISOR`.
pub(crate) fn volunteer_bonus(kbps: u32) -> u32 {
    (kbps / BONUS_DIVISOR).min(BONUS_CAP_KBPS)
}

/// The forwarder for a call of `n` live devices: the participants, at the election's default
/// capacity as a migration assumes, and the volunteers at their offered capacity plus the bonus.
/// With no volunteers it picks what `elect_forwarder` picks on the roster alone, so a call without
/// volunteers elects as it always has.
pub(crate) fn elect_with_volunteers(participants: &[u64], volunteers: &[Volunteer], n: usize) -> Option<u64> {
    let mut set: Vec<(Candidate, bool)> = participants
        .iter()
        .map(|sid| (candidate(*sid, DEFAULT_KBPS), false))
        .filter(|(c, _)| capacity(c.upload_kbps, Mode::AudioTop3) >= n)
        .collect();
    set.extend(
        volunteers
            .iter()
            .filter(|v| capacity(v.offer.kbps, Mode::AudioTop3) >= n)
            .map(|v| {
                let ranked = Candidate {
                    peer: v.sid,
                    upload_kbps: v.offer.kbps.saturating_add(volunteer_bonus(v.offer.kbps)),
                    on_battery: v.offer.on_battery,
                    wired: v.offer.wired,
                    cpu_ok: true,
                    rtt_ms_median: candidate(v.sid, 0).rtt_ms_median,
                };
                (ranked, v.relay_host)
            }),
    );
    // The election's order, then a hosted relay, then the lower id.
    let anonymous = |c: &Candidate| Candidate { peer: 0, ..c.clone() };
    set.sort_by(|(a, a_relay), (b, b_relay)| {
        compare(&anonymous(a), &anonymous(b))
            .then_with(|| b_relay.cmp(a_relay))
            .then_with(|| a.peer.cmp(&b.peer))
    });
    set.first().map(|(c, _)| c.peer)
}

/// A call this device offered to forward.
struct Offered {
    scope: Scope,
    /// The call's devices this offer was sent to.
    devices: BTreeSet<DeviceId>,
    /// When the offer was last sent to all of them.
    sent: Instant,
}

/// A volunteer's own state: the calls it offered to forward, and its relays for calls it is not in.
#[derive(Default)]
pub(crate) struct Volunteering {
    offered: Mutex<BTreeMap<RoomId, Offered>>,
    relays: Mutex<Vec<Arc<Room>>>,
    /// In this device's own call: the volunteers it stopped using because enough leaves reported
    /// them stalled, by call. Kept per call rather than per room, so every room the call migrates
    /// to, staged or current, excludes them for the rest of the call.
    excluded: Mutex<BTreeMap<RoomId, BTreeSet<u64>>>,
}

fn prefs_path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("forward_calls.bin")
}

/// Runs for the engine's life: every quarter of `forward_offer_every`, offers to forward a call in
/// the servers this device volunteers for, and renews or withdraws its offers.
pub(crate) async fn volunteer_loop(weak: Weak<Inner>) {
    while let Some(every) = weak.upgrade().map(|inner| inner.timings.forward_offer_every) {
        tokio::time::sleep(every / 4).await;
        let Some(inner) = weak.upgrade() else {
            break;
        };
        Engine { inner }.offer_round().await;
    }
}

impl Engine {
    /// The servers this device volunteers to forward calls for. Kept on this device only.
    fn forward_calls_set(&self) -> BTreeSet<CommunityId> {
        let paths = self.paths();
        paths.read_bin(&prefs_path(&self.read())).ok().flatten().unwrap_or_default()
    }

    /// Whether this device volunteers in `community` now: a person who turned on "Help Forward
    /// Calls", or a seeder in a server whose owner allows seeders to forward calls.
    fn volunteers_in(&self, community: CommunityId, set: &BTreeSet<CommunityId>) -> bool {
        set.contains(&community) || self.seeder_may_forward(community)
    }

    /// Whether this device is a seeder (a storage member) of `community`, and its owner allows
    /// seeders to forward calls.
    fn seeder_may_forward(&self, community: CommunityId) -> bool {
        let w = self.read();
        w.communities.get(&community).is_some_and(|c| {
            c.state().seeder_forwards_calls && c.state().members.get(&self.inner.me).is_some_and(|m| m.storage && !m.left)
        })
    }

    /// Whether `community`'s owner lets its seeders forward calls ("Seeder may forward calls").
    pub fn seeder_forwards_calls(&self, community: CommunityId) -> bool {
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().seeder_forwards_calls)
    }

    /// Lets the community's seeders forward its voice calls, or stops them. The owner only, which
    /// replay enforces. Off by default.
    pub async fn set_seeder_forwards_calls(&self, community: CommunityId, on: bool) -> Result<()> {
        self.author_publish(community, dissent_events::ChannelId::CONTROL, dissent_events::Payload::SeederForwardsCalls { on })
            .await?;
        self.emit(crate::views::EngineEvent::CommunityUpdated {
            community_id: community.to_string(),
        });
        Ok(())
    }

    /// Whether this device helps forward calls in `community` ("Help forward calls in this
    /// server"). Off unless turned on.
    pub fn forwards_calls(&self, community: CommunityId) -> bool {
        self.forward_calls_set().contains(&community)
    }

    /// Turns "Help forward calls in this server" on or off for a server this device belongs to.
    /// Off withdraws this device's offers in that server and stops its relays there.
    pub fn set_forward_calls(&self, community: CommunityId, on: bool) -> Result<()> {
        if !self.read().communities.contains_key(&community) {
            return Err(CoreError::NotFound(format!("community {community}")));
        }
        let (paths, path) = (self.paths(), prefs_path(&self.read()));
        {
            let _serial = self.write();
            let mut set: BTreeSet<CommunityId> = paths.read_bin(&path).ok().flatten().unwrap_or_default();
            if on {
                set.insert(community);
            } else {
                set.remove(&community);
            }
            paths.write_bin(&path, &set)?;
        }
        if !on {
            let engine = self.clone();
            self.spawn_task(async move { engine.withdraw_forward_offers(Some(community)).await });
        }
        Ok(())
    }

    /// This device's sender id in `community`'s calls: its MLS leaf index.
    /// A seeder is outside the group: its id is the one `device_sid` gives its device.
    fn my_sid_in(&self, community: CommunityId) -> Option<u64> {
        let me = self.inner.identity.device_id();
        if self.seeder_may_forward(community) {
            return Some(device_sid(&me));
        }
        self.indexed_members(community)
            .ok()?
            .into_iter()
            .find(|(_, device, _)| *device == me)
            .map(|(sid, _, _)| sid)
    }

    /// The upload this device offers as a volunteer, in kbps. It is in no call, so nothing measures
    /// it: the election's default, unless a test overrides it.
    fn offered_kbps(&self) -> u32 {
        match self.inner.calls.capacity_override.load(Ordering::SeqCst) {
            0 => DEFAULT_KBPS,
            kbps => kbps,
        }
    }

    fn offer_ttl_ms(&self) -> u32 {
        let ttl = millis(self.inner.timings.forward_offer_every).saturating_mul(u64::from(OFFER_LIFETIMES));
        u32::try_from(ttl).unwrap_or(u32::MAX).min(MAX_OFFER_TTL_MS)
    }

    /// One round of the volunteer loop. A device in a call of its own volunteers for none. Otherwise
    /// it offers to forward one call in a server it volunteers for: the one it relays already, else
    /// the largest it hears of. It withdraws its offer to any other call.
    async fn offer_round(&self) {
        if self.inner.legacy_calls.load(Ordering::Relaxed) {
            return;
        }
        if lock(&self.inner.calls.current).is_some() {
            self.withdraw_forward_offers(None).await;
            return;
        }
        let volunteered = self.forward_calls_set();
        // Relays in a server this device no longer volunteers for stop.
        let keep: BTreeSet<CommunityId> = lock(&self.inner.calls.volunteering.relays)
            .iter()
            .filter_map(|room| match room.scope {
                Scope::Channel { community, .. } => Some(community),
                _ => None,
            })
            .filter(|community| self.volunteers_in(*community, &volunteered))
            .collect();
        lock(&self.inner.calls.volunteering.relays).retain(|room| match room.scope {
            Scope::Channel { community, .. } => keep.contains(&community),
            _ => false,
        });
        let calls = self.forwardable_calls(&volunteered);
        let serving = lock(&self.inner.calls.volunteering.relays).last().map(|room| room.id);
        let chosen = serving
            .filter(|id| calls.contains_key(id))
            .or_else(|| {
                calls
                    .iter()
                    .max_by(|a, b| a.1 .2.cmp(&b.1 .2).then_with(|| b.0.cmp(a.0)))
                    .map(|(id, _)| *id)
            });
        let every = self.inner.timings.forward_offer_every;
        let mut sends: Vec<(RoomId, Vec<DeviceId>)> = Vec::new();
        let mut withdrawals: Vec<(RoomId, Vec<DeviceId>)> = Vec::new();
        {
            let mut offered = lock(&self.inner.calls.volunteering.offered);
            let stale: Vec<RoomId> = offered.keys().copied().filter(|id| Some(*id) != chosen).collect();
            for id in stale {
                if let Some(old) = offered.remove(&id) {
                    withdrawals.push((id, old.devices.into_iter().collect()));
                }
            }
            if let Some((id, (scope, devices, _))) = chosen.and_then(|id| calls.get(&id).map(|call| (id, call))) {
                let entry = offered.entry(id).or_insert_with(|| Offered {
                    scope: *scope,
                    devices: BTreeSet::new(),
                    sent: Instant::now(),
                });
                // A new device of the call hears the offer now. Everyone hears it once per interval.
                let renew = entry.devices.is_empty() || entry.sent.elapsed() >= every;
                let targets: Vec<DeviceId> = devices
                    .iter()
                    .copied()
                    .filter(|device| renew || !entry.devices.contains(device))
                    .collect();
                if renew {
                    entry.sent = Instant::now();
                }
                entry.devices.extend(targets.iter().copied());
                if !targets.is_empty() {
                    sends.push((id, targets));
                }
            }
        }
        for (id, devices) in withdrawals {
            self.send_offers(id, devices, 0).await;
        }
        let ttl = self.offer_ttl_ms();
        for (id, devices) in sends {
            self.send_offers(id, devices, ttl).await;
        }
    }

    /// The voice channel calls this device hears of in the servers it volunteers for, by room: the
    /// scope, the devices in the call, and how many people are in it. Only calls of `MIN_PEOPLE` or
    /// more, in servers where this device is a member with a sender id.
    fn forwardable_calls(&self, volunteered: &BTreeSet<CommunityId>) -> BTreeMap<RoomId, (Scope, BTreeSet<DeviceId>, usize)> {
        let now = crate::engine::now_ms();
        let ttl = millis(self.inner.timings.call_presence_ttl);
        let me = self.inner.me;
        let heard: Vec<(DeviceId, super::Heard)> = lock(&self.inner.calls.heard)
            .iter()
            .filter(|(_, h)| now.saturating_sub(h.seen_ms) < ttl)
            .map(|(device, h)| (*device, h.clone()))
            .collect();
        let mut calls: BTreeMap<RoomId, (Scope, BTreeSet<DeviceId>, BTreeSet<RootId>)> = BTreeMap::new();
        for (device, h) in heard {
            let Scope::Channel { community, .. } = h.scope else {
                continue;
            };
            if !self.volunteers_in(community, volunteered) || h.members.iter().any(|m| m.root == me) {
                continue;
            }
            let entry = calls.entry(h.room).or_insert_with(|| (h.scope, BTreeSet::new(), BTreeSet::new()));
            entry.1.insert(device);
            entry.2.extend(h.members.iter().map(|m| m.root));
        }
        let usable: BTreeSet<CommunityId> = {
            let w = self.read();
            calls
                .values()
                .filter_map(|(scope, _, _)| match scope {
                    Scope::Channel { community, .. } => Some(*community),
                    _ => None,
                })
                .filter(|id| w.communities.get(id).is_some_and(|c| c.is_member(&me) && !c.state().banned.contains(&me)))
                .collect()
        };
        calls
            .into_iter()
            .filter(|(_, (scope, _, people))| {
                people.len() >= MIN_PEOPLE
                    && matches!(scope, Scope::Channel { community, .. } if usable.contains(community) && self.my_sid_in(*community).is_some())
            })
            .map(|(id, (scope, devices, people))| (id, (scope, devices, people.len())))
            .collect()
    }

    /// Sends this device's signed offer for room `id` to `devices`, in parallel and acknowledged.
    /// A `ttl_ms` of zero withdraws it.
    async fn send_offers(&self, id: RoomId, devices: Vec<DeviceId>, ttl_ms: u32) {
        let offer = ForwardOffer::sign(&self.inner.identity.device, id, self.offered_kbps(), FEATURES, ttl_ms);
        let mut sends = tokio::task::JoinSet::new();
        for device in devices {
            let engine = self.clone();
            let msg = CallMsg::ForwardOffer(Box::new(offer.clone()));
            sends.spawn(async move {
                match engine.call_rpc(device, msg).await {
                    Ok(CallReply::Ack) => {}
                    Ok(other) => debug!(device = %device, reply = ?other, "forward offer refused"),
                    Err(e) => debug!(device = %device, error = %e, "forward offer not delivered"),
                }
            });
        }
        while sends.join_next().await.is_some() {}
    }

    /// Withdraws this device's offers, in one server or all of them, and stops its relays there.
    pub(crate) async fn withdraw_forward_offers(&self, community: Option<CommunityId>) {
        let in_scope = |scope: &Scope| match (scope, community) {
            (_, None) => true,
            (Scope::Channel { community: c, .. }, Some(wanted)) => *c == wanted,
            _ => false,
        };
        lock(&self.inner.calls.volunteering.relays).retain(|room| !in_scope(&room.scope));
        let withdrawn: Vec<(RoomId, Vec<DeviceId>)> = {
            let mut offered = lock(&self.inner.calls.volunteering.offered);
            let ids: Vec<RoomId> = offered.iter().filter(|(_, o)| in_scope(&o.scope)).map(|(id, _)| *id).collect();
            ids.into_iter()
                .filter_map(|id| offered.remove(&id).map(|o| (id, o.devices.into_iter().collect())))
                .collect()
        };
        for (id, devices) in withdrawn {
            self.send_offers(id, devices, 0).await;
        }
    }

    /// Withdraws every offer before the engine stops, so the calls fail over now rather than when
    /// the offers lapse. Best effort.
    pub(crate) async fn withdraw_on_shutdown(&self) {
        let _ = tokio::time::timeout(WITHDRAW_WAIT, self.withdraw_forward_offers(None)).await;
    }

    // --- In the call: keeping offers and electing volunteers. ---

    /// Records or withdraws an offer to forward this device's call.
    pub(super) fn on_forward_offer(&self, from: DeviceId, offer: ForwardOffer) -> CallReply {
        let Some(room) = self.current_room(&offer.room) else {
            return CallReply::Refused("not in this call".into());
        };
        // Someone who may not forward hears the same as for a call this device is not in, so an offer
        // cannot be used to learn whether this device is in a call.
        let root = match self.check_volunteer(&room, &from) {
            Ok(root) => root,
            Err(why) => {
                debug!(device = %from, why, "forward offer refused");
                return CallReply::Refused("not in this call".into());
            }
        };
        if offer.verify(&from).is_err() {
            return CallReply::Refused("bad forward offer signature".into());
        }
        if offer.features & FEATURE_VOLUNTEERS == 0 {
            return CallReply::Refused("the volunteer cannot forward".into());
        }
        if room.live_sid(&from).is_some() {
            return CallReply::Refused("a participant does not volunteer".into());
        }
        let storage = self.is_storage(&room, &root);
        let sid = if storage {
            // A seeder has no leaf index. Its id must not be one the group already uses.
            let sid = device_sid(&from);
            let taken = lock(&room.leaves).iter().any(|(device, known)| *known == sid && *device != from);
            if taken {
                return CallReply::Refused("the seeder's sender id is taken".into());
            }
            sid
        } else {
            let Some(sid) = room.sid_of(&from) else {
                return CallReply::Refused("not in the server's group".into());
            };
            sid
        };
        let ttl = Duration::from_millis(u64::from(offer.ttl_ms.min(MAX_OFFER_TTL_MS)));
        let staged = lock(&self.inner.calls.staged).clone().filter(|s| s.id == room.id);
        for room in std::iter::once(&room).chain(staged.iter()) {
            if ttl.is_zero() {
                lock(&room.offers).remove(&sid);
                continue;
            }
            let kept = VolunteerOffer {
                device: from,
                root,
                kbps: offer.capacity_kbps,
                wired: offer.wired,
                on_battery: offer.on_battery,
                storage,
                until: Instant::now() + ttl,
            };
            if storage {
                lock(&room.leaves).insert(from, sid);
            }
            lock(&room.offers).insert(sid, kept);
            lock(&room.features).insert(sid, (offer.features, Instant::now()));
        }
        CallReply::Ack
    }

    /// Whether `device` may forward `room`'s call as a volunteer: a device of a current, unbanned
    /// member of the call's server. Returns that member.
    fn check_volunteer(&self, room: &Room, device: &DeviceId) -> std::result::Result<RootId, String> {
        let Scope::Channel { community, .. } = room.scope else {
            return Err("only voice channel calls take volunteers".into());
        };
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| "unknown community".to_string())?;
        let root = w.directory.root_of(device).ok_or_else(|| "unknown device".to_string())?;
        if c.state().banned.contains(&root) {
            return Err("banned".into());
        }
        if !c.is_member(&root) {
            return Err("not a member".into());
        }
        let storage = c.state().members.get(&root).is_some_and(|m| m.storage);
        if storage && !c.state().seeder_forwards_calls {
            return Err("the owner does not let seeders forward calls".into());
        }
        Ok(root)
    }

    /// Whether `root` is a storage member (a seeder) of `room`'s server.
    fn is_storage(&self, room: &Room, root: &RootId) -> bool {
        let Scope::Channel { community, .. } = room.scope else {
            return false;
        };
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.state().members.get(root).is_some_and(|m| m.storage))
    }

    /// The sender ids of the seeders that offer to forward `room`'s call, by device. They are outside
    /// the group, so the membership tick and a migration keep them beside the group's ids.
    pub(super) fn volunteer_leaves(&self, room: &Room) -> Vec<(DeviceId, u64)> {
        lock(&room.offers)
            .iter()
            .filter(|(_, offer)| offer.storage)
            .map(|(sid, offer)| (offer.device, *sid))
            .collect()
    }

    /// Whether every live device of `room` said it supports volunteers, recently.
    fn all_support_volunteers(&self, room: &Room) -> bool {
        self.all_support(room, FEATURE_VOLUNTEERS)
    }

    /// Whether every live device of `room` sent the feature `bit`, recently.
    fn all_support(&self, room: &Room, bit: u64) -> bool {
        if self.inner.legacy_calls.load(Ordering::Relaxed) {
            return false;
        }
        let live: Vec<u64> = lock(&room.live).keys().copied().collect();
        let features = lock(&room.features).clone();
        let now = Instant::now();
        live.iter().all(|sid| {
            *sid == room.my_sid
                || features.get(sid).is_some_and(|(bits, at)| bits & bit != 0 && now.duration_since(*at) < CAPACITY_FRESH)
        })
    }

    /// The live offer of volunteer `sid` in `room`, if this device may link to it now: the offer
    /// holds, the volunteer is still a current, unbanned member and not in the call, and every
    /// device in the call supports volunteers.
    pub(super) fn volunteer_offer(&self, room: &Room, sid: u64) -> Option<VolunteerOffer> {
        if lock(&room.live).contains_key(&sid) || self.is_excluded(room, sid) || !self.all_support_volunteers(room) {
            return None;
        }
        let offer = lock(&room.offers).get(&sid).cloned()?;
        if offer.until <= Instant::now() || room.sid_of(&offer.device) != Some(sid) {
            return None;
        }
        if offer.storage && !self.all_support(room, FEATURE_SEEDER_FORWARDERS) {
            return None;
        }
        self.check_volunteer(room, &offer.device).ok()?;
        Some(offer)
    }

    /// Every volunteer `room` may elect now.
    fn live_volunteers(&self, room: &Room) -> Vec<Volunteer> {
        let sids: Vec<u64> = lock(&room.offers).keys().copied().collect();
        let community = match room.scope {
            Scope::Channel { community, .. } => community,
            _ => return Vec::new(),
        };
        sids.into_iter()
            .filter_map(|sid| self.volunteer_offer(room, sid).map(|offer| (sid, offer)))
            .map(|(sid, offer)| {
                let relay_host = self.hosts_relay(community, &offer.root);
                Volunteer { sid, offer, relay_host }
            })
            .collect()
    }

    /// Whether `root` publishes hosted relays for `community`.
    fn hosts_relay(&self, community: CommunityId, root: &RootId) -> bool {
        self.read()
            .communities
            .get(&community)
            .and_then(|c| c.state().relay_hosts.get(root).map(|(_, relays)| !relays.is_empty()))
            .unwrap_or(false)
    }

    /// The forwarder a migration of `old` to the forwarded tier builds, and the volunteer behind it
    /// if it is one. A `forced` forwarder, named by a tier message or a re-election, is used when it
    /// is in the roster or a volunteer this device may link to. Otherwise the call elects on its
    /// roster and live offers.
    pub(super) fn migration_forwarder(
        &self,
        old: &Room,
        sids: &[u64],
        forced: Option<u64>,
    ) -> Result<(u64, Option<(DeviceId, RootId)>)> {
        match forced {
            Some(f) if sids.contains(&f) => return Ok((f, None)),
            Some(f) => {
                if let Some(offer) = self.volunteer_offer(old, f) {
                    return Ok((f, Some((offer.device, offer.root))));
                }
            }
            None => {}
        }
        let volunteers = self.live_volunteers(old);
        let forwarder = elect_with_volunteers(sids, &volunteers, sids.len())
            .ok_or_else(|| super::call_err("no device can forward this call"))?;
        let volunteer = volunteers
            .iter()
            .find(|v| v.sid == forwarder)
            .map(|v| (v.offer.device, v.offer.root));
        Ok((forwarder, volunteer))
    }

    /// The forwarded call's volunteer rules, run on each membership tick. A call forwarded by a
    /// volunteer keeps it while this device may link to it, and fails over to a participant at once
    /// when it may not. A call forwarded by a participant moves to a volunteer that wins the
    /// election and holds that for `reelect_sustain`. Returns true when the volunteer rules decided,
    /// so the capacity re-election does not also run.
    pub(super) fn volunteer_check(&self, room: &Arc<Room>, forwarding: &Forwarding) -> bool {
        let sids: Vec<u64> = lock(&room.live).keys().copied().collect();
        if forwarding.volunteer.is_some() {
            self.stall_tick(room, forwarding.forwarder);
            if self.volunteer_offer(room, forwarding.forwarder).is_none() && !room.migrating.swap(true, Ordering::SeqCst) {
                *lock(&room.forwarder_since) = None;
                // On the roster alone, so every device fails over to the same participant.
                match elect_forwarder(&sids, sids.len(), &BTreeMap::new()) {
                    Some(participant) => {
                        debug!(forwarder = participant, "volunteer forwarder gone; a participant takes over");
                        self.spawn_migration(room.clone(), Tier::Forwarded, Some(participant));
                    }
                    None => room.migrating.store(false, Ordering::SeqCst),
                }
            }
            return true;
        }
        let volunteers = self.live_volunteers(room);
        if volunteers.is_empty() {
            return false;
        }
        let Some(best) = elect_with_volunteers(&sids, &volunteers, sids.len()) else {
            return false;
        };
        if best == forwarding.forwarder || !volunteers.iter().any(|v| v.sid == best) {
            return false;
        }
        let since = *lock(&room.forwarder_since).get_or_insert_with(Instant::now);
        if since.elapsed() >= self.inner.timings.reelect_sustain && !room.migrating.swap(true, Ordering::SeqCst) {
            *lock(&room.forwarder_since) = None;
            self.spawn_migration(room.clone(), Tier::Forwarded, Some(best));
        }
        true
    }

    /// A leaf of volunteer `forwarder`, on each membership tick: reports the forwarder stalled to
    /// every live device while this leaf hears no audio through it, and excludes it once the reports
    /// add up.
    fn stall_tick(&self, room: &Arc<Room>, forwarder: u64) {
        let now = Instant::now();
        let report = {
            let mut watch = lock(&room.stall);
            match watch.deaf(now) {
                Some(spoke) if watch.reported.is_none_or(|t| now.duration_since(t) >= STALL_REPORT_EVERY) => {
                    watch.reported = Some(now);
                    // This device's own report counts with the others'.
                    watch.reports.insert(room.my_sid, (spoke, now));
                    Some(spoke)
                }
                _ => None,
            }
        };
        if let Some(spoke) = report {
            debug!(forwarder, spoke, "no audio through the volunteer forwarder; reporting it stalled");
            let me = self.inner.identity.device_id();
            let devices: Vec<DeviceId> = lock(&room.live).values().copied().filter(|d| *d != me).collect();
            for device in devices {
                let engine = self.clone();
                let msg = CallMsg::ForwarderStalled { room: room.id, forwarder, spoke };
                self.spawn_task(async move {
                    if let Err(e) = engine.call_rpc(device, msg).await {
                        debug!(device = %device, error = %e, "stall report not delivered");
                    }
                });
            }
        }
        self.judge_stall(room, forwarder);
    }

    /// Excludes volunteer `forwarder` from the call when the fresh stall reports, this device's own
    /// included, confirm it stalled. The volunteer check then fails the call over.
    fn judge_stall(&self, room: &Room, forwarder: u64) {
        let now = Instant::now();
        let reports: Vec<bool> = {
            let mut watch = lock(&room.stall);
            watch.reports.retain(|_, (_, at)| now.duration_since(*at) < STALL_REPORT_FRESH);
            watch.reports.values().map(|(spoke, _)| *spoke).collect()
        };
        if !stall_confirmed(&reports) {
            return;
        }
        let mut excluded = lock(&self.inner.calls.volunteering.excluded);
        // Only this device's current call is remembered, so a past call's exclusions don't pile up.
        excluded.retain(|id, _| *id == room.id);
        if excluded.entry(room.id).or_default().insert(forwarder) {
            tracing::warn!(forwarder, reporters = reports.len(), "volunteer forwarder stalled; excluded from this call");
        }
    }

    /// Whether this call excluded volunteer `sid` as stalled.
    fn is_excluded(&self, room: &Room, sid: u64) -> bool {
        lock(&self.inner.calls.volunteering.excluded).get(&room.id).is_some_and(|set| set.contains(&sid))
    }

    /// Records a leaf's report that this call's volunteer forwarder carries no audio to it.
    pub(super) fn on_forwarder_stalled(&self, from: DeviceId, id: RoomId, forwarder: u64, spoke: bool) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        let Some(sid) = room.live_sid(&from) else {
            return CallReply::Refused("not in this call".into());
        };
        // A report about a participant is not about a volunteer. One about a volunteer this device
        // has already failed over from still counts, so every device ends up excluding it.
        if lock(&room.live).contains_key(&forwarder) {
            return CallReply::Ack;
        }
        lock(&room.stall).reports.insert(sid, (spoke, Instant::now()));
        self.judge_stall(&room, forwarder);
        CallReply::Ack
    }

    /// For tests: makes this device's relays drop every packet while `on`, as a stalled volunteer.
    pub(crate) async fn drop_relayed_now(&self, on: bool) {
        let relays: Vec<Arc<Room>> = lock(&self.inner.calls.volunteering.relays).clone();
        for room in relays {
            if let Some(sfu) = room.forwarding.as_ref() {
                if let Some(sfu) = sfu.sfu.lock().await.as_mut() {
                    sfu.set_blackhole(on);
                }
            }
        }
    }

    /// A leaf of a volunteer forwarder: tells the volunteer this device's sender id in `room`, and
    /// waits for its answer, before it offers its link.
    pub(super) async fn tell_relay_leaf(&self, room: &Room, volunteer: DeviceId) -> Result<()> {
        let msg = CallMsg::RelayLeaf { room: room.id, gen: room.gen, sid: room.my_sid };
        match self.call_rpc(volunteer, msg).await? {
            CallReply::Ack => Ok(()),
            CallReply::Refused(why) => Err(super::call_err(why)),
            other => Err(super::call_err(format!("unexpected reply {other:?}"))),
        }
    }

    /// On a volunteer: a leaf names its sender id in generation `gen` of call `id`. A seeder learns
    /// the call's ids only this way. A member volunteer checks the id against its group. An id
    /// another device of the call already named is refused.
    pub(super) async fn on_relay_leaf(&self, from: DeviceId, id: RoomId, gen: RoomId, sid: u64) -> CallReply {
        // The tier message that builds the relay may land a moment after the leaf's first word.
        let deadline = Instant::now() + Duration::from_secs(5);
        let room = loop {
            if let Some(room) = self.relay_room_gen(&id, &gen) {
                break room;
            }
            if Instant::now() >= deadline {
                return CallReply::Refused("not in this call".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        if let Err(why) = self.check_volunteer(&room, &from) {
            debug!(device = %from, why, "relay request refused");
            return CallReply::Refused("not in this call".into());
        }
        let mut leaves = lock(&room.leaves);
        match leaves.get(&from) {
            Some(known) if *known == sid => return CallReply::Ack,
            Some(_) => return CallReply::Refused("that is not this device's sender id".into()),
            None => {}
        }
        if leaves.values().any(|known| *known == sid) || sid == room.my_sid {
            return CallReply::Refused("that sender id is taken".into());
        }
        leaves.insert(from, sid);
        CallReply::Ack
    }

    // --- On the volunteer: its relays. ---

    /// The newest relay this device runs for call `id`.
    pub(crate) fn relay_room(&self, id: &RoomId) -> Option<Arc<Room>> {
        lock(&self.inner.calls.volunteering.relays).iter().rev().find(|room| room.id == *id).cloned()
    }

    /// This device's relay for generation `gen` of call `id`.
    pub(super) fn relay_room_gen(&self, id: &RoomId, gen: &RoomId) -> Option<Arc<Room>> {
        lock(&self.inner.calls.volunteering.relays)
            .iter()
            .find(|room| room.id == *id && room.gen == *gen)
            .cloned()
    }

    fn is_relay(&self, room: &Arc<Room>) -> bool {
        lock(&self.inner.calls.volunteering.relays).iter().any(|r| Arc::ptr_eq(r, room))
    }

    /// Whether this device's relay for `room` may take a link from `device`: a current, unbanned
    /// member of the server, in its MLS group.
    pub(super) fn may_relay_for(&self, room: &Room, device: &DeviceId) -> std::result::Result<(), String> {
        self.check_volunteer(room, device)?;
        room.sid_of(device).map(|_| ()).ok_or_else(|| "not in the server's group".into())
    }

    /// A device of the call names this one its forwarder in a signed tier message. If this device
    /// offered to forward that call, it builds a relay for the generation the message describes,
    /// which the call's leaves then link to.
    pub(super) async fn on_volunteer_tier(&self, from: DeviceId, msg: TierMsg) -> CallReply {
        let refuse = |why: &str| CallReply::Refused(why.into());
        if msg.verify(&from).is_err() {
            return refuse("bad tier signature");
        }
        if lock(&self.inner.calls.current).is_some() {
            return refuse("busy in another call");
        }
        let Some(scope) = lock(&self.inner.calls.volunteering.offered).get(&msg.room).map(|o| o.scope) else {
            return refuse("not in this call");
        };
        let Scope::Channel { community, .. } = scope else {
            return refuse("not in this call");
        };
        if msg.tier != Tier::Forwarded || !self.volunteers_in(community, &self.forward_calls_set()) {
            return refuse("not in this call");
        }
        let Some(my_sid) = self.my_sid_in(community) else {
            return refuse("not in the server's group");
        };
        if msg.forwarder != Some(my_sid) {
            return refuse("not in this call");
        }
        let gen = gen_id(&msg.room, Tier::Forwarded, &msg.roster, my_sid);
        if self.relay_room_gen(&msg.room, &gen).is_some() {
            return CallReply::Ack;
        }
        let room = match self.relay_for(msg.room, scope, community, gen, my_sid) {
            Ok(room) => room,
            Err(e) => return CallReply::Refused(e.to_string()),
        };
        // The sender must be a member. A seeder learns its sender id only when it links (`RelayLeaf`).
        if let Err(why) = self.check_volunteer(&room, &from) {
            debug!(device = %from, why, "relay request refused");
            return CallReply::Refused("not in this call".into());
        }
        {
            let mut relays = lock(&self.inner.calls.volunteering.relays);
            // One call at a time, and at most its room and the room it migrates to.
            relays.retain(|r| r.id == msg.room);
            relays.push(room.clone());
            while relays.len() > MAX_RELAYS {
                relays.remove(0);
            }
        }
        debug!(call = %super::hex(&msg.room), "forwarding a call as a volunteer");
        self.spawn_task(drive_relay(Arc::downgrade(&self.inner), Arc::downgrade(&room)));
        CallReply::Ack
    }

    /// A relay for generation `gen` of call `id`, which this device is not in. Its mesh never links
    /// and holds a random key, so this device has no media key for the call. Its forwarder is a
    /// `Sfu`, which takes none.
    fn relay_for(&self, id: RoomId, scope: Scope, community: CommunityId, gen: RoomId, my_sid: u64) -> Result<Arc<Room>> {
        let unused = MediaKey::new(DeviceKey::generate().0.seed());
        let mesh = Mesh::new(unused, my_sid).map_err(super::call_err)?;
        let mut room = Room::new(id, scope, mesh, my_sid, 0, None);
        room.gen = gen;
        room.tier = Tier::Forwarded;
        room.forwarding = Some(Forwarding {
            role: Role::Root,
            forwarder: my_sid,
            children: Vec::new(),
            standby: None,
            served: 0,
            sfu: AsyncMutex::new(Some(Sfu::new(Mode::AudioTop3))),
            leaf: AsyncMutex::new(None),
            volunteer: None,
            serving: true,
        });
        // A seeder has no group to read: its leaves tell it their ids (`RelayLeaf`).
        if let Ok(indexed) = self.indexed_members(community) {
            lock(&room.leaves).extend(indexed.iter().map(|(sid, device, _)| (*device, *sid)));
        }
        Ok(Arc::new(room))
    }

    /// A leaf left the call: this device's relays drop its downlink.
    pub(super) async fn drop_relay_leaf(&self, id: &RoomId, device: &DeviceId) {
        let relays: Vec<Arc<Room>> = lock(&self.inner.calls.volunteering.relays)
            .iter()
            .filter(|room| room.id == *id)
            .cloned()
            .collect();
        for room in relays {
            if let Some(sid) = room.sid_of(device) {
                self.drop_downlink(&room, sid).await;
            }
        }
    }

    async fn drop_downlink(&self, room: &Room, sid: u64) {
        if let Some(forwarding) = room.forwarding.as_ref() {
            if let Some(sfu) = forwarding.sfu.lock().await.as_mut() {
                sfu.remove_downlink(sid);
            }
        }
        lock(&room.live).remove(&sid);
        lock(&room.peers).remove(&sid);
    }

    /// The relay's membership tick: follows the server's group, drops leaves that were banned or
    /// left the server, and asks the senders for the layers its viewers need.
    async fn relay_membership(&self, room: &Arc<Room>) {
        let Scope::Channel { community, .. } = room.scope else {
            return;
        };
        if !self.seeder_may_forward(community) {
            if let Ok(indexed) = self.indexed_members(community) {
                *lock(&room.leaves) = indexed.iter().map(|(sid, device, _)| (*device, *sid)).collect();
            }
        }
        let linked: Vec<(u64, DeviceId)> = lock(&room.live).iter().map(|(sid, device)| (*sid, *device)).collect();
        for (sid, device) in linked {
            if self.may_relay_for(room, &device).is_err() {
                self.drop_downlink(room, sid).await;
            }
        }
        self.spawn_signal_layers(room);
    }

    /// For tests: how many leaves this device's relays carry, summed over them.
    pub(crate) async fn relay_downlinks_now(&self) -> usize {
        let relays: Vec<Arc<Room>> = lock(&self.inner.calls.volunteering.relays).clone();
        let mut total = 0;
        for room in relays {
            if let Some(forwarding) = room.forwarding.as_ref() {
                total += forwarding.sfu.lock().await.as_ref().map_or(0, |sfu| sfu.downlinks().len());
            }
        }
        total
    }

    /// For tests: how many live volunteer offers this device's call holds.
    pub(crate) fn forward_offers_now(&self) -> usize {
        let Some(room) = lock(&self.inner.calls.current).clone() else {
            return 0;
        };
        let now = Instant::now();
        let offers = lock(&room.offers).values().filter(|offer| offer.until > now).count();
        offers
    }

    /// For tests: sends this device's offer to forward the call in `channel` to `device`, as a
    /// volunteer would, and returns the reply: "ack", or the reason it was refused.
    pub(crate) async fn offer_to_forward_now(
        &self,
        community: CommunityId,
        channel: dissent_events::ChannelId,
        device: DeviceId,
    ) -> String {
        let id = super::room_id(&[&community.0[..], &channel.0[..]]);
        let offer = ForwardOffer::sign(&self.inner.identity.device, id, self.offered_kbps(), FEATURES, self.offer_ttl_ms());
        match self.call_rpc(device, CallMsg::ForwardOffer(Box::new(offer))).await {
            Ok(CallReply::Ack) => "ack".into(),
            Ok(CallReply::Refused(why)) => why,
            Ok(other) => format!("{other:?}"),
            Err(e) => e.to_string(),
        }
    }
}

/// Runs one of this device's relays: forwards every 20 ms, and every half second follows the
/// server's membership. Stops when the relay is dropped, or once no leaf has been linked to it for
/// `RELAY_IDLE`.
async fn drive_relay(weak: Weak<Inner>, room: Weak<Room>) {
    let mut tick = tokio::time::interval(Duration::from_millis(FRAME_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut since_check: u32 = 0;
    let mut idle_since: Option<Instant> = None;
    loop {
        tick.tick().await;
        let (Some(inner), Some(room)) = (weak.upgrade(), room.upgrade()) else {
            break;
        };
        let engine = Engine { inner };
        if !engine.is_relay(&room) {
            break;
        }
        let Some(forwarding) = room.forwarding.as_ref() else {
            break;
        };
        pump_forwarded(&engine, &room, forwarding).await;
        since_check += 1;
        if since_check < MEMBERSHIP_EVERY {
            continue;
        }
        since_check = 0;
        engine.relay_membership(&room).await;
        let linked = forwarding.sfu.lock().await.as_ref().is_some_and(|sfu| !sfu.downlinks().is_empty());
        if linked {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= RELAY_IDLE {
            lock(&engine.inner.calls.volunteering.relays).retain(|r| !Arc::ptr_eq(r, &room));
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volunteer(sid: u64, kbps: u32, relay_host: bool) -> Volunteer {
        let key = DeviceKey::generate();
        Volunteer {
            sid,
            offer: VolunteerOffer {
                device: key.id(),
                root: RootId([sid as u8; 32]),
                kbps,
                wired: true,
                on_battery: false,
                storage: false,
                until: Instant::now() + Duration::from_secs(60),
            },
            relay_host,
        }
    }

    #[test]
    fn a_stall_needs_two_leaves_and_a_speaker_among_them() {
        // One speaker and one listener both heard nothing: the forwarder dropped the speaker's audio.
        assert!(stall_confirmed(&[true, false]));
        // A quiet call: everyone hears nothing because nobody speaks.
        assert!(!stall_confirmed(&[false; 6]));
        // A lone speaker who hears nobody, in a call where the others are silent and hear it.
        assert!(!stall_confirmed(&[true]));
    }

    #[test]
    fn a_leaf_is_deaf_only_after_the_window_and_says_whether_it_spoke() {
        let mut watch = StallWatch::new();
        let now = Instant::now();
        assert_eq!(watch.deaf(now), None, "a new room is not deaf yet");
        let later = now + STALL_WINDOW + Duration::from_millis(1);
        assert_eq!(watch.deaf(later), Some(false));
        watch.note_spoke(later - Duration::from_millis(200));
        assert_eq!(watch.deaf(later), Some(false), "a leaf that just started speaking is no witness");
        let mut watch = StallWatch::new();
        let mut t = now;
        while t < later {
            watch.note_spoke(t);
            t += Duration::from_millis(20);
        }
        assert_eq!(watch.deaf(later), Some(true), "speaking for the whole window");
        assert_eq!(watch.deaf(later + SPEECH_GAP + Duration::from_millis(100)), Some(false), "a leaf that stopped speaking");
        watch.heard = Some(later - Duration::from_secs(1));
        assert_eq!(watch.deaf(later), None, "audio heard within the window");
    }

    #[test]
    fn a_seeder_hears_of_a_call_only_when_it_may_forward_one() {
        assert!(!seeders_hear_call(false, MIN_PEOPLE + 4), "never while the owner has not allowed it");
        assert!(!seeders_hear_call(true, MIN_PEOPLE - 1), "not for a call too small to need a forwarder");
        assert!(seeders_hear_call(true, MIN_PEOPLE));
        assert!(seeders_hear_call(true, 0), "the empty roster that ends a call reaches it");
    }

    #[test]
    fn a_seeder_gets_who_is_in_a_call_but_not_their_mute_or_deafen() {
        let root = dissent_identity::RootKey::generate().id();
        let roster = [super::super::PresenceMember { root, muted: true, deafened: true }];
        assert_eq!(for_seeder(&roster), vec![super::super::PresenceMember { root, muted: false, deafened: false }]);
    }

    #[test]
    fn without_volunteers_the_election_is_unchanged() {
        let sids = [9, 4, 7, 12];
        assert_eq!(
            elect_with_volunteers(&sids, &[], sids.len()),
            elect_forwarder(&sids, sids.len(), &BTreeMap::new())
        );
        assert_eq!(elect_with_volunteers(&sids, &[], sids.len()), Some(4));
    }

    #[test]
    fn a_volunteer_at_the_default_capacity_wins_on_its_bonus() {
        let sids = [1, 2, 3, 4, 5, 6, 7];
        assert_eq!(elect_with_volunteers(&sids, &[volunteer(20, DEFAULT_KBPS, false)], 7), Some(20));
    }

    #[test]
    fn the_bonus_is_capped_so_a_weak_volunteer_loses() {
        assert_eq!(volunteer_bonus(5_000), 500);
        assert_eq!(volunteer_bonus(80_000), BONUS_CAP_KBPS);
        // 8 Mbps plus its 800 kbps bonus is still short of a participant's 10.
        let sids = [3, 5];
        assert_eq!(elect_with_volunteers(&sids, &[volunteer(1, 8_000, false)], 2), Some(3));
        // A volunteer with too little upload for the call is not eligible at all.
        assert_eq!(elect_with_volunteers(&[], &[volunteer(1, 500, false)], 20), None);
    }

    #[test]
    fn a_hosted_relay_breaks_a_tie_between_volunteers_only() {
        let sids = [1, 2];
        let plain = volunteer(10, DEFAULT_KBPS, false);
        let host = volunteer(11, DEFAULT_KBPS, true);
        assert_eq!(elect_with_volunteers(&sids, &[plain.clone(), host.clone()], 2), Some(11));
        // It never outweighs capacity: a stronger volunteer beats a host.
        let strong = volunteer(12, 20_000, false);
        assert_eq!(elect_with_volunteers(&sids, &[host, strong], 2), Some(12));
        // Without the tie-break, the lower id wins between equal volunteers.
        assert_eq!(elect_with_volunteers(&sids, &[plain, volunteer(13, DEFAULT_KBPS, false)], 2), Some(10));
    }

    /// `CallMsg` as the last release before volunteers has it: every variant up to `WantedLayers`.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    enum PreVolunteerCallMsg {
        Join(Box<super::super::JoinMsg>),
        Offer { room: RoomId, gen: RoomId, signal: Vec<u8> },
        Mute { room: RoomId, muted: bool },
        Decline { room: RoomId },
        Tier(Box<TierMsg>),
        Capacity { room: RoomId, kbps: u32, sig: Signature },
        AnsweredElsewhere { room: RoomId },
        Leave { room: RoomId },
        Presence(Box<super::super::PresenceMsg>),
        Features { room: RoomId, bits: u64 },
        VideoLayers { room: RoomId, screen: bool, layers: Vec<(u32, u32)> },
        TileWidths { room: RoomId, tiles: Vec<(u64, bool, u32)> },
        WantedLayers { room: RoomId, screen: bool, mask: u8 },
    }

    #[test]
    fn an_older_device_cannot_decode_an_offer_but_still_decodes_the_rest() {
        let offer = ForwardOffer::sign(&DeviceKey::generate(), [1; 32], DEFAULT_KBPS, FEATURES, 60_000);
        let bytes = postcard::to_stdvec(&CallMsg::ForwardOffer(Box::new(offer))).unwrap();
        assert!(postcard::from_bytes::<PreVolunteerCallMsg>(&bytes).is_err());
        let wanted = postcard::to_stdvec(&CallMsg::WantedLayers { room: [1; 32], screen: false, mask: 3 }).unwrap();
        assert!(matches!(
            postcard::from_bytes::<PreVolunteerCallMsg>(&wanted),
            Ok(PreVolunteerCallMsg::WantedLayers { mask: 3, .. })
        ));
    }

    #[test]
    fn an_offer_verifies_only_for_its_sender_and_unchanged() {
        let key = DeviceKey::generate();
        let offer = ForwardOffer::sign(&key, [3; 32], 9_000, FEATURES, 60_000);
        assert!(offer.verify(&key.id()).is_ok());
        assert!(offer.verify(&DeviceKey::generate().id()).is_err(), "another device cannot sign for it");
        let mut stretched = offer.clone();
        stretched.ttl_ms = u32::MAX;
        assert!(stretched.verify(&key.id()).is_err(), "a changed lifetime fails the signature");
        let mut other_room = offer;
        other_room.room = [4; 32];
        assert!(other_room.verify(&key.id()).is_err(), "an offer is bound to its call");
    }
}
