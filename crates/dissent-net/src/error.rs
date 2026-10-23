use std::fmt::Display;

/// Errors from the networking layer.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("could not bind endpoint: {0}")]
    Bind(#[from] iroh::endpoint::BindError),
    #[error("could not connect: {0}")]
    Connect(#[from] iroh::endpoint::ConnectError),
    #[error("connection failed: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("invalid peer: {0}")]
    BadPeer(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("gossip: {0}")]
    Gossip(#[from] iroh_gossip::api::ApiError),
    #[error("shutdown failed: {0}")]
    Shutdown(String),
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    FrameTooLarge { len: usize, max: usize },
    #[error("request timed out")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, NetError>;

pub(crate) fn stream(e: impl Display) -> NetError {
    NetError::Stream(e.to_string())
}
