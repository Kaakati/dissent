//! Content-blind wake-up relay for mobile push.
//!
//! A phone that sleeps cannot receive messages. A peer that wants to wake it asks this relay to
//! send an empty push to the phone's push endpoint. The relay sees the endpoint, a random wake
//! token, and the source IP of each request. It does not see senders, recipients or content,
//! because a wake carries nothing and no request names the sender.

pub mod apns;
pub mod limit;
pub mod net;
pub mod push;
pub mod server;
pub mod store;
pub mod token;

use std::path::PathBuf;
use std::sync::Arc;

pub use apns::ApnsSettings;
pub use server::{AppState, app};

/// Relay settings that are not about APNs.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Allow plain HTTP and private addresses. For tests and local runs only.
    pub dev: bool,
    /// UnifiedPush hosts that may be registered. Empty means any public host.
    pub ups_allow_hosts: Vec<String>,
    /// JSON file that holds the registrations between restarts.
    pub snapshot: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("push provider answered {0}")]
    Upstream(u16),
    #[error("push request failed: {0}")]
    Transport(String),
    #[error("APNs is not enabled on this relay")]
    ApnsDisabled,
    #[error("APNs key: {0}")]
    Key(String),
    #[error("random number generator failed")]
    Random,
    #[error("storage: {0}")]
    Storage(String),
}

/// Builds the shared state from settings. Opens the snapshot, if one is configured.
pub fn build_state(cfg: Config, apns: Option<ApnsSettings>) -> Result<Arc<AppState>, PushError> {
    let push = push::Push::new(apns)?;
    Ok(Arc::new(AppState::new(cfg, push)?))
}
