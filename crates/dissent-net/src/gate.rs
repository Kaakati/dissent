//! The one gate every incoming connection passes, whatever its protocol. After the handshake the remote device is
//! known, and the app's [`ConnectionGate`] says what it is to us: someone we share something with, a stranger, or a
//! device that was revoked or banned. Refused devices are closed at once. Strangers share one bounded pool, so a flood
//! of fresh device ids, which cost nothing to make, cannot crowd out members, contacts or seeders, and every device has
//! a cap on its open connections. Dials this node makes are never judged here.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use dissent_identity::DeviceId;
use iroh::endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, Side};

use crate::inbound::BUSY;

/// Close code for a connection from a revoked or banned device.
pub(crate) const REFUSED: u32 = 5;

/// What a remote device is to this node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// We share a server, a conversation or a contact with its user, or it is one of our own devices.
    Known,
    /// Anything else, including a device that joins and whose certificate has not reached us yet.
    Stranger,
    /// Revoked, or its user is banned and shares nothing else with us. Only explicit revocations and bans land here.
    Refuse,
}

/// Says what a device is. Called on every incoming connection, so it must answer at once, without waiting or taking
/// locks that other work holds for long.
pub trait ConnectionGate: Send + Sync + 'static {
    fn verdict(&self, device: DeviceId) -> Gate;
}

/// How many connections the gate lets in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateLimits {
    /// Connections open at once for one device, over every protocol. 32: gossip, requests, file transfers and call
    /// links each hold a few.
    pub per_device: usize,
    /// Connections open at once from every stranger together. 256: joins, invite previews and address lookups are
    /// short, so honest strangers rarely hold many at once.
    pub strangers: usize,
}

impl Default for GateLimits {
    fn default() -> Self {
        Self { per_device: 32, strangers: 256 }
    }
}

/// The endpoint hook that applies a [`ConnectionGate`].
pub(crate) struct Hooks {
    gate: Arc<dyn ConnectionGate>,
    limits: GateLimits,
    counts: Arc<Mutex<Counts>>,
}

#[derive(Debug, Default)]
struct Counts {
    per_device: HashMap<DeviceId, usize>,
    strangers: usize,
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks").field("limits", &self.limits).finish_non_exhaustive()
    }
}

impl Hooks {
    pub(crate) fn new(gate: Arc<dyn ConnectionGate>, limits: GateLimits) -> Self {
        Self { gate, limits, counts: Arc::default() }
    }

    /// Admits a connection from `device` judged `verdict`, counting it, or says why not.
    fn admit(&self, device: DeviceId, verdict: Gate) -> Result<(), (u32, &'static [u8])> {
        let stranger = match verdict {
            Gate::Refuse => return Err((REFUSED, b"refused")),
            Gate::Stranger => true,
            Gate::Known => false,
        };
        let mut counts = lock(&self.counts);
        if counts.per_device.get(&device).copied().unwrap_or(0) >= self.limits.per_device {
            return Err((BUSY, b"busy"));
        }
        if stranger && counts.strangers >= self.limits.strangers {
            return Err((BUSY, b"busy"));
        }
        *counts.per_device.entry(device).or_default() += 1;
        if stranger {
            counts.strangers += 1;
        }
        Ok(())
    }
}

/// Gives back what [`Hooks::admit`] counted for one connection.
fn release(counts: &Mutex<Counts>, device: DeviceId, stranger: bool) {
    let mut counts = lock(counts);
    if let Some(n) = counts.per_device.get_mut(&device) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            counts.per_device.remove(&device);
        }
    }
    if stranger {
        counts.strangers = counts.strangers.saturating_sub(1);
    }
}

impl EndpointHooks for Hooks {
    async fn after_handshake<'a>(&'a self, conn: &'a Connection) -> AfterHandshakeOutcome {
        if conn.side() != Side::Server {
            return AfterHandshakeOutcome::accept();
        }
        let device = DeviceId(*conn.remote_id().as_bytes());
        let verdict = self.gate.verdict(device);
        if let Err((code, reason)) = self.admit(device, verdict) {
            return AfterHandshakeOutcome::Reject { error_code: code.into(), reason: reason.to_vec() };
        }
        // The count is given back when the connection closes. The weak handle does not keep it open.
        let closed = conn.weak_handle().closed();
        let counts = self.counts.clone();
        tokio::spawn(async move {
            closed.await;
            release(&counts, device, verdict == Gate::Stranger);
        });
        AfterHandshakeOutcome::accept()
    }
}

fn lock(counts: &Mutex<Counts>) -> MutexGuard<'_, Counts> {
    counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Gate);

    impl ConnectionGate for Fixed {
        fn verdict(&self, _device: DeviceId) -> Gate {
            self.0
        }
    }

    fn hooks(limits: GateLimits) -> Hooks {
        Hooks::new(Arc::new(Fixed(Gate::Known)), limits)
    }

    #[test]
    fn a_refused_device_is_closed_with_its_own_code() {
        let h = hooks(GateLimits::default());
        assert_eq!(h.admit(DeviceId([1; 32]), Gate::Refuse).unwrap_err().0, REFUSED);
    }

    #[test]
    fn strangers_share_one_pool_and_known_devices_do_not_take_from_it() {
        let h = hooks(GateLimits { per_device: 8, strangers: 2 });
        assert!(h.admit(DeviceId([1; 32]), Gate::Stranger).is_ok());
        assert!(h.admit(DeviceId([2; 32]), Gate::Stranger).is_ok());
        assert_eq!(h.admit(DeviceId([3; 32]), Gate::Stranger).unwrap_err().0, BUSY);
        assert!(h.admit(DeviceId([4; 32]), Gate::Known).is_ok(), "a member gets in while the pool is full");
        release(&h.counts, DeviceId([1; 32]), true);
        assert!(h.admit(DeviceId([3; 32]), Gate::Stranger).is_ok(), "a closed stranger frees its place");
    }

    #[test]
    fn one_device_gets_its_cap_and_no_more() {
        let h = hooks(GateLimits { per_device: 2, strangers: 256 });
        let d = DeviceId([9; 32]);
        assert!(h.admit(d, Gate::Known).is_ok());
        assert!(h.admit(d, Gate::Known).is_ok());
        assert_eq!(h.admit(d, Gate::Known).unwrap_err().0, BUSY);
        release(&h.counts, d, false);
        assert!(h.admit(d, Gate::Known).is_ok());
    }
}
