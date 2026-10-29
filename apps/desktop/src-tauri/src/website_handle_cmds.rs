//! The Tauri commands for website-verified handles: setting up your own in Settings › Account, and the state of anyone's
//! domain-shaped server handle for the badges. Checks run in this app, never on a peer's word (see `website_handles`).
//! Names and argument keys match `ui-next/src/lib/api.ts`.

use dissent_identity::RootId;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Semaphore;

use crate::commands::{err, now_ms, parse, AppState};
use crate::website_handles::{self, CheckView, Checks, OwnHandle, Outcome};

/// A finished check, so every badge showing that handle updates. Payload: a `CheckView`.
pub const EVENT: &str = "dissent://website-handle";
/// Most lookups running at once, so a long member list does not flood the resolver.
const MAX_LOOKUPS: usize = 4;
/// Most handles one status call asks for.
const MAX_ITEMS: usize = 200;

/// The check results and the lookup limit, managed for the whole run.
pub struct HandleChecks {
    checks: Checks,
    lookups: Semaphore,
}

impl Default for HandleChecks {
    fn default() -> Self {
        Self { checks: Checks::default(), lookups: Semaphore::new(MAX_LOOKUPS) }
    }
}

/// What Settings › Account shows: the domain being set up, the handle to claim, the TXT record to publish, and the
/// last check of it. Everything but `record_value` is empty before a domain is set.
#[derive(Serialize)]
pub struct SetupView {
    pub domain: Option<String>,
    pub handle: Option<String>,
    pub record_type: &'static str,
    pub record_name: Option<String>,
    pub record_value: String,
    pub check: Option<CheckView>,
}

#[derive(Deserialize)]
pub struct HandleItem {
    pub root_id: String,
    pub handle: String,
}

fn my_root(state: &AppState) -> RootId {
    state.identity.lock().unwrap_or_else(|e| e.into_inner()).root_id()
}

/// The handle form of a domain the user typed: what `claim_handle` gets, and what viewers check.
fn handle_for(domain: &str) -> Result<(String, String), String> {
    let typed = domain.trim().trim_start_matches('@').trim_end_matches('.');
    let normalized = dissent_discovery::normalize_domain(typed).map_err(err)?;
    let handle = dissent_events::normalize_handle(typed).map_err(|e| {
        format!("{typed} cannot be a server handle: {e}. Website handles use the server handle rules.")
    })?;
    if website_handles::domain_of(&handle).as_deref() != Some(normalized.as_str()) {
        return Err(format!("{typed} cannot be a server handle as written. Try the domain in lowercase ASCII."));
    }
    Ok((normalized, handle))
}

fn setup_view(own: &OwnHandle, root: RootId, checks: &Checks) -> Result<SetupView, String> {
    let record_value = dissent_discovery::dns_record_value(&root);
    let Some(domain) = own.domain.clone() else {
        return Ok(SetupView { domain: None, handle: None, record_type: "TXT", record_name: None, record_value, check: None });
    };
    let (_, handle) = handle_for(&domain)?;
    let record_name = dissent_discovery::dns_record_name(&domain).map_err(err)?;
    let (check, _) = checks.view(root, &handle, now_ms()?, false);
    Ok(SetupView { domain: Some(domain), handle: Some(handle), record_type: "TXT", record_name: Some(record_name), record_value, check: Some(check) })
}

/// Runs one lookup under the limit and records it. Returns the view to show and emits it to the window.
async fn run_check(app: &AppHandle, root: RootId, handle: String, domain: String) -> Result<CheckView, String> {
    let checks = app.state::<HandleChecks>();
    let outcome = {
        let _permit = checks.lookups.acquire().await.map_err(err)?;
        let record_name = dissent_discovery::dns_record_name(&domain).unwrap_or_else(|_| format!("_dissent.{domain}"));
        website_handles::outcome_of(dissent_discovery::check_dns_handle(&domain, &root).await, &record_name)
    };
    if let Outcome::Error(why) = &outcome {
        tracing::debug!(%domain, %why, "website handle check did not finish");
    }
    let now = now_ms()?;
    checks.checks.finish(root, &domain, outcome, now);
    let (view, _) = checks.checks.view(root, &handle, now, false);
    let _ = app.emit(EVENT, &view);
    Ok(view)
}

#[tauri::command]
pub fn website_handle_setup(state: State<'_, AppState>, checks: State<'_, HandleChecks>) -> Result<SetupView, String> {
    setup_view(&website_handles::load(&state.data_dir), my_root(&state), &checks.checks)
}

/// Saves the domain to set up. It must be a domain that is also a valid server handle (at most 32 characters).
#[tauri::command]
pub fn set_website_handle(
    state: State<'_, AppState>,
    checks: State<'_, HandleChecks>,
    domain: String,
) -> Result<SetupView, String> {
    let (normalized, _) = handle_for(&domain)?;
    let own = OwnHandle { domain: Some(normalized) };
    website_handles::save(&state.data_dir, &own).map_err(|e| format!("could not save the website handle: {e}"))?;
    setup_view(&own, my_root(&state), &checks.checks)
}

/// Looks up your own record now, whatever the last result was, and returns what it found.
#[tauri::command]
pub async fn check_website_handle(app: AppHandle, state: State<'_, AppState>) -> Result<CheckView, String> {
    let Some(domain) = website_handles::load(&state.data_dir).domain else {
        return Err("set a domain first".into());
    };
    let (normalized, handle) = handle_for(&domain)?;
    run_check(&app, my_root(&state), handle, normalized).await
}

/// Forgets the domain on this device. Server handles already claimed stay until another is claimed.
#[tauri::command]
pub fn remove_website_handle(state: State<'_, AppState>, checks: State<'_, HandleChecks>) -> Result<SetupView, String> {
    let root = my_root(&state);
    if let Some(domain) = website_handles::load(&state.data_dir).domain {
        checks.checks.forget(root, &domain);
    }
    let own = OwnHandle::default();
    website_handles::save(&state.data_dir, &own).map_err(|e| format!("could not remove the website handle: {e}"))?;
    setup_view(&own, root, &checks.checks)
}

/// The state of each handle as this app last checked it. Handles that are due are checked in the background, and each
/// result arrives as the `dissent://website-handle` event. A handle that is not a domain comes back `not_domain`.
#[tauri::command]
pub fn website_handle_status(
    app: AppHandle,
    checks: State<'_, HandleChecks>,
    items: Vec<HandleItem>,
) -> Result<Vec<CheckView>, String> {
    let now = now_ms()?;
    let mut out = Vec::with_capacity(items.len().min(MAX_ITEMS));
    for item in items.into_iter().take(MAX_ITEMS) {
        let root: RootId = parse("user", &item.root_id)?;
        let (view, start) = checks.checks.view(root, &item.handle, now, true);
        if let Some(domain) = start {
            let app = app.clone();
            let handle = item.handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = run_check(&app, root, handle, domain).await {
                    tracing::warn!(error = %e, "website handle check failed to run");
                }
            });
        }
        out.push(view);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_becomes_the_handle_viewers_check() {
        assert_eq!(handle_for("Example.COM.").unwrap(), ("example.com".into(), "example.com".into()));
        assert_eq!(handle_for("@example.com").unwrap().1, "example.com");
        let (domain, handle) = handle_for("bücher.example").unwrap();
        assert_eq!((domain.as_str(), handle.as_str()), ("xn--bcher-kva.example", "bücher.example"));
    }

    #[test]
    fn a_domain_that_cannot_be_a_server_handle_is_refused() {
        assert!(handle_for("localhost").is_err());
        let long = format!("{}.example.com", "a".repeat(30));
        let why = handle_for(&long).unwrap_err();
        assert!(why.contains("cannot be a server handle"), "{why}");
    }
}
