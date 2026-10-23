//! Errors returned by the engine. Each wrapped crate keeps its own error type.

/// Everything the engine can fail with.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The community's replay refused the action, for example `"Banned"` or `"NoPermission"`.
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Identity(#[from] dissent_identity::IdentityError),
    #[error(transparent)]
    Events(#[from] dissent_events::Error),
    #[error(transparent)]
    Net(#[from] dissent_net::NetError),
    #[error(transparent)]
    Discovery(#[from] dissent_discovery::DiscoveryError),
    #[error("storage: {0}")]
    Storage(#[from] std::io::Error),
    #[error("encoding: {0}")]
    Encoding(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
