//! Sealed message text. A channel message's text is encrypted under the community's MLS group.
//! The DAG keeps the kind, the target and the epoch in clear, so permissions need no key. Each
//! device recovers plaintext locally and keeps it in its content view and `content.log`.
//! See `docs/design/05-encrypted-servers.md`.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::Duration;

use dissent_events::{EventId, Hlc, Payload, SealedKind, SignedEvent};
use dissent_identity::{CommunityId, DeviceId};
use dissent_mls::{MlsClient, MlsError};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::store::{append_sealed_records, content_vault, write_bytes_atomic, write_bytes_unsynced};
use crate::world::ContentRecord;

/// Ratchet steps before the MLS state is flushed regardless of time.
const FLUSH_EVERY_STEPS: u64 = 100;
/// Longest time ratchet steps may stay unflushed while the device runs.
const FLUSH_EVERY: Duration = Duration::from_secs(1);

pub(crate) fn mls_err(e: MlsError) -> CoreError {
    CoreError::Invalid(format!("encryption: {e}"))
}

impl Engine {
    /// Runs an MLS operation that changes group membership, an epoch, or the key package pool.
    /// The state is written at once. Never call this while holding the world lock.
    pub(crate) fn with_mls<T>(
        &self,
        op: impl FnOnce(&mut MlsClient) -> dissent_mls::Result<T>,
    ) -> dissent_mls::Result<T> {
        let mut mls = self
            .inner
            .mls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let out = op(&mut mls)?;
        let bytes = mls.snapshot_sealed(&self.inner.vault)?;
        drop(mls);
        self.write_mls_snapshot(&bytes, true);
        Ok(out)
    }

    /// Runs a ratchet step: encrypting or decrypting one application message. The state is
    /// flushed at most once a second, or every `FLUSH_EVERY_STEPS` steps, and on shutdown.
    pub(crate) fn with_mls_ratchet<T>(
        &self,
        op: impl FnOnce(&mut MlsClient) -> dissent_mls::Result<T>,
    ) -> dissent_mls::Result<T> {
        let out = {
            let mut mls = self
                .inner
                .mls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            op(&mut mls)?
        };
        let steps = self.inner.mls_dirty.fetch_add(1, Ordering::Relaxed) + 1;
        let due = self
            .inner
            .mls_flushed_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
            >= FLUSH_EVERY;
        if steps >= FLUSH_EVERY_STEPS || due {
            self.flush_mls(false);
        }
        Ok(out)
    }

    /// Writes the MLS state if any ratchet steps are unflushed. Called on shutdown and by
    /// maintenance, so a quiet device still persists its last steps.
    pub(crate) fn flush_mls(&self, durable: bool) {
        if self.inner.mls_dirty.load(Ordering::Relaxed) == 0 {
            return;
        }
        let bytes = {
            let mls = self
                .inner
                .mls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            mls.snapshot_sealed(&self.inner.vault)
        };
        match bytes {
            Ok(bytes) => self.write_mls_snapshot(&bytes, durable),
            Err(e) => tracing::warn!(error = %e, "could not snapshot MLS state"),
        }
    }

    fn write_mls_snapshot(&self, bytes: &[u8], durable: bool) {
        let written = if durable {
            write_bytes_atomic(&self.inner.mls_path, bytes)
        } else {
            write_bytes_unsynced(&self.inner.mls_path, bytes)
        };
        if let Err(e) = written {
            tracing::warn!(error = %e, "could not persist MLS state");
            return;
        }
        self.inner.mls_dirty.store(0, Ordering::Relaxed);
        *self
            .inner
            .mls_flushed_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = std::time::Instant::now();
    }

    /// Reads the MLS state without saving it.
    pub(crate) fn with_mls_read<T>(&self, op: impl FnOnce(&MlsClient) -> T) -> T {
        let mls = self
            .inner
            .mls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        op(&mls)
    }

    /// Current epoch of a community's group, if this device is in it.
    /// The MLS epoch this device is in for `community`. None when it has no group, as for a storage
    /// member. A removal commit moves it, so a test can wait for one to land.
    pub fn mls_epoch(&self, community: CommunityId) -> Option<u64> {
        let mls = self
            .inner
            .mls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        mls.epoch(&community.0).ok()
    }

    /// Encrypts `text` for a community's group. Returns the epoch and the ciphertext. Written to
    /// disk at once: a crash that lost the sender ratchet could reuse a key for the next message.
    pub(crate) fn seal(&self, community: CommunityId, text: &str) -> Result<(u64, Vec<u8>)> {
        let plaintext = postcard::to_allocvec(&text.to_string())
            .map_err(|e| CoreError::Encoding(e.to_string()))?;
        self.with_mls(|mls| {
            let epoch = mls.epoch(&community.0)?;
            let ciphertext = mls.encrypt(&community.0, &plaintext)?;
            Ok((epoch, ciphertext))
        })
        .map_err(mls_err)
    }

    /// Records decrypted text for a content key, unless we already hold newer text for it.
    pub(crate) fn store_content(
        &self,
        community: CommunityId,
        key: EventId,
        hlc: Hlc,
        text: String,
    ) {
        let mut w = self.write();
        let paths = w.paths.clone();
        let Some(c) = w.communities.get_mut(&community) else {
            return;
        };
        if c.content.get(&key).is_some_and(|(have, _)| *have >= hlc) {
            return;
        }
        c.content.insert(key, (hlc, text.clone()));
        let record = ContentRecord { key, hlc, text };
        let key = content_vault(&self.inner.vault, &community);
        if let Err(e) = append_sealed_records(&paths.content_log(&community), &key, &[record]) {
            tracing::warn!(community = %community, error = %e, "could not persist decrypted text");
        }
    }

    /// Decrypts every sealed event this device can read and does not yet hold text for.
    ///
    /// A message from a later epoch than ours means we missed a commit. Then we ask its author for
    /// the commits, and retry once, in the background. Older epochs are not retried: the keys for
    /// them are gone by design.
    pub(crate) fn unseal_pending(&self, community: CommunityId) {
        if self.secret_behind(community) {
            self.spawn_secret_refresh(community);
        }
        self.unseal_pass(community, true);
    }

    fn unseal_pass(&self, community: CommunityId, may_catch_up: bool) {
        let own_device = self.inner.identity.device_id();
        let our_epoch = self.mls_epoch(community);
        let candidates: Vec<SignedEvent> = {
            let w = self.read();
            let Some(c) = w.communities.get(&community) else {
                return;
            };
            c.sealed_in_order()
                .into_iter()
                .filter(|se| {
                    let Some((key, hlc)) = content_key(se) else {
                        return false;
                    };
                    se.event.body.device != own_device
                        && c.content.get(&key).is_none_or(|(have, _)| *have < hlc)
                })
                .collect()
        };
        for se in candidates {
            let Payload::Sealed {
                epoch,
                ciphertext,
                kind,
            } = &se.event.body.payload
            else {
                continue;
            };
            let (manifest, mls_ciphertext) = crate::attachments::split_manifest(ciphertext);
            if !manifest.is_empty() {
                self.mirror_manifest(se.event.body.device, &manifest);
            }
            let history = matches!(kind, SealedKind::HistoryMessage);
            let outcome = if history {
                self.open_history_text(community, *epoch, mls_ciphertext)
                    .map(|bytes| (bytes, se.event.body.device))
                    .ok_or(())
            } else {
                self.with_mls_ratchet(|mls| mls.decrypt_application(&community.0, mls_ciphertext))
                    .map_err(|_| ())
            };
            let (bytes, device) = match outcome {
                Ok((bytes, device)) => (bytes, device),
                _ => {
                    if may_catch_up && !history && our_epoch.is_some_and(|ours| *epoch > ours) {
                        self.catch_up_in_background(community, se.event.body.device);
                    }
                    continue;
                }
            };
            if device != se.event.body.device {
                continue;
            }
            let Ok(text) = postcard::from_bytes::<String>(&bytes) else {
                continue;
            };
            // The cleartext manifest must describe exactly the files in the sealed text. Otherwise
            // the sender advertised a different blob, and the message is refused.
            let (_, files) = crate::attachments::unpack(&text);
            if !crate::attachments::manifest_matches(&manifest, &files) {
                tracing::warn!(community = %community, "refused a message whose manifest does not match");
                continue;
            }
            if let Some((key, hlc)) = content_key(&se) {
                let for_hook = text.clone();
            self.store_content(community, key, hlc, text);
            self.auto_download(Some(community), key.to_string(), &for_hook);
            }
        }
    }

    /// Fetches missed commits from `device` and retries decryption once. Only one request per
    /// device and community is in flight at a time.
    fn catch_up_in_background(&self, community: CommunityId, device: DeviceId) {
        let fresh = self
            .inner
            .mls_catching_up
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((community, device));
        if !fresh {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            engine.mls_catch_up_commits(community, device).await;
            engine.unseal_pass(community, false);
            engine
                .inner
                .mls_catching_up
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&(community, device));
        });
    }
}

/// The content key a sealed event writes under, and the hlc that orders it. A message or reply
/// keys by its own id. An edit keys by its target.
pub(crate) fn content_key(se: &SignedEvent) -> Option<(EventId, Hlc)> {
    let hlc = se.event.body.hlc;
    match &se.event.body.payload {
        Payload::Sealed { kind, .. } => match kind {
            SealedKind::Message | SealedKind::HistoryMessage | SealedKind::Reply { .. } => Some((se.event.id, hlc)),
            SealedKind::React { .. } | SealedKind::Unreact { .. } => Some((se.event.id, hlc)),
            SealedKind::Edit { target } => Some((*target, hlc)),
        },
        _ => None,
    }
}
