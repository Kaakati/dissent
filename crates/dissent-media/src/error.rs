//! Errors of the media spike.

use thiserror::Error;

/// Everything that can go wrong in a [`crate::Call`] or in the SFrame layer.
#[derive(Debug, Error)]
pub enum MediaError {
    /// The UDP socket failed.
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
    /// str0m rejected an operation.
    #[error("WebRTC error: {0}")]
    Rtc(#[from] str0m::RtcError),
    /// SFrame encryption or key derivation failed.
    #[error("SFrame error: {0}")]
    Sframe(#[from] sframe::error::SframeError),
    /// An ICE candidate string did not parse.
    #[error("ICE candidate error: {0}")]
    Ice(String),
    /// A session description did not parse.
    #[error("bad session description: {0}")]
    BadSignal(String),
    /// A signal arrived that the call cannot apply in its current state.
    #[error("signal does not fit the call state: {0}")]
    BadState(&'static str),
    /// The sender id is larger than the SFrame key id can carry.
    #[error("sender id {0} does not fit the SFrame key id")]
    SenderIdOutOfRange(u64),
    /// Media was written before ICE and DTLS completed.
    #[error("media is not connected yet")]
    NotConnected,
    /// The audio track is not negotiated yet, or the codec is missing.
    #[error("no audio track is negotiated yet")]
    NoAudioTrack,
    /// The underlying str0m instance has shut down.
    #[error("call is closed")]
    Closed,
    /// A frame names a key epoch that is neither current nor still in its grace period.
    #[error("frame is from a key epoch that is no longer accepted")]
    EpochNotAccepted,
    /// A frame was already received, or is older than the replay window.
    #[error("frame is a replay or older than the replay window")]
    Replayed,
    /// A wire frame is too short to carry its audio-level byte and SFrame header.
    #[error("frame is too short to be an SFrame frame")]
    MalformedFrame,
    /// The participant has no forwarder left to send through.
    #[error("no forwarder is available to carry the call")]
    NoSfu,
    /// The iroh media link refused a datagram.
    #[error("media link error: {0}")]
    Link(#[from] dissent_net::NetError),
    /// A signal could not be encoded or decoded as bytes.
    #[error("signal encoding error: {0}")]
    Encoding(#[from] postcard::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, MediaError>;
