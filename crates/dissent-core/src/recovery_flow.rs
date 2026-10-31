//! Guardian recovery as the app runs it (`docs/design/03-recovery-and-safety.md`, "Recovering a lost key").
//!
//! The recovering device holds a fresh root. It makes a request naming the lost root and shows it as a
//! `dissent://recover/...` link, with this device's address after `?via=`, and a nine-digit code. Each guardian opens
//! the link, hears the code from the person on a call or face to face, and approves only when the two match. Their
//! app signs the approval and sends it straight to the new device when the address reaches it; the approval link is
//! the fallback to send back by hand. With the approval go the owner's signed guardian list, which every guardian keeps
//! from the moment it was named (`Request::GuardianOf`), and, if the guardian chooses, invites to the servers they
//! share with the lost root. A guardian can also decline, signed, so the person sees who said no.
//!
//! Once the threshold of the list's guardians approved, `finish_recovery` assembles the rotation and runs
//! `complete_recovery`. Nothing secret moves: approvals, the list and invites are all things the person could paste.

use std::collections::BTreeMap;

use dissent_identity::{GuardianApproval, GuardianSet, RootId, RotationRequest, Signature};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};
use crate::views::EngineEvent;

const DECLINE_DOMAIN: &[u8] = b"dissent/recovery-decline/v1";
const CODE_DOMAIN: &[u8] = b"dissent/recovery-code/v1";
/// Invites one answer may carry, and that a recovery keeps in all.
const MAX_INVITES: usize = 16;
/// Where the recovering device's address starts in a request link.
const VIA: &str = "?via=";

/// A recovery this device started, kept until it finishes or is cancelled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PendingRecovery {
    pub request: RotationRequest,
    pub set: Option<GuardianSet>,
    pub approvals: Vec<GuardianApproval>,
    /// Guardians who declined, each checked against its signature.
    pub declined: Vec<RootId>,
    pub invites: Vec<String>,
}

/// A guardian's answer, as it travels to the recovering device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RecoveryAnswer {
    pub request: RotationRequest,
    pub answer: Answer,
    /// The lost root's signed guardian list, when the guardian holds it.
    pub set: Option<GuardianSet>,
    pub invites: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Answer {
    Approved(GuardianApproval),
    Declined { guardian: RootId, sig: Signature },
}

/// The recovery in progress on this device, as the recovery screen reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecoveryView {
    /// The lost root, base32.
    pub old_root: String,
    pub old_suffix: String,
    /// The link to send each guardian. It carries this device's address, so their answer comes back by itself.
    pub request_link: String,
    /// Nine digits, `123 456 789`, to read out to each guardian. They approve only when theirs matches.
    pub code: String,
    /// How many guardians must approve, once the guardian list is known.
    pub threshold: Option<u8>,
    /// Every guardian of the list, or every guardian who answered while the list is unknown.
    pub guardians: Vec<RecoveryGuardian>,
    /// Approvals from guardians on the list. Before the list is known, every approval counts here.
    pub approvals: usize,
    pub set_known: bool,
    pub invites: usize,
    /// True once the list is known and enough of its guardians approved.
    pub ready: bool,
    pub requested_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecoveryGuardian {
    pub root_id: String,
    pub suffix: String,
    pub display_name: Option<String>,
    /// `waiting`, `approved` or `declined`.
    pub state: String,
}

/// A recovery request as a guardian's app shows it before they answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecoveryRequestView {
    pub old_root: String,
    pub old_suffix: String,
    /// The name we know the lost root by, if any.
    pub old_name: Option<String>,
    pub new_suffix: String,
    /// Whether we are on the lost root's guardian list, `None` when we hold no list for it.
    pub listed: Option<bool>,
    /// Servers both we and the lost root are in, whose invites we can send along.
    pub shared_servers: Vec<String>,
    /// Whether the link names the device to send the answer to.
    pub reachable: bool,
    pub requested_ms: u64,
    /// When we last saw this account's key change, if we did. A recent change may mean someone already recovered
    /// it, or took it over.
    pub key_changed_ms: Option<u64>,
    /// Whether a server we are in already moved this account to another key.
    pub already_moved: bool,
}

/// What answering did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecoveryAnswerView {
    /// The `dissent://approve/...` link to send back by hand, for an approval.
    pub approval_link: Option<String>,
    /// Whether the new device took the answer.
    pub delivered: bool,
    /// Why it was not delivered, when it was not.
    pub note: Option<String>,
}

/// The code both sides compare: nine digits from the request, in groups of three.
pub(crate) fn code_of(request: &RotationRequest) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CODE_DOMAIN);
    hasher.update(&postcard::to_allocvec(request).unwrap_or_default());
    let bytes = hasher.finalize();
    let n = u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap_or([0; 8])) % 1_000_000_000;
    let s = format!("{n:09}");
    format!("{} {} {}", &s[0..3], &s[3..6], &s[6..9])
}

fn decline_bytes(request: &RotationRequest) -> Vec<u8> {
    let mut v = DECLINE_DOMAIN.to_vec();
    v.extend(postcard::to_allocvec(request).unwrap_or_default());
    v
}

/// Splits a request link into the request and the recovering device's address, if it names one.
fn parse_request_link(link: &str) -> Result<(RotationRequest, Option<dissent_net::PeerAddr>)> {
    let link = link.trim();
    let (base, via) = match link.split_once(VIA) {
        Some((base, via)) => (base, Some(via)),
        None => (link, None),
    };
    let request = RotationRequest::parse(base)?;
    let addr = via.and_then(|v| {
        let bytes = data_encoding::BASE32_NOPAD.decode(v.to_ascii_uppercase().as_bytes()).ok()?;
        postcard::from_bytes::<dissent_net::PeerAddr>(&bytes).ok()
    });
    Ok((request, addr))
}

fn digits(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

impl Engine {
    fn load_recovery(&self) -> Result<Option<PendingRecovery>> {
        {
            let paths = self.paths();
            paths.read_bin(&paths.recovery())
        }
    }

    fn save_recovery(&self, pending: Option<&PendingRecovery>) -> Result<()> {
        let paths = self.paths();
        let path = paths.recovery();
        match pending {
            Some(p) => paths.write_bin(&path, p),
            None => match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    fn recovery_view(&self, p: &PendingRecovery) -> Result<RecoveryView> {
        let base = p.request.to_link()?;
        let request_link = match self.my_address().and_then(|a| postcard::to_allocvec(&a).ok()) {
            Some(bytes) => format!("{base}{VIA}{}", data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()),
            None => base,
        };
        let w = self.read();
        let name_of = |root: &RootId| {
            let name = w.display_name(root);
            (name != root.to_string()).then_some(name)
        };
        let state_of = |root: &RootId| {
            if p.approvals.iter().any(|a| a.guardian == *root) {
                "approved"
            } else if p.declined.contains(root) {
                "declined"
            } else {
                "waiting"
            }
        };
        let listed: Vec<RootId> = match &p.set {
            Some(set) => set.guardians.clone(),
            None => {
                let mut seen: Vec<RootId> = p.approvals.iter().map(|a| a.guardian).collect();
                seen.extend(p.declined.iter().copied());
                seen.dedup();
                seen
            }
        };
        let guardians = listed
            .iter()
            .map(|root| RecoveryGuardian {
                root_id: root.to_string(),
                suffix: root.suffix(),
                display_name: name_of(root),
                state: state_of(root).to_string(),
            })
            .collect();
        let approvals = match &p.set {
            Some(set) => p.approvals.iter().filter(|a| set.guardians.contains(&a.guardian)).count(),
            None => p.approvals.len(),
        };
        let threshold = p.set.as_ref().map(|s| s.threshold);
        Ok(RecoveryView {
            old_root: p.request.old_root.to_string(),
            old_suffix: p.request.old_root.suffix(),
            request_link,
            code: code_of(&p.request),
            threshold,
            guardians,
            approvals,
            set_known: p.set.is_some(),
            invites: p.invites.len(),
            ready: threshold.is_some_and(|t| approvals >= usize::from(t)),
            requested_ms: p.request.requested_ms,
        })
    }

    /// Starts recovering `old` on this device, which holds a fresh root: a root id, or the lost root's
    /// `dissent://guardians/...` link. A recovery already started for the same root is kept.
    pub fn begin_recovery(&self, old: &str) -> Result<RecoveryView> {
        let old = old.trim();
        let (old_root, set) = if old.starts_with("dissent://guardians/") {
            let set = GuardianSet::parse(old)?;
            set.verify()?;
            (set.owner, Some(set))
        } else {
            let root: RootId = old.parse().map_err(|_| CoreError::Invalid("that is not an account id or a guardians link".into()))?;
            (root, None)
        };
        if old_root == self.inner.me {
            return Err(CoreError::Invalid("this is the account on this device; recover a lost one".into()));
        }
        let pending = match self.load_recovery()? {
            Some(mut p) if p.request.old_root == old_root => {
                if p.set.is_none() {
                    p.set = set;
                }
                p
            }
            _ => {
                let request = RotationRequest::new_for(self.root_key()?, old_root, now_ms());
                PendingRecovery { request, set, approvals: Vec::new(), declined: Vec::new(), invites: Vec::new() }
            }
        };
        self.save_recovery(Some(&pending))?;
        self.recovery_view(&pending)
    }

    /// The recovery in progress on this device, if any.
    pub fn recovery(&self) -> Result<Option<RecoveryView>> {
        self.load_recovery()?.map(|p| self.recovery_view(&p)).transpose()
    }

    /// Adds a link a guardian sent by hand: an approval, the lost root's guardians link, or an invite.
    pub fn add_recovery_link(&self, link: &str) -> Result<RecoveryView> {
        let mut p = self.load_recovery()?.ok_or_else(|| CoreError::Invalid("no recovery is in progress".into()))?;
        let link = link.trim();
        if link.starts_with("dissent://approve/") {
            let approval = GuardianApproval::parse(link)?;
            approval.verify_for(&p.request)?;
            add_approval(&mut p, approval);
        } else if link.starts_with("dissent://guardians/") {
            let set = GuardianSet::parse(link)?;
            set.verify()?;
            if set.owner != p.request.old_root {
                return Err(CoreError::Invalid("that guardian list is for another account".into()));
            }
            p.set = Some(set);
        } else if link.starts_with("dissent://invite/") {
            add_invites(&mut p, std::iter::once(link.to_string()));
        } else {
            return Err(CoreError::Invalid("paste an approval, guardians or invite link".into()));
        }
        self.save_recovery(Some(&p))?;
        self.recovery_view(&p)
    }

    /// Finishes the recovery once the threshold approved: rotates the lost root to this device's root in every
    /// server it can reach through the invites. Returns how many servers took the rotation.
    pub async fn finish_recovery(&self) -> Result<usize> {
        let p = self.load_recovery()?.ok_or_else(|| CoreError::Invalid("no recovery is in progress".into()))?;
        let set = p.set.as_ref().ok_or_else(|| CoreError::Invalid("the guardian list has not arrived yet".into()))?;
        let approvals: Vec<String> = p
            .approvals
            .iter()
            .filter(|a| set.guardians.contains(&a.guardian))
            .map(|a| a.to_link())
            .collect::<std::result::Result<_, _>>()?;
        if approvals.len() < usize::from(set.threshold) {
            return Err(CoreError::Invalid(format!("{} of {} guardians approved so far", approvals.len(), set.threshold)));
        }
        let published = self.complete_recovery(&approvals, &set.to_link()?, &p.invites).await?;
        self.save_recovery(None)?;
        self.emit(EngineEvent::RecoveryChanged);
        Ok(published)
    }

    /// Drops the recovery in progress. The request's link stops counting for anything on this device.
    pub fn cancel_recovery(&self) -> Result<()> {
        self.save_recovery(None)?;
        self.emit(EngineEvent::RecoveryChanged);
        Ok(())
    }

    /// Takes a guardian's answer to the recovery in progress. Anything that does not check out is refused.
    pub(crate) fn receive_recovery_answer(&self, answer: RecoveryAnswer) -> Response {
        let mut p = match self.load_recovery() {
            Ok(Some(p)) if p.request == answer.request => p,
            Ok(_) => return Response::Refused("no such recovery here".into()),
            Err(e) => return Response::Error(e.to_string()),
        };
        match &answer.answer {
            Answer::Approved(approval) => {
                if approval.verify_for(&p.request).is_err() {
                    return Response::Refused("the approval does not verify".into());
                }
                add_approval(&mut p, approval.clone());
            }
            Answer::Declined { guardian, sig } => {
                if guardian.verify(&decline_bytes(&p.request), sig).is_err() {
                    return Response::Refused("the decline does not verify".into());
                }
                if !p.declined.contains(guardian) && !p.approvals.iter().any(|a| a.guardian == *guardian) {
                    p.declined.push(*guardian);
                }
            }
        }
        if let Some(set) = answer.set.filter(|s| s.owner == p.request.old_root && s.verify().is_ok()) {
            if p.set.as_ref().is_none_or(|have| set.created_ms > have.created_ms) {
                p.set = Some(set);
            }
        }
        add_invites(&mut p, answer.invites.into_iter().take(MAX_INVITES));
        if let Err(e) = self.save_recovery(Some(&p)) {
            return Response::Error(e.to_string());
        }
        self.emit(EngineEvent::RecoveryChanged);
        Response::Ack
    }

    // ---- the guardian's side ----

    fn load_guarded(&self) -> BTreeMap<RootId, GuardianSet> {
        {
            let paths = self.paths();
            paths.read_bin::<Vec<GuardianSet>>(&paths.guarded())
        }
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.owner, s))
            .collect()
    }

    /// Keeps a list that names us as a guardian, so we can send it to the owner's new device if they lose theirs.
    pub(crate) fn accept_guardian_of(&self, set: GuardianSet) -> Response {
        if set.verify().is_err() || !set.guardians.contains(&self.inner.me) {
            return Response::Refused("not a list that names this account".into());
        }
        let mut sets = self.load_guarded();
        if sets.get(&set.owner).is_some_and(|have| have.created_ms >= set.created_ms) {
            return Response::Ack;
        }
        sets.insert(set.owner, set);
        let all: Vec<GuardianSet> = sets.into_values().collect();
        let paths = self.paths();
        match paths.write_bin(&paths.guarded(), &all) {
            Ok(()) => Response::Ack,
            Err(e) => Response::Error(e.to_string()),
        }
    }

    /// Sends our new guardian list to each guardian's devices we can reach. Each keeps it for our recovery.
    pub(crate) async fn tell_guardians(&self, set: &GuardianSet) {
        let peers: Vec<dissent_net::PeerAddr> = {
            let w = self.read();
            let devices: Vec<dissent_identity::DeviceId> =
                set.guardians.iter().flat_map(|g| w.directory.devices_of(g)).collect();
            w.addresses.peers().into_iter().filter(|p| devices.contains(&p.id)).collect()
        };
        let request = Request::GuardianOf(Box::new(set.clone()));
        for peer in peers {
            if let Err(e) = self.node().remember(&peer) {
                debug!(error = %e, "could not remember a guardian's device");
            }
            if let Err(e) = self.call(peer.id, &request).await {
                debug!(device = %peer.id, error = %e, "the guardian list did not reach a guardian's device");
            }
        }
    }

    /// What a recovery request link asks, for the guardian to look at before answering.
    pub fn inspect_recovery_request(&self, link: &str) -> Result<RecoveryRequestView> {
        let (request, addr) = parse_request_link(link)?;
        let listed = self.load_guarded().get(&request.old_root).map(|s| s.guardians.contains(&self.inner.me));
        let w = self.read();
        let name = w.display_name(&request.old_root);
        let shared_servers = w
            .communities
            .values()
            .filter(|c| c.is_member(&request.old_root))
            .map(|c| c.name().to_string())
            .collect();
        let key_changed_ms = w.identity_changes.iter().filter(|c| c.root == request.old_root).map(|c| c.ms).max();
        let already_moved = w.communities.values().any(|c| c.state().aliases.contains_key(&request.old_root));
        Ok(RecoveryRequestView {
            key_changed_ms,
            already_moved,
            old_root: request.old_root.to_string(),
            old_suffix: request.old_root.suffix(),
            old_name: (name != request.old_root.to_string()).then_some(name),
            new_suffix: request.new_root.suffix(),
            listed,
            shared_servers,
            reachable: addr.is_some(),
            requested_ms: request.requested_ms,
        })
    }

    /// Answers a recovery request as a guardian. An approval needs the code the person read out, so a link
    /// passed on by someone else cannot be approved unseen. The answer goes to the new device when the link names
    /// it; the approval link is returned either way.
    pub async fn answer_recovery_request(
        &self,
        link: &str,
        approve: bool,
        code: Option<&str>,
        send_invites: bool,
    ) -> Result<RecoveryAnswerView> {
        let (request, addr) = parse_request_link(link)?;
        if request.old_root == self.inner.me {
            return Err(CoreError::Invalid("you cannot approve a recovery of your own account".into()));
        }
        let root = self.root_key()?;
        let (answer, approval_link) = if approve {
            let given = code.map(digits).unwrap_or_default();
            if given != digits(&code_of(&request)) {
                return Err(CoreError::Invalid("the code does not match. Ask them to read it again from their screen".into()));
            }
            let approval = GuardianApproval::sign(root, request);
            let link = approval.to_link()?;
            (Answer::Approved(approval), Some(link))
        } else {
            (Answer::Declined { guardian: root.id(), sig: root.0.sign(&decline_bytes(&request)) }, None)
        };
        let Some(addr) = addr else {
            return Ok(RecoveryAnswerView {
                approval_link,
                delivered: false,
                note: Some("The link does not name their device. Send them the approval link.".into()),
            });
        };
        let invites = if approve && send_invites { self.invites_for(&request.old_root).await } else { Vec::new() };
        let set = self.load_guarded().remove(&request.old_root);
        let message = Request::RecoveryAnswer(Box::new(RecoveryAnswer { request, answer, set, invites }));
        if let Err(e) = self.node().remember(&addr) {
            warn!(error = %e, "could not remember the recovering device");
        }
        let (delivered, note) = match self.call(addr.id, &message).await {
            Ok(Response::Ack) => (true, None),
            Ok(Response::Refused(reason)) => (false, Some(format!("Their device refused it: {reason}"))),
            Ok(other) => (false, Some(format!("Unexpected answer: {other:?}"))),
            Err(e) => (false, Some(format!("Their device could not be reached ({e}). Send them the approval link."))),
        };
        Ok(RecoveryAnswerView { approval_link, delivered, note })
    }

    /// Invites to the servers both we and `root` are in, made now.
    async fn invites_for(&self, root: &RootId) -> Vec<String> {
        let ids: Vec<dissent_identity::CommunityId> =
            self.read().communities.iter().filter(|(_, c)| c.is_member(root)).map(|(id, _)| *id).collect();
        let mut out = Vec::new();
        for id in ids.into_iter().take(MAX_INVITES) {
            match self.create_invite(id).await {
                Ok(invite) => out.push(invite),
                Err(e) => debug!(community = %id, error = %e, "no invite for a recovering member"),
            }
        }
        out
    }
}

fn add_approval(p: &mut PendingRecovery, approval: GuardianApproval) {
    p.declined.retain(|g| *g != approval.guardian);
    if !p.approvals.iter().any(|a| a.guardian == approval.guardian) {
        p.approvals.push(approval);
    }
}

fn add_invites(p: &mut PendingRecovery, invites: impl Iterator<Item = String>) {
    for invite in invites.filter(|i| i.starts_with("dissent://invite/")) {
        if p.invites.len() < MAX_INVITES && !p.invites.contains(&invite) {
            p.invites.push(invite);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    #[test]
    fn the_code_is_nine_digits_the_same_on_both_sides_and_changes_with_the_request() {
        let new = RootKey::generate();
        let old = RootKey::generate().id();
        let request = RotationRequest::new_for(&new, old, 1);
        let code = code_of(&request);
        assert_eq!(code.len(), 11);
        assert_eq!(digits(&code).len(), 9);
        assert_eq!(code, code_of(&request));
        assert_ne!(code, code_of(&RotationRequest::new_for(&new, old, 2)));
    }

    #[test]
    fn a_request_link_carries_the_device_address_and_parses_without_it() {
        let request = RotationRequest::new_for(&RootKey::generate(), RootKey::generate().id(), 5);
        let base = request.to_link().unwrap();
        let (plain, none) = parse_request_link(&base).unwrap();
        assert_eq!((plain, none), (request, None));
        let addr = dissent_net::PeerAddr { id: dissent_identity::DeviceKey::generate().id(), relay: None, direct: vec![] };
        let via = data_encoding::BASE32_NOPAD.encode(&postcard::to_allocvec(&addr).unwrap()).to_lowercase();
        let (parsed, found) = parse_request_link(&format!("{base}{VIA}{via}")).unwrap();
        assert_eq!(parsed, request);
        assert_eq!(found, Some(addr));
    }

    #[test]
    fn a_decline_verifies_only_for_its_request_and_guardian() {
        let guardian = RootKey::generate();
        let request = RotationRequest::new_for(&RootKey::generate(), RootKey::generate().id(), 1);
        let sig = guardian.0.sign(&decline_bytes(&request));
        assert!(guardian.id().verify(&decline_bytes(&request), &sig).is_ok());
        let other = RotationRequest { requested_ms: 2, ..request };
        assert!(guardian.id().verify(&decline_bytes(&other), &sig).is_err());
        assert!(RootKey::generate().id().verify(&decline_bytes(&request), &sig).is_err());
    }
}
