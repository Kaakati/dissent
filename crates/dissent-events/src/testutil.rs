//! Shared helpers for unit tests.

use crate::{ChannelId, Event, EventBody, EventId, Hlc, Payload};
use dissent_identity::{CommunityId, DeviceCert, DeviceKey, RootId, RootKey};

pub struct User {
    pub root: RootKey,
    pub device: DeviceKey,
    pub cert: DeviceCert,
}

impl User {
    pub fn new() -> Self {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        Self { root, device, cert }
    }

    pub fn id(&self) -> RootId {
        self.root.id()
    }
}

/// The smallest HLC that is after every parent and not before `wall_ms`.
/// Use it for generated events so they respect the causal-HLC rule.
pub fn after(parents: &[Hlc], wall_ms: u64) -> Hlc {
    let floor = parents.iter().copied().max().unwrap_or_default();
    if wall_ms > floor.wall_ms {
        Hlc {
            wall_ms,
            counter: 0,
        }
    } else {
        Hlc {
            wall_ms: floor.wall_ms,
            counter: floor.counter + 1,
        }
    }
}

/// Community-agnostic event on the control channel.
pub fn event(u: &User, parents: Vec<EventId>, hlc: Hlc, payload: Payload) -> Event {
    on_channel(u, ChannelId::CONTROL, parents, hlc, payload)
}

pub fn on_channel(
    u: &User,
    channel: ChannelId,
    parents: Vec<EventId>,
    hlc: Hlc,
    payload: Payload,
) -> Event {
    let body = EventBody {
        community: CommunityId([0; 32]),
        channel,
        author: u.id(),
        device: u.device.id(),
        parents,
        hlc,
        payload,
    };
    Event::sign(body, &u.device)
}
