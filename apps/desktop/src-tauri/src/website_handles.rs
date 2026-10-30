//! Website-verified handles: a server handle shaped like a domain, such as `@example.com`, is shown as verified only
//! after this app finds `dissent-root=<their root>` in the TXT record at `_dissent.example.com` itself (see
//! `dissent_discovery::check_dns_handle` and `docs/design/03-recovery-and-safety.md`). A peer's word is never used.
//!
//! The claim travels as an ordinary `HandleClaim` in each server, so nothing on the wire changes. This module keeps
//! the results of those checks for this run, decides when each is stale or due again, and stores the domain you are
//! setting up for yourself in `website-handle.json` in the data folder.
//!
//! A lookup tells the resolver, and the domain's own name servers, that someone is looking at that handle. So a
//! handle is checked only when the user opens that person's profile or points at the handle, never because it was
//! drawn in a list or a message, and a result is reused for hours.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use dissent_discovery::{DiscoveryError, DnsHandleCheck};
use dissent_identity::RootId;
use serde::{Deserialize, Serialize};

const FILE: &str = "website-handle.json";
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// A pass is shown as verified for this long. After it the handle is "not checked yet" until a new check passes.
pub const VERIFIED_FOR_MS: u64 = 24 * HOUR_MS;
/// A pass is checked again, when the user next asks, once it is this old, well before it goes stale.
pub const RECHECK_AFTER_MS: u64 = 6 * HOUR_MS;
/// A definite failure (no record, wrong value) is shown for this long before it can be checked again.
pub const RETRY_FAILED_MS: u64 = HOUR_MS;
/// A lookup that could not finish (offline, timed out) can be tried again after this.
pub const RETRY_ERROR_MS: u64 = 10 * MINUTE_MS;
/// Most checks remembered. The oldest are dropped first.
pub const MAX_ENTRIES: usize = 1024;

/// What one check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Verified,
    /// The record is missing or holds another value. The text says which, for the person who set it up.
    Failed(String),
    /// The lookup did not finish, so nothing is known. Shown as "not checked yet".
    Error(String),
}

/// The state of a handle as the UI shows it. `unchecked` covers never checked, stale and could not check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Verified,
    Failed,
    Unchecked,
    /// The handle is not a domain, so there is nothing to check.
    NotDomain,
}

/// One handle's state for the UI. `handle` is echoed as it was asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckView {
    pub root_id: String,
    pub handle: String,
    pub state: State,
    pub checked_ms: Option<u64>,
    /// Why it failed, or why it could not be checked.
    pub reason: Option<String>,
    /// A check is running now.
    pub checking: bool,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    last: Option<(Outcome, u64)>,
    in_flight: bool,
    /// Last time this entry was touched, for eviction.
    used_ms: u64,
}

/// The state shown for a result checked at `checked_ms`.
pub fn state_of(last: Option<&(Outcome, u64)>, now_ms: u64) -> (State, Option<String>) {
    match last {
        Some((Outcome::Verified, at)) if now_ms.saturating_sub(*at) < VERIFIED_FOR_MS => (State::Verified, None),
        Some((Outcome::Failed(why), _)) => (State::Failed, Some(why.clone())),
        Some((Outcome::Error(why), _)) => (State::Unchecked, Some(why.clone())),
        _ => (State::Unchecked, None),
    }
}

/// True when a new check should start: never checked, or the last result is old enough for its kind.
pub fn due(last: Option<&(Outcome, u64)>, now_ms: u64) -> bool {
    let Some((outcome, at)) = last else { return true };
    let age = now_ms.saturating_sub(*at);
    age >= match outcome {
        Outcome::Verified => RECHECK_AFTER_MS,
        Outcome::Failed(_) => RETRY_FAILED_MS,
        Outcome::Error(_) => RETRY_ERROR_MS,
    }
}

/// The domain a handle names, normalized, or `None` when it is not one.
pub fn domain_of(handle: &str) -> Option<String> {
    let handle = handle.trim().trim_start_matches('@');
    if !handle.contains('.') {
        return None;
    }
    dissent_discovery::normalize_domain(handle).ok()
}

/// What a check result means, in words for the settings page and the profile.
pub fn outcome_of(result: Result<DnsHandleCheck, DiscoveryError>, record_name: &str) -> Outcome {
    match result {
        Ok(DnsHandleCheck::Verified) => Outcome::Verified,
        Ok(DnsHandleCheck::NoRecord) => Outcome::Failed(format!(
            "No TXT record was found at {record_name}. If you just added it, DNS can take a while to update."
        )),
        Ok(DnsHandleCheck::Mismatch { other_root: true }) => Outcome::Failed(format!(
            "The TXT record at {record_name} is for a different Dissent identity."
        )),
        Ok(DnsHandleCheck::Mismatch { other_root: false }) => Outcome::Failed(format!(
            "{record_name} has TXT records, but none is exactly the value to publish."
        )),
        Err(DiscoveryError::InvalidDomain(why)) => Outcome::Failed(format!("Not a domain that can be checked: {why}.")),
        Err(e) => Outcome::Error(format!("Could not check: {e}.")),
    }
}

/// The results of this run's checks, keyed by root and normalized domain.
#[derive(Default)]
pub struct Checks {
    entries: Mutex<HashMap<(RootId, String), Entry>>,
}

impl Checks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(RootId, String), Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The view of `handle` for `root`, and whether a check should start now. A check that should start is marked
    /// running, so a second caller does not start another; the caller must then call [`Checks::finish`].
    pub fn view(&self, root: RootId, handle: &str, now_ms: u64, start_if_due: bool) -> (CheckView, Option<String>) {
        let Some(domain) = domain_of(handle) else {
            return (not_domain(root, handle), None);
        };
        let mut entries = self.lock();
        let entry = entries.entry((root, domain.clone())).or_default();
        entry.used_ms = now_ms;
        let start = start_if_due && !entry.in_flight && due(entry.last.as_ref(), now_ms);
        if start {
            entry.in_flight = true;
        }
        let view = view_of(root, handle, entry, now_ms);
        let evict = entries.len() > MAX_ENTRIES;
        if evict {
            evict_oldest(&mut entries);
        }
        (view, start.then_some(domain))
    }

    /// Records a finished check.
    pub fn finish(&self, root: RootId, domain: &str, outcome: Outcome, now_ms: u64) {
        let mut entries = self.lock();
        let entry = entries.entry((root, domain.to_string())).or_default();
        entry.last = Some((outcome, now_ms));
        entry.in_flight = false;
        entry.used_ms = now_ms;
    }

    /// Forgets the result for one domain, as when you remove your own.
    pub fn forget(&self, root: RootId, domain: &str) {
        self.lock().remove(&(root, domain.to_string()));
    }
}

fn not_domain(root: RootId, handle: &str) -> CheckView {
    CheckView {
        root_id: root.to_string(),
        handle: handle.to_string(),
        state: State::NotDomain,
        checked_ms: None,
        reason: None,
        checking: false,
    }
}

fn view_of(root: RootId, handle: &str, entry: &Entry, now_ms: u64) -> CheckView {
    let (state, reason) = state_of(entry.last.as_ref(), now_ms);
    CheckView {
        root_id: root.to_string(),
        handle: handle.to_string(),
        state,
        checked_ms: entry.last.as_ref().map(|(_, at)| *at),
        reason,
        checking: entry.in_flight,
    }
}

fn evict_oldest(entries: &mut HashMap<(RootId, String), Entry>) {
    let oldest = entries
        .iter()
        .filter(|(_, e)| !e.in_flight)
        .min_by_key(|(_, e)| e.used_ms)
        .map(|(k, _)| k.clone());
    if let Some(key) = oldest {
        entries.remove(&key);
    }
}

/// The domain you are setting up as your own handle. Kept on this device only; others learn it from your server handle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OwnHandle {
    pub domain: Option<String>,
}

/// The saved setup, or none when the file is missing or cannot be read.
pub fn load(data_dir: &Path) -> OwnHandle {
    std::fs::read(data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(bytes.strip_prefix(BOM).unwrap_or(&bytes)).ok())
        .unwrap_or_default()
}

/// Writes the setup through a temporary file, so a crash never leaves half a file.
pub fn save(data_dir: &Path, own: &OwnHandle) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let bytes = serde_json::to_vec_pretty(own).map_err(io::Error::other)?;
    let tmp = data_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, data_dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    const T0: u64 = 1_800_000_000_000;

    #[test]
    fn only_domains_are_checked() {
        assert_eq!(domain_of("example.com").as_deref(), Some("example.com"));
        assert_eq!(domain_of("@Example.COM").as_deref(), Some("example.com"));
        assert_eq!(domain_of("maya"), None);
        assert_eq!(domain_of("exa_mple.com"), None);
        assert_eq!(domain_of("a..b"), None);
    }

    #[test]
    fn a_pass_goes_stale_and_is_never_shown_after_a_failure() {
        let pass = (Outcome::Verified, T0);
        assert_eq!(state_of(Some(&pass), T0 + 1).0, State::Verified);
        assert_eq!(state_of(Some(&pass), T0 + VERIFIED_FOR_MS - 1).0, State::Verified);
        assert_eq!(state_of(Some(&pass), T0 + VERIFIED_FOR_MS).0, State::Unchecked);
        assert_eq!(state_of(None, T0).0, State::Unchecked);
        let fail = (Outcome::Failed("gone".into()), T0);
        assert_eq!(state_of(Some(&fail), T0), (State::Failed, Some("gone".into())));
        let error = (Outcome::Error("offline".into()), T0);
        assert_eq!(state_of(Some(&error), T0), (State::Unchecked, Some("offline".into())));
    }

    #[test]
    fn a_result_is_reused_for_hours() {
        let hour = 60 * MINUTE_MS;
        assert!(!due(Some(&(Outcome::Verified, T0)), T0 + 5 * hour));
        assert!(!due(Some(&(Outcome::Failed(String::new()), T0)), T0 + hour - 1));
        assert!(!due(Some(&(Outcome::Error(String::new()), T0)), T0 + 9 * MINUTE_MS));
        assert!(VERIFIED_FOR_MS >= 24 * hour);
    }

    #[test]
    fn a_pass_is_rechecked_before_it_goes_stale() {
        let pass = (Outcome::Verified, T0);
        assert!(due(None, T0));
        assert!(!due(Some(&pass), T0 + RECHECK_AFTER_MS - 1));
        assert!(due(Some(&pass), T0 + RECHECK_AFTER_MS));
        const { assert!(RECHECK_AFTER_MS < VERIFIED_FOR_MS) };
        assert!(due(Some(&(Outcome::Failed(String::new()), T0)), T0 + RETRY_FAILED_MS));
        assert!(!due(Some(&(Outcome::Error(String::new()), T0)), T0 + RETRY_ERROR_MS - 1));
    }

    #[test]
    fn one_check_runs_at_a_time_and_its_result_is_shown() {
        let checks = Checks::default();
        let root = RootKey::generate().id();
        let (view, start) = checks.view(root, "Example.com", T0, true);
        assert_eq!(start.as_deref(), Some("example.com"));
        assert_eq!(view.state, State::Unchecked);
        assert!(view.checking);
        let (_, again) = checks.view(root, "example.com", T0, true);
        assert_eq!(again, None, "a running check is not started twice");
        checks.finish(root, "example.com", Outcome::Verified, T0 + 5);
        let (view, start) = checks.view(root, "Example.com", T0 + 10, true);
        assert_eq!((view.state, view.checked_ms, start), (State::Verified, Some(T0 + 5), None));
        assert_eq!(view.handle, "Example.com");
        // Another person with the same domain is checked on their own.
        let other = RootKey::generate().id();
        assert_eq!(checks.view(other, "example.com", T0, false).0.state, State::Unchecked);
        checks.forget(root, "example.com");
        assert_eq!(checks.view(root, "example.com", T0 + 20, false).0.state, State::Unchecked);
    }

    #[test]
    fn a_handle_that_is_not_a_domain_is_never_checked() {
        let checks = Checks::default();
        let (view, start) = checks.view(RootKey::generate().id(), "maya", T0, true);
        assert_eq!((view.state, start), (State::NotDomain, None));
    }

    #[test]
    fn the_oldest_entry_is_dropped_past_the_cap() {
        let checks = Checks::default();
        let root = RootKey::generate().id();
        for i in 0..=MAX_ENTRIES {
            let domain = format!("d{i}.example");
            checks.view(root, &domain, T0 + i as u64, false);
        }
        assert_eq!(checks.lock().len(), MAX_ENTRIES);
        assert!(!checks.lock().contains_key(&(root, "d0.example".into())));
    }

    #[test]
    fn reasons_say_what_to_fix() {
        let name = "_dissent.example.com";
        assert_eq!(outcome_of(Ok(DnsHandleCheck::Verified), name), Outcome::Verified);
        let Outcome::Failed(why) = outcome_of(Ok(DnsHandleCheck::NoRecord), name) else { panic!() };
        assert!(why.contains("No TXT record") && why.contains(name));
        let Outcome::Failed(why) = outcome_of(Ok(DnsHandleCheck::Mismatch { other_root: true }), name) else { panic!() };
        assert!(why.contains("different Dissent identity"));
        let Outcome::Error(why) = outcome_of(Err(DiscoveryError::Dns("timed out".into())), name) else { panic!() };
        assert!(why.starts_with("Could not check") && why.contains("timed out"));
    }

    #[test]
    fn the_setup_round_trips_and_a_missing_file_is_none() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-webhandle-{}-{nanos}", std::process::id()));
        assert_eq!(load(&dir), OwnHandle::default());
        let own = OwnHandle { domain: Some("example.com".into()) };
        save(&dir, &own).unwrap();
        assert_eq!(load(&dir), own);
        std::fs::remove_dir_all(&dir).ok();
    }
}
