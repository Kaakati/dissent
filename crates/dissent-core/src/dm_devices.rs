//! Multi-device direct conversations. Every device of each party must be in the conversation's MLS group.
//! This module adds the devices linked after the group was made, and removes the revoked ones. The design is in
//! `docs/design/10-dm-delivery.md`, "Multi-device conversations".
//!
//! Only the creator's device changes the group, since two devices committing at once would fork it. A device is
//! added only when it answers `DmDevicesSupport(true)`, so an older peer keeps the devices it has.

use std::collections::BTreeSet;
use std::time::Duration;

use dissent_identity::DeviceId;
use tracing::{debug, warn};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::groups::{ConvId, ConvKind, ConvMeta, ConvPacket};
use crate::rpc::{Request, Response};

const ASK_WAIT: Duration = Duration::from_secs(5);

impl Engine {
    /// Whether `device` can be added to a direct conversation after it was made. An older device, or one that does
    /// not answer, counts as no.
    pub(crate) async fn dm_device_supports(&self, device: DeviceId) -> bool {
        matches!(
            tokio::time::timeout(ASK_WAIT, self.call(device, &Request::DmDevicesSupport)).await,
            Ok(Ok(Response::DmDevicesSupport(true)))
        )
    }

    /// Whether any community has revoked `device`.
    pub(crate) fn dm_device_revoked(&self, device: DeviceId) -> bool {
        self.read()
            .communities
            .values()
            .any(|c| c.state().revoked_devices.contains_key(&device))
    }

    /// The devices of ours that a conversation should have: every device we have certified, except inbox devices,
    /// revoked ones, and this device.
    fn own_wanted_devices(&self) -> BTreeSet<DeviceId> {
        let me = self.inner.identity.device_id();
        let inboxes: BTreeSet<DeviceId> = self
            .read()
            .contact_book
            .known_inboxes
            .values()
            .flatten()
            .copied()
            .collect();
        self.load_devices()
            .map(|records| records.into_iter().map(|r| r.cert.device).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| *d != me && !inboxes.contains(d) && !self.dm_device_revoked(*d))
            .collect()
    }

    /// Brings one direct conversation's devices of ours in line. Adds our certified devices that are missing and
    /// support it, and removes our revoked devices that are still in the group. Either party does this for its own
    /// devices. The party with the higher root waits one pass when the epoch has moved, so the two sides do not
    /// commit at the same epoch. A collision would split the group, since an accepted commit cannot be undone.
    pub(crate) async fn sync_dm_devices(&self, conv: ConvId) -> Result<()> {
        let meta = self.meta_of(conv)?;
        if meta.kind != ConvKind::Dm || !meta.ready || meta.left || !meta.members.contains(&self.inner.me) {
            return Ok(());
        }
        let me = self.inner.identity.device_id();
        let my_root = self.inner.me;
        let members = self.mls_members(conv);
        let current: BTreeSet<DeviceId> = members.iter().map(|c| c.device).collect();
        let removable: Vec<DeviceId> = members
            .iter()
            .filter(|c| c.root == my_root && c.device != me && self.dm_device_revoked(c.device))
            .map(|c| c.device)
            .collect();
        let wanted = self.own_wanted_devices();
        let mut missing = Vec::new();
        for device in wanted.difference(&current) {
            if self.dm_device_supports(*device).await {
                missing.push(*device);
            } else {
                debug!(conv = %conv, device = %device, "a device without multi-device support is not added");
            }
        }
        if missing.is_empty() && removable.is_empty() {
            return Ok(());
        }
        if self.dm_must_yield(conv, &meta)? {
            return Ok(());
        }
        if !missing.is_empty() {
            self.add_dm_devices(conv, &meta, &missing).await?;
        }
        if !removable.is_empty() {
            self.remove_dm_devices(conv, &meta, &removable).await?;
        }
        Ok(())
    }

    /// Whether this device, having the higher root of the two, waits: it records the epoch and yields when the epoch
    /// has moved since its last pass that wanted a change. The lower root never yields.
    fn dm_must_yield(&self, conv: ConvId, meta: &ConvMeta) -> Result<bool> {
        let other = meta.members.iter().copied().find(|m| *m != self.inner.me);
        let Some(other) = other else {
            return Ok(false);
        };
        if self.inner.me < other {
            return Ok(false);
        }
        let epoch = self.with_mls_read(|mls| mls.epoch(&conv.0)).map_err(crate::sealed::mls_err)?;
        let mut seen = self.inner.dm_sync_seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.get(&conv) == Some(&epoch) {
            return Ok(false);
        }
        seen.insert(conv, epoch);
        Ok(true)
    }

    /// Adds `devices` to the group, one key package each, with one commit. The other devices get the commit, and
    /// the new ones get the welcome.
    async fn add_dm_devices(&self, conv: ConvId, meta: &ConvMeta, devices: &[DeviceId]) -> Result<()> {
        let mut key_packages = Vec::new();
        for device in devices {
            match self.call(*device, &Request::DmKeyPackage).await {
                Ok(Response::KeyPackages(found)) => key_packages.extend(found.into_iter().take(1)),
                _ => warn!(conv = %conv, device = %device, "no key package from a device to add"),
            }
        }
        if key_packages.is_empty() {
            return Ok(());
        }
        let before: BTreeSet<DeviceId> = self.mls_device_set(conv);
        let commit = self
            .with_mls(|mls| mls.add_members(&conv.0, &key_packages))
            .map_err(crate::sealed::mls_err)?;
        let after = self.mls_members(conv);
        let me = self.inner.identity.device_id();
        let existing: Vec<(DeviceId, dissent_identity::RootId)> = after
            .iter()
            .filter(|c| before.contains(&c.device) && c.device != me)
            .map(|c| (c.device, c.root))
            .collect();
        let added: Vec<(DeviceId, dissent_identity::RootId)> = after
            .iter()
            .filter(|c| !before.contains(&c.device))
            .map(|c| (c.device, c.root))
            .collect();
        let updated = meta.clone();
        {
            let mut guard = self.write();
            Self::enqueue(&mut guard, &existing, &ConvPacket::Commit { conv, commit: commit.commit });
            self.queue_welcome(&mut guard, &updated, commit.welcome, &added)?;
        }
        self.flush_conv_queues().await;
        self.notify_conv(&updated);
        Ok(())
    }

    /// Removes revoked `devices` from the group, with one commit that rotates the epoch.
    async fn remove_dm_devices(&self, conv: ConvId, meta: &ConvMeta, devices: &[DeviceId]) -> Result<()> {
        self.remove_devices(conv, meta, devices).await?;
        Ok(())
    }

    /// Both parties sync each direct conversation's devices. Runs from maintenance, so a device linked or revoked is
    /// handled within a few minutes.
    pub async fn sync_all_dm_devices(&self) {
        let convs: Vec<ConvId> = self
            .read()
            .conv_book
            .convs
            .values()
            .filter(|m| m.kind == ConvKind::Dm && m.ready && !m.left)
            .map(|m| m.id)
            .collect();
        for conv in convs {
            if let Err(e) = self.sync_dm_devices(conv).await {
                debug!(conv = %conv, error = %e, "could not sync the devices of a conversation");
            }
        }
    }

    /// The devices in the conversation's MLS group, us included.
    fn mls_device_set(&self, conv: ConvId) -> BTreeSet<DeviceId> {
        self.mls_members(conv).iter().map(|c| c.device).collect()
    }

    /// The conversation's metadata, or `NotFound`.
    fn meta_of(&self, conv: ConvId) -> Result<ConvMeta> {
        self.read()
            .conv_book
            .convs
            .get(&conv)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("conversation {conv}")))
    }
}
