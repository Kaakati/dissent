//! Voice and video spike: a 1:1 media session on str0m, with every frame encrypted by SFrame
//! (RFC 9605) before it enters RTP.
//!
//! Signalling is opaque to this crate. [`Call::start_offer`] and [`Call::accept`] return a
//! [`Signal`], which the caller encodes with [`Signal::to_bytes`] and sends over any transport,
//! such as the RPC or gossip layer. The peer decodes it and passes it to [`Call::apply`] or
//! [`Call::accept`].
//!
//! The media key is passed in as a [`MediaKey`]. In Dissent it comes from the MLS exporter.

mod call;
mod e2ee;
mod error;
mod key;
mod signal;

pub use call::{Call, CallStats};
pub use e2ee::MAX_SENDER_ID;
pub use error::{MediaError, Result};
pub use key::MediaKey;
pub use signal::Signal;
