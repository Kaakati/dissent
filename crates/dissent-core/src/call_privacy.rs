//! "Who can call you": everyone, friends only, or no one. Enforced by the device being called, in the
//! engine, before anything rings: a refused DM or group call raises no ringing, notification or sound, and
//! creates no room, key, link or address for the caller. The caller is answered exactly as for a call
//! nobody picks up: `Ringing`, then `Decline` after the ring timeout. A refusal by this setting leaves a
//! quiet entry in that conversation, so the user can add the person. Voice channels are not calls to
//! someone and are not covered. See `04-calls.md`, "Who can call you".
//!
//! The setting is per device, like the DM policy: each device a person uses enforces its own.

use dissent_identity::{DeviceId, RootId};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::calls::{lock, CallMsg, CallReply, RoomId, Scope};
use crate::engine::{now_ms, Engine};
use crate::error::Result;
use crate::groups::{dm_conv_id, ConvId};
use crate::views::{DmItem, EngineEvent};

/// Who may ring this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPolicy {
    /// Anyone whose call reaches this device, as before the setting existed.
    #[default]
    Everyone,
    /// Only contacts.
    Friends,
    /// Nobody.
    NoOne,
}

/// Why a call left a missed entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NoteReason {
    /// The caller is not a contact, and calls are for friends only.
    NotAFriend,
    /// Calls are off.
    CallsOff,
}

/// What this device does with an incoming DM or group call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Ring, as ever.
    Ring,
    /// Answer as an unanswered call, and leave a missed entry for this reason.
    Quiet(NoteReason),
    /// Answer as an unanswered call and leave nothing: a blocked caller, or one the DM policy keeps out.
    Silent,
}

/// The decision, from the facts about the caller. `may_message` is the DM policy's answer, which already
/// covers blocking.
pub(crate) fn verdict(policy: CallPolicy, blocked: bool, contact: bool, may_message: bool) -> Verdict {
    if blocked || !may_message {
        return Verdict::Silent;
    }
    match policy {
        CallPolicy::Everyone => Verdict::Ring,
        CallPolicy::Friends if contact => Verdict::Ring,
        CallPolicy::Friends => Verdict::Quiet(NoteReason::NotAFriend),
        CallPolicy::NoOne => Verdict::Quiet(NoteReason::CallsOff),
    }
}

/// A missed entry: someone called and this device's setting turned the call away.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CallNote {
    pub conv: ConvId,
    pub from: RootId,
    pub ms: u64,
    pub reason: NoteReason,
}

/// Missed entries kept, newest last. Older ones drop off.
const MAX_NOTES: usize = 500;

impl Engine {
    /// Who may call this device.
    pub fn call_policy(&self) -> CallPolicy {
        let paths = &self.read().paths;
        paths.read_bin::<CallPolicy>(&paths.call_privacy()).ok().flatten().unwrap_or_default()
    }

    /// Changes who may call this device. Takes effect for the next call.
    pub fn set_call_policy(&self, policy: CallPolicy) -> Result<()> {
        let guard = self.read();
        guard.paths.write_bin(&guard.paths.call_privacy(), &policy)
    }

    /// What to do with a call from `root`. Only DM calls and rung group calls are checked; a voice channel is
    /// not a call to someone.
    pub(crate) fn call_verdict(&self, scope: &Scope, root: RootId, ring: bool) -> Verdict {
        let ringing_call = matches!(scope, Scope::Dm) || (matches!(scope, Scope::Group { .. }) && ring);
        if !ringing_call {
            return Verdict::Ring;
        }
        let policy = self.call_policy();
        let w = self.read();
        let blocked = w.contact_book.blocked.contains(&root);
        let contact = w.is_contact(&root);
        // A group member may call the group whatever the DM policy; a DM call is a direct message.
        let may_message = match scope {
            Scope::Dm => crate::contacts::dm_allowed(&w, &root),
            _ => true,
        };
        verdict(policy, blocked, contact, may_message)
    }

    /// Answers a refused call as a call nobody picks up: `Ringing` now, and `Decline` after the ring
    /// timeout, unless the caller gives up first. Busy answers match a ringing call's. Nothing rings and no
    /// room, key, link or address is kept. `note` leaves a missed entry in the conversation.
    pub(crate) fn ring_quietly(&self, id: RoomId, from: DeviceId, scope: Scope, caller: RootId, note: Option<NoteReason>) -> CallReply {
        {
            let ringing = lock(&self.inner.calls.ringing);
            if ringing.as_ref().is_some_and(|r| r.id != id) {
                return CallReply::Refused("busy with another incoming call".into());
            }
        }
        let first = lock(&self.inner.calls.quiet).insert(id, from).is_none();
        if !first {
            return CallReply::Ringing;
        }
        let timeout = *lock(&self.inner.calls.ring_timeout);
        let engine = self.clone();
        self.spawn_task(async move {
            if let Some(reason) = note {
                engine.add_call_note(scope, caller, reason);
            }
            tokio::time::sleep(timeout).await;
            let still = lock(&engine.inner.calls.quiet).remove(&id).is_some();
            if still {
                if let Err(e) = engine.call_rpc(from, CallMsg::Decline { room: id }).await {
                    debug!(device = %from, error = %e, "missed call not reported to caller");
                }
            }
        });
        debug!(device = %from, "call answered as unanswered by the call setting");
        CallReply::Ringing
    }

    /// The caller of a quietly answered call gave up. Returns whether `id` was one.
    pub(crate) fn end_quiet_ring(&self, id: &RoomId, from: &DeviceId) -> bool {
        let mut quiet = lock(&self.inner.calls.quiet);
        if quiet.get(id) == Some(from) {
            quiet.remove(id);
            true
        } else {
            false
        }
    }

    fn add_call_note(&self, scope: Scope, caller: RootId, reason: NoteReason) {
        let conv = match scope {
            Scope::Group { conv } => conv,
            _ => match self.ensure_dm(caller) {
                Ok(conv) => conv,
                Err(e) => {
                    warn!(error = %e, "no conversation for a missed call entry");
                    dm_conv_id(&self.inner.me, &caller)
                }
            },
        };
        {
            let guard = self.read();
            let paths = &guard.paths;
            let mut notes: Vec<CallNote> = paths.read_bin(&paths.call_notes()).ok().flatten().unwrap_or_default();
            notes.push(CallNote { conv, from: caller, ms: now_ms(), reason });
            let excess = notes.len().saturating_sub(MAX_NOTES);
            notes.drain(..excess);
            if let Err(e) = paths.write_bin(&paths.call_notes(), &notes) {
                warn!(error = %e, "could not keep a missed call entry");
            }
        }
        // A quiet change: the view re-reads the conversation, and nothing alerts or sounds.
        let event = match scope {
            Scope::Group { conv } => EngineEvent::ConversationChanged { id: conv.to_string() },
            _ => EngineEvent::DmChanged { peer: caller.to_string() },
        };
        self.emit(event);
    }

    /// The missed entries of `conv`, as conversation items.
    pub(crate) fn call_note_items(&self, conv: ConvId) -> Vec<DmItem> {
        let guard = self.read();
        let paths = &guard.paths;
        let notes: Vec<CallNote> = paths.read_bin(&paths.call_notes()).ok().flatten().unwrap_or_default();
        notes
            .into_iter()
            .filter(|n| n.conv == conv)
            .map(|n| {
                let name = guard.display_name(&n.from);
                let why = match n.reason {
                    NoteReason::NotAFriend => "not in your friends",
                    NoteReason::CallsOff => "you are not taking calls",
                };
                note_item(&n, format!("Missed call from {name}: {why}"))
            })
            .collect()
    }
}

fn note_item(n: &CallNote, notice: String) -> DmItem {
    DmItem {
        id: format!("call-note-{}-{}", n.ms, n.from),
        from: n.from.to_string(),
        text: String::new(),
        ms: n.ms,
        pending: false,
        delivery: "delivered".into(),
        queued: false,
        reason: None,
        retryable: false,
        attachments: Vec::new(),
        embeds: Vec::new(),
        reply_to: None,
        reactions: Vec::new(),
        edited: false,
        deleted: false,
        mentions_me: false,
        mentions: Vec::new(),
        notice: Some(notice),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every setting against a contact, a stranger the DM policy lets in, a blocked caller and one the DM policy
    /// keeps out.
    #[test]
    fn each_setting_rings_only_whom_it_should() {
        use NoteReason::*;
        use Verdict::*;
        let cases = [
            // policy, blocked, contact, may_message, verdict
            (CallPolicy::Everyone, false, true, true, Ring),
            (CallPolicy::Everyone, false, false, true, Ring),
            (CallPolicy::Everyone, true, true, false, Silent),
            (CallPolicy::Everyone, false, false, false, Silent),
            (CallPolicy::Friends, false, true, true, Ring),
            (CallPolicy::Friends, false, false, true, Quiet(NotAFriend)),
            (CallPolicy::Friends, true, true, false, Silent),
            (CallPolicy::Friends, false, false, false, Silent),
            (CallPolicy::NoOne, false, true, true, Quiet(CallsOff)),
            (CallPolicy::NoOne, false, false, true, Quiet(CallsOff)),
            (CallPolicy::NoOne, true, false, false, Silent),
        ];
        for (policy, blocked, contact, may_message, want) in cases {
            assert_eq!(verdict(policy, blocked, contact, may_message), want, "{policy:?} blocked={blocked} contact={contact} may_message={may_message}");
        }
    }

    #[test]
    fn the_setting_is_spelled_for_the_window() {
        assert_eq!(serde_json::to_string(&CallPolicy::NoOne).unwrap(), "\"no_one\"");
        assert_eq!(serde_json::to_string(&CallPolicy::Friends).unwrap(), "\"friends\"");
        assert_eq!(CallPolicy::default(), CallPolicy::Everyone);
    }
}
