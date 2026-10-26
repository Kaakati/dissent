//! Voice and video spike: a 1:1 media session on str0m, with every frame encrypted by SFrame
//! (RFC 9605) before it enters RTP.
//!
//! Signalling is opaque to this crate. [`Call::start_offer`] and [`Call::accept`] return a
//! [`Signal`], which the caller encodes with [`Signal::to_bytes`] and sends over any transport,
//! such as the RPC or gossip layer. The peer decodes it and passes it to [`Call::apply`] or
//! [`Call::accept`].
//!
//! Media packets move over a [`PacketTransport`]. [`UdpTransport`] is the direct-IP one. Calls
//! over other paths, such as iroh, use [`Call::offer_over`] and [`Call::accept_over`].
//!
//! The media key is passed in as a [`MediaKey`]. In Dissent it comes from the MLS exporter.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

mod auth;
mod bwe;
mod call;
mod cascade;
mod e2ee;
mod election;
mod error;
mod frame;
mod key;
mod link;
mod mesh;
mod participant;
mod sfu;
mod signal;
mod transport;
mod video;

pub use auth::Auth;
pub use bwe::{pick_layer, BandwidthEstimator, MAX_KBPS, MIN_KBPS};
pub use call::{Call, CallStats};
pub use cascade::{assign_forwarders, load, plan, Cascade};
pub use e2ee::MAX_SENDER_ID;
pub use election::{
    apply_centrality, capacity, centrality, elect, ranked, Candidate, Election, Mode, PeerId,
};
pub use error::{MediaError, Result};
pub use frame::{ReceivedFrame, FRAME_SAMPLES};
pub use key::MediaKey;
pub use link::{LinkTransport, Side};
pub use mesh::Mesh;
pub use participant::{Failover, FailoverReason, Participant, SILENCE_TIMEOUT};
pub use sfu::{Observer, Sfu, SfuStats};
pub use signal::Signal;
pub use transport::{PacketTransport, UdpTransport};
pub use video::VideoCodec;
pub use video::VideoMeta;
pub use video::VideoFrame;

/// Host candidate of the caller on a transport with no IP path of its own, such as iroh.
///
/// str0m needs an address per candidate and tracks each ICE pair by address. Both sides use
/// fixed synthetic addresses, and the transport maps every datagram to them. Nothing is routed
/// by these addresses. They only let ICE and str0m run.
pub const SYNTHETIC_CALLER: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 255, 0, 1)), 1);

/// Host candidate of the callee on a transport with no IP path of its own. See
/// [`SYNTHETIC_CALLER`].
pub const SYNTHETIC_CALLEE: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 255, 0, 2)), 1);
