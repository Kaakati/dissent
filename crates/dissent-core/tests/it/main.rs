//! Integration tests for the engine, built as one binary so the crate links once. Most run
//! engines on this machine with relays off. Networked ones are `#[ignore]`d and run through
//! `scripts/fwtest.sh` on Windows; see CLAUDE.md.

mod common;

mod addresses;
mod attachments;
mod avatars;
mod calls;
mod channel_settings;
mod contacts;
mod control_latency;
mod dm_actions;
mod groups;
mod identity_flows;
mod internet;
mod invite_cards;
mod user_address;
mod invites;
mod leave;
mod member_sealed;
mod legacy_state;
mod part6;
mod peer_exchange;
mod presence;
mod previews;
mod reactions;
mod relays;
mod replies;
mod status;
mod sync;
mod two_peers;
mod typing;
mod voice_presence;
mod upgrade_compat;
mod warm;
