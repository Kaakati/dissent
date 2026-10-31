//! Tells the user when someone they talk to changes identity. A root rotation to someone they share
//! a server with, or have messaged, can mean a new key. A new device of someone they have messaged
//! can mean a different person. The safety number may then no longer match. Changes are kept in a
//! log, so the user sees the notice again after a restart.

use std::collections::BTreeMap;

use dissent_events::SignedDm;
use dissent_identity::{DeviceCert, RootId};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::directory::Directory;
use crate::views::EngineEvent;

pub(crate) const ROTATED: &str = "rotated";
pub(crate) const NEW_DEVICE: &str = "new_device";

/// One recorded identity change. `key` identifies the event that caused it, so a change seen in
/// several communities is recorded once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IdentityChange {
    pub root: RootId,
    pub key: String,
    pub reason: String,
    pub ms: u64,
}

/// Records a change for `user` and returns the event to emit. Returns `None` when `key` was
/// already recorded.
pub(crate) fn record(
    log: &mut Vec<IdentityChange>,
    paths: &crate::store::Paths,
    user: RootId,
    key: String,
    reason: &str,
    ms: u64,
) -> Option<EngineEvent> {
    if log.iter().any(|c| c.key == key) {
        return None;
    }
    let change = IdentityChange {
        root: user,
        key,
        reason: reason.to_string(),
        ms,
    };
    if let Err(e) = paths.append_records(&paths.identity_changes(), std::slice::from_ref(&change)) {
        warn!(error = %e, "could not persist identity change");
    }
    log.push(change);
    Some(EngineEvent::IdentityChanged {
        user: user.to_string(),
        reason: reason.to_string(),
    })
}

/// Learns a device certificate from the network. A device that is new for someone we have
/// messaged, beside devices we already know for them, is recorded. Returns the notice to emit.
/// Every path that learns certificates from the network goes through here, so a device seen first
/// in an address announcement is not missed.
pub(crate) fn learn_device(
    directory: &mut Directory,
    dms: &BTreeMap<RootId, Vec<SignedDm>>,
    convs: &crate::groups::ConvBook,
    log: &mut Vec<IdentityChange>,
    paths: &crate::store::Paths,
    cert: &DeviceCert,
    ms: u64,
) -> Option<EngineEvent> {
    let fresh = !directory.knows(cert)
        && messaged(dms, convs, &cert.root)
        && !directory.devices_of(&cert.root).is_empty();
    let notice = if fresh {
        record(
            log,
            paths,
            cert.root,
            format!("dev:{}", cert.device),
            NEW_DEVICE,
            ms,
        )
    } else {
        None
    };
    directory.learn(cert);
    notice
}

/// When `user`'s identity last changed, in milliseconds, if it ever did.
pub(crate) fn last_change(log: &[IdentityChange], user: &RootId) -> Option<u64> {
    log.iter()
        .filter(|c| c.root == *user)
        .map(|c| c.ms)
        .max()
}
#[cfg(test)]
mod tests;

/// Whether we have messaged `root`, directly or in a two-person conversation.
pub(crate) fn messaged(
    dms: &BTreeMap<RootId, Vec<SignedDm>>,
    convs: &crate::groups::ConvBook,
    root: &RootId,
) -> bool {
    dms.contains_key(root)
        || convs
            .convs
            .values()
            .any(|c| matches!(c.kind, crate::groups::ConvKind::Dm) && c.members.contains(root))
}
