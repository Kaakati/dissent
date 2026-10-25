//! Signed, hash-linked event DAG with deterministic permission replay, plus direct messages.
//!
//! * [`event`]: signed events. An event's id is the blake3 hash of its body.
//! * [`control`]: control payloads sealed under a community secret, opened by replay.
//! * [`hlc`]: hybrid logical clock stamps used to order concurrent events.
//! * [`dag`]: in-memory store. Holds events whose parents have not arrived yet.
//! * [`state`]: replays the causal order into [`CommunityState`].
//! * [`wire`]: events packaged with the device cert needed to verify them.
//! * [`dm`]: signed 1:1 direct messages.
//!
//! Every peer that holds the same set of events computes the same state,
//! regardless of the order in which it received them.

pub mod control;
pub mod dag;
pub mod dm;
pub mod emoji;
pub mod event;
pub mod hlc;
pub mod names;
pub mod picture;
pub mod relay;
pub mod replayer;
pub mod state;
#[cfg(test)]
pub(crate) mod testutil;
pub mod wire;

pub use control::{
    is_member_sealable, is_sealable, open_event, open_event_keyed, open_member, seal, seal_member, verify_member_tag,
    EpochKey, EpochKeys, OpenError, SealKey, EXPORT_LABEL,
};
pub use dag::{Dag, InsertOutcome};
pub use dm::{DmBody, DmId, DmMessage, SignedDm};
pub use emoji::{normalize_emoji, EmojiError};
pub use event::{
    ChannelId, ChannelVisibility, Event, EventBody, EventId, HistorySetting, Payload, Role,
    SealedKind,
};
pub use hlc::{Hlc, HlcClock};
pub use names::{confusable, normalize_handle, skeleton, NameError};
pub use picture::{Picture, MAX_PICTURE_BYTES};
pub use relay::{valid_relay_url, valid_relays, MAX_COMMUNITY_RELAYS};
pub use replayer::{ReplayStats, Replayer};
pub use state::{replay, replay_keyed, replay_with, CommunityState, Member, MessageView, Rejection};
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
