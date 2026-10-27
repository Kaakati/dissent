//! A relay for one person's own servers. It admits only the device keys of members of those servers,
//! caps how many connections are open at once, and caps how fast each client can send to the relay.
//!
//! The allow-list comes from the community state, so it follows joins, leaves and bans. iroh-relay
//! checks access once, when a connection opens, so a ban also closes the banned device's open
//! connections. It does that with iroh-relay's `Clients::disconnect`, one key at a time, and the server
//! keeps running for everyone else.
//!
//! The bandwidth cap is per client. iroh-relay limits how fast it reads from each client, and the
//! relay forwards what it reads, so the total the relay sends is bounded by the per-client rate times
//! the connection cap. [`Caps::per_client`] divides the host's total across that cap.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRateLimit, ClientRequest, ConnectionId, Limits, Server,
    ServerConfig,
};

/// A device's endpoint key: the 32 bytes of its public key, which is also its device id.
pub type NodeKey = [u8; 32];

/// Total bytes per second the relay reads from its clients, unless the host says otherwise: 2 MiB/s,
/// about 16 Mbit/s, roughly a home connection's upload.
pub const DEFAULT_TOTAL_BYTES_PER_SECOND: u32 = 2 * 1024 * 1024;
/// Connections open at once, unless the host says otherwise.
pub const DEFAULT_MAX_CONNECTIONS: usize = 16;

/// The reason a device is refused. Shown to the client, so it says what to do.
pub const NOT_A_MEMBER: &str = "this relay carries traffic only for members of its host's servers";
/// The reason a member is refused when the relay is full.
pub const FULL: &str = "this relay is full; try again later";

/// The limits a host sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Most bytes per second the relay reads from its clients together.
    pub total_bytes_per_second: u32,
    /// Most connections the relay admits at once.
    pub max_connections: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            total_bytes_per_second: DEFAULT_TOTAL_BYTES_PER_SECOND,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

impl Caps {
    /// Checks that both limits can work: a rate above zero, and at least one connection.
    pub fn validate(&self) -> Result<(), String> {
        if self.total_bytes_per_second == 0 {
            return Err("the bandwidth cap must be above zero".into());
        }
        if self.max_connections == 0 {
            return Err("the connection cap must be at least one".into());
        }
        Ok(())
    }

    /// The read rate each client gets: the total divided over the connection cap, never below one
    /// byte per second.
    pub fn per_client(&self) -> NonZeroU32 {
        let share = self.total_bytes_per_second / self.max_connections.max(1) as u32;
        NonZeroU32::new(share).unwrap_or(NonZeroU32::MIN)
    }
}

/// The access hook: admits a connection when its key is on the list and the relay has room.
#[derive(Debug)]
pub struct HostedAccess {
    max_connections: usize,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    allowed: HashSet<NodeKey>,
    /// Admitted connections, and the key each one authenticated.
    live: HashMap<ConnectionId, NodeKey>,
}

impl HostedAccess {
    /// A hook that admits the keys in `allowed`, up to `caps.max_connections` at once.
    pub fn new(caps: &Caps, allowed: impl IntoIterator<Item = NodeKey>) -> Arc<Self> {
        Arc::new(HostedAccess {
            max_connections: caps.max_connections,
            state: Mutex::new(State {
                allowed: allowed.into_iter().collect(),
                live: HashMap::new(),
            }),
        })
    }

    /// Replaces the allow-list with the keys of the current members. Call it on every join, leave or
    /// ban. Returns the distinct keys that had an open connection and are no longer on the list. Their
    /// connections are forgotten here; the caller closes them on the server.
    pub fn set_members(&self, members: impl IntoIterator<Item = NodeKey>) -> Vec<NodeKey> {
        let allowed: HashSet<NodeKey> = members.into_iter().collect();
        let mut state = lock(&self.state);
        let mut stranded: Vec<NodeKey> = state
            .live
            .values()
            .filter(|key| !allowed.contains(*key))
            .copied()
            .collect();
        stranded.sort();
        stranded.dedup();
        state.live.retain(|_, key| allowed.contains(key));
        state.allowed = allowed;
        stranded
    }

    /// Forgets every open connection. Used when the server holding them has been restarted.
    pub fn forget_connections(&self) {
        lock(&self.state).live.clear();
    }

    /// Whether `key` is on the list right now.
    pub fn is_allowed(&self, key: &NodeKey) -> bool {
        lock(&self.state).allowed.contains(key)
    }

    /// Connections open right now.
    pub fn live(&self) -> usize {
        lock(&self.state).live.len()
    }
}

/// Puts the hook and the per-client read limit into a server config.
pub fn apply(caps: &Caps, access: Arc<HostedAccess>, config: &mut ServerConfig) {
    let mut limits = Limits::default();
    limits.client_rx = Some(ClientRateLimit::new(caps.per_client()));
    if let Some(relay) = config.relay.as_mut() {
        relay.limits = limits;
        relay.access = access;
    }
}

impl AccessControl for HostedAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let key: NodeKey = *request.endpoint_id().as_bytes();
        let mut state = lock(&self.state);
        if !state.allowed.contains(&key) {
            return Access::Deny {
                reason: Some(NOT_A_MEMBER.into()),
            };
        }
        if state.live.len() >= self.max_connections {
            return Access::Deny {
                reason: Some(FULL.into()),
            };
        }
        state.live.insert(request.connection_id(), key);
        Access::Allow
    }

    fn on_disconnect(&self, _endpoint_id: EndpointId, connection_id: ConnectionId) {
        lock(&self.state).live.remove(&connection_id);
    }
}

/// Builds the server config for a relay, given the access hook to put in it. It is async, since TLS
/// setup is, and it runs again on each restart. The bind address must be fixed, so a restart comes
/// back where clients expect it.
pub type ConfigBuilder = Box<
    dyn Fn(Arc<HostedAccess>) -> Pin<Box<dyn Future<Output = Result<ServerConfig, String>> + Send>>
        + Send
        + Sync,
>;

/// A running relay for one host's servers. It owns the server and its hook. A device that is removed from
/// the list has its open connections closed on the server, so a ban takes effect at once and the server
/// keeps running for everyone else.
pub struct RelayHost {
    access: Arc<HostedAccess>,
    server: Option<Server>,
}

impl RelayHost {
    /// Starts the relay with the members' keys on its list.
    pub async fn start(build: ConfigBuilder, access: Arc<HostedAccess>) -> Result<Self, String> {
        let config = build(access.clone()).await?;
        let server = Server::spawn(config)
            .await
            .map_err(|e| format!("could not start the relay: {e}"))?;
        Ok(RelayHost {
            access,
            server: Some(server),
        })
    }

    /// The hook the server checks connections against.
    pub fn access(&self) -> &HostedAccess {
        &self.access
    }

    /// The address the relay listens on, if it is running: HTTPS when TLS is on, else plain HTTP.
    pub fn addr(&self) -> Option<std::net::SocketAddr> {
        self.server
            .as_ref()
            .and_then(|s| s.https_addr().or_else(|| s.http_addr()))
    }

    /// Replaces the allow-list with the current members, and closes the open connections of any device
    /// that was removed. Returns how many devices were closed. The server is not restarted.
    pub fn set_members(&mut self, members: impl IntoIterator<Item = NodeKey>) -> usize {
        let stranded = self.access.set_members(members);
        if let Some(service) = self.server.as_ref().and_then(|s| s.relay_service()) {
            for key in &stranded {
                if let Ok(id) = EndpointId::from_bytes(key) {
                    service.clients().disconnect(id, None);
                }
            }
        }
        stranded.len()
    }

    /// Stops the relay and closes its connections.
    pub async fn stop(mut self) {
        if let Some(server) = self.server.take() {
            let _ = server.shutdown().await;
        }
        self.access.forget_connections();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::DeviceKey;
    use http::request::Parts;
    use iroh_relay::http::ProtocolVersion;

    fn key() -> NodeKey {
        DeviceKey::generate().id().0
    }

    fn request(key: NodeKey) -> ClientRequest {
        let parts: Parts = http::Request::builder()
            .uri("/relay")
            .body(())
            .expect("request")
            .into_parts()
            .0;
        ClientRequest::new(
            EndpointId::from_bytes(&key).expect("key"),
            ProtocolVersion::V1,
            parts,
        )
    }

    #[tokio::test]
    async fn admits_a_member_and_refuses_a_stranger() {
        let member = key();
        let stranger = key();
        let access = HostedAccess::new(&Caps::default(), [member]);
        assert!(matches!(access.on_connect(&request(member)).await, Access::Allow));
        match access.on_connect(&request(stranger)).await {
            Access::Deny { reason } => assert_eq!(reason.as_deref(), Some(NOT_A_MEMBER)),
            Access::Allow => panic!("a stranger was admitted"),
        }
        assert_eq!(access.live(), 1);
    }

    #[tokio::test]
    async fn the_connection_cap_refuses_past_the_limit_and_frees_on_disconnect() {
        let caps = Caps {
            total_bytes_per_second: 1024,
            max_connections: 2,
        };
        let members: Vec<NodeKey> = (0..3).map(|_| key()).collect();
        let access = HostedAccess::new(&caps, members.clone());
        let first = request(members[0]);
        let second = request(members[1]);
        assert!(matches!(access.on_connect(&first).await, Access::Allow));
        assert!(matches!(access.on_connect(&second).await, Access::Allow));
        match access.on_connect(&request(members[2])).await {
            Access::Deny { reason } => assert_eq!(reason.as_deref(), Some(FULL)),
            Access::Allow => panic!("a connection past the cap was admitted"),
        }
        access.on_disconnect(EndpointId::from_bytes(&members[0]).expect("key"), first.connection_id());
        assert!(matches!(access.on_connect(&request(members[2])).await, Access::Allow));
    }

    #[tokio::test]
    async fn a_ban_removes_access_for_new_connections() {
        let kept = key();
        let banned = key();
        let access = HostedAccess::new(&Caps::default(), [kept, banned]);
        assert!(matches!(access.on_connect(&request(banned)).await, Access::Allow));
        // The ban: the list is replaced without the banned device. Its open connection is reported.
        assert_eq!(access.set_members([kept]).len(), 1);
        assert!(!access.is_allowed(&banned));
        assert!(matches!(access.on_connect(&request(banned)).await, Access::Deny { .. }));
        assert!(matches!(access.on_connect(&request(kept)).await, Access::Allow));
    }

    #[test]
    fn the_bandwidth_cap_is_shared_over_the_connection_cap() {
        let caps = Caps::default();
        assert_eq!(caps.per_client().get(), 2 * 1024 * 1024 / 16);
        let tiny = Caps {
            total_bytes_per_second: 1,
            max_connections: 16,
        };
        assert_eq!(tiny.per_client().get(), 1, "a share never drops to zero");
    }

    #[test]
    fn apply_sets_the_per_client_read_limit() {
        let caps = Caps::default();
        let access = HostedAccess::new(&caps, []);
        let mut config = ServerConfig::default();
        config.relay = Some(iroh_relay::server::RelayConfig::new(([127, 0, 0, 1], 0)));
        apply(&caps, access, &mut config);
        let relay = config.relay.as_ref().expect("relay");
        assert_eq!(
            relay.limits.client_rx.map(|l| l.bytes_per_second),
            Some(caps.per_client())
        );
    }

    #[tokio::test]
    async fn a_ban_closes_the_banned_device_and_the_server_keeps_running() {
        let kept = key();
        let banned = key();
        let caps = Caps::default();
        let access = HostedAccess::new(&caps, [kept, banned]);
        let build: ConfigBuilder = Box::new(move |access| {
            Box::pin(async move {
                let mut config = crate::dev_config(([127, 0, 0, 1], 0).into());
                apply(&caps, access, &mut config);
                Ok(config)
            })
        });
        let mut host = RelayHost::start(build, access.clone()).await.expect("start");
        let addr = host.addr().expect("the relay is running");
        // An open connection by the device that is about to be banned.
        assert!(matches!(access.on_connect(&request(banned)).await, Access::Allow));
        assert_eq!(access.live(), 1);
        // The ban closes that connection, and nothing restarts.
        assert_eq!(host.set_members([kept]), 1, "one device was closed");
        assert_eq!(access.live(), 0, "its connection is forgotten");
        assert_eq!(host.addr(), Some(addr), "the server is the same one, not restarted");
        assert!(matches!(access.on_connect(&request(banned)).await, Access::Deny { .. }));
        // With nothing left to close, a change closes nothing.
        assert_eq!(host.set_members([kept]), 0);
        host.stop().await;
    }

    #[test]
    fn caps_refuse_zero() {
        assert!(Caps::default().validate().is_ok());
        assert!(Caps { total_bytes_per_second: 0, ..Caps::default() }.validate().is_err());
        assert!(Caps { max_connections: 0, ..Caps::default() }.validate().is_err());
    }

}
