//! Signed, hash-linked event DAG with deterministic permission replay, plus direct messages.
//!
//! * [`event`]: signed events. An event's id is the blake3 hash of its body.
//! * [`hlc`]: hybrid logical clock stamps used to order concurrent events.
//! * [`dag`]: in-memory store. Holds events whose parents have not arrived yet.
//! * [`state`]: replays the causal order into [`CommunityState`].
//! * [`wire`]: events packaged with the device cert needed to verify them.
//! * [`dm`]: signed 1:1 direct messages.
//!
//! Every peer that holds the same set of events computes the same state,
//! regardless of the order in which it received them.

pub mod dag;
pub mod dm;
pub mod emoji;
pub mod event;
pub mod hlc;
pub mod names;
pub mod replayer;
pub mod state;
#[cfg(test)]
pub(crate) mod testutil;
pub mod wire;

pub use dag::{Dag, InsertOutcome};
pub use dm::{DmBody, DmId, DmMessage, SignedDm};
pub use emoji::{normalize_emoji, EmojiError};
pub use event::{ChannelId, Event, EventBody, EventId, Payload, Role, SealedKind};
pub use hlc::{Hlc, HlcClock};
pub use names::{confusable, normalize_handle, skeleton, NameError};
pub use replayer::{ReplayStats, Replayer};
pub use state::{replay, CommunityState, Member, MessageView, Rejection};
pub use wire::SignedEvent;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("id does not match its body")]
    IdMismatch,
    #[error("hlc is not strictly after every parent's hlc")]
    HlcNotAfterParents,
    #[error("not in canonical form: {0}")]
    NonCanonical(&'static str),
    #[error("device does not match the certificate")]
    DeviceMismatch,
    #[error("author does not match the certificate root")]
    AuthorMismatch,
    #[error(transparent)]
    Identity(#[from] dissent_identity::IdentityError),
    #[error("encoding: {0}")]
    Encoding(String),
}

pub type Result<T> = std::result::Result<T, Error>;
