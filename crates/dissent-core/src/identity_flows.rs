//! Identity flows on top of `dissent-identity`: guardian recovery, and linking or revoking the
//! devices of one root. See `docs/design/03-recovery-and-safety.md`.
//!
//! Recovery and device linking need a root key, so the engine methods here fail on a device
//! that holds none. A new device that is still waiting for its grant has no engine yet, so it
//! uses [`start_device_link`] and [`PendingDevice::finish`] instead.

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};

use crate::rpc::{Request, Response};
use crate::views::EngineEvent;
use crate::store::{read_bin, write_bin};
use dissent_events::{ChannelId, Payload};
use dissent_identity::{
    CommunityId, DeviceCert, DeviceId, DeviceKey, GuardianApproval, GuardianSet, Identity,
    KeyRotation, LinkGrant, LinkRequest, RootId, RootKey, RotationRequest,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// A device of our root, as recorded when it was linked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DeviceRecord {
    pub(crate) cert: DeviceCert,
    name: String,
    /// When this device was linked, in milliseconds.
    added_ms: u64,
    /// When this device last reached us. Set when the device is linked.
    last_seen_ms: u64,
}

/// Our guardians, in the shape the Recovery settings read. `docs/design/07-visual-system.md`
/// part 6.5.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct GuardianView {
    /// False until guardians are set. The list is then empty and the threshold zero.
    pub set: bool,
    pub threshold: u8,
    pub guardians: Vec<GuardianEntry>,
    /// `dissent://guardians/...` link, for the recovering device. `None` until guardians are set.
    pub share_link: Option<String>,
}

/// One guardian.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct GuardianEntry {
    /// Base32 root id.
    pub root_id: String,
    pub display_name: String,
    /// Short suffix of the root id, for telling people with one name apart.
    pub suffix: String,
    pub is_contact: bool,
    /// Names of the communities that both we and this guardian belong to.
    pub shared_servers: Vec<String>,
}

/// One device of our root, in the shape the Devices settings read. `docs/design/07-visual-system.md`
/// part 6.6.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DeviceView {
    pub device_id: String,
    pub name: String,
    pub this_device: bool,
    pub added_ms: u64,
    pub last_seen_ms: u64,
}

/// A link request made on a new device that has no identity yet. Keep it until the grant comes
/// back, then call [`PendingDevice::finish`]. It holds the new device's secret key.
pub struct PendingDevice {
    device: DeviceKey,
    request: LinkRequest,
}

/// Starts linking a new device named `name`, such as "Laptop". Returns the pending request.
pub fn start_device_link(name: &str) -> Result<PendingDevice> {
    let device = DeviceKey::generate();
    let request = LinkRequest::new(&device, name, now_ms())?;
    Ok(PendingDevice { device, request })
}

impl PendingDevice {
    /// `dissent://link/...`. Show it on a device that holds the root key.
    pub fn link(&self) -> Result<String> {
        Ok(self.request.to_link()?)
    }

    /// Checks the grant against this request and returns the linked identity. That identity
    /// has no root key, so it can chat but cannot link or revoke devices.
    pub fn finish(self, grant_link: &str) -> Result<Identity> {
        let grant = LinkGrant::parse(grant_link)?;
        Ok(Identity::from_grant(&self.request, self.device, grant)?)
    }
}

impl Engine {
    fn root_key(&self) -> Result<&RootKey> {
        self.inner
            .identity
            .root
            .as_ref()
            .ok_or_else(|| CoreError::Invalid("this device does not hold the root key".into()))
    }

    pub(crate) fn load_devices(&self) -> Result<Vec<DeviceRecord>> {
        let path = self.read().paths.devices();
        Ok(read_bin(&path)?.unwrap_or_default())
    }

    fn save_devices(&self, devices: &[DeviceRecord]) -> Result<()> {
        let path = self.read().paths.devices();
        write_bin(&path, &devices.to_vec())
    }

    fn load_guardians(&self) -> Result<Option<GuardianSet>> {
        let path = self.read().paths.guardians();
        read_bin(&path)
    }

    /// Our guardians. Before any are set, `set` is false.
    pub fn guardians(&self) -> Result<GuardianView> {
        match self.load_guardians()? {
            Some(set) => self.guardian_view(&set),
            None => Ok(GuardianView {
                set: false,
                threshold: 0,
                guardians: Vec::new(),
                share_link: None,
            }),
        }
    }

    fn guardian_view(&self, set: &GuardianSet) -> Result<GuardianView> {
        let w = self.read();
        let guardians = set
            .guardians
            .iter()
            .map(|root| GuardianEntry {
                root_id: root.to_string(),
                display_name: w.display_name(root),
                suffix: root.suffix(),
                is_contact: w.is_contact(root),
                shared_servers: w
                    .communities
                    .values()
                    .filter(|c| c.is_member(root))
                    .map(|c| c.name().to_string())
                    .collect(),
            })
            .collect();
        Ok(GuardianView {
            set: true,
            threshold: set.threshold,
            guardians,
            share_link: Some(set.to_link()?),
        })
    }

    /// Signs a guardian list with the root key, saves it, and shares it with our other devices.
    pub async fn set_guardians(&self, guardians: Vec<RootId>, threshold: u8) -> Result<GuardianView> {
        let set = GuardianSet::create(self.root_key()?, guardians, threshold, now_ms())?;
        let path = self.read().paths.guardians();
        write_bin(&path, &set)?;
        self.share_guardians(&set).await;
        self.guardian_view(&set)
    }

    /// Sends the list to every other device of this root that we know. A device that is offline
    /// misses it, and gets it the next time the list is set.
    async fn share_guardians(&self, set: &GuardianSet) {
        let me = self.inner.identity.device_id();
        let devices = match self.load_devices() {
            Ok(devices) => devices,
            Err(e) => {
                warn!(error = %e, "could not read linked devices; guardians not shared");
                return;
            }
        };
        let bytes = match self.request_bytes(&Request::Guardians(Box::new(set.clone()))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode guardians");
                return;
            }
        };
        for record in devices.iter().filter(|d| d.cert.device != me) {
            if let Err(e) = self.node().request(record.cert.device, bytes.clone()).await {
                warn!(device = %record.cert.device, error = %e, "guardians not shared with device");
            }
        }
    }

    /// Stores a guardian list that another device of our root sent us. Only a root-signed list
    /// for our own root is accepted.
    pub(crate) fn accept_guardians(&self, set: GuardianSet) -> Response {
        let ours = self.inner.identity.cert.root;
        if set.owner != ours || set.verify().is_err() {
            return Response::Error("guardian list is not signed by our root".into());
        }
        let path = self.read().paths.guardians();
        match write_bin(&path, &set) {
            Ok(()) => Response::Ack,
            Err(e) => Response::Error(format!("could not store guardians: {e}")),
        }
    }

    /// The `dissent://recover/...` link a new device shows. It names the lost root and this
    /// device's new root. Call it on the new device, which holds the new root.
    pub fn start_recovery(&self, old_root: RootId) -> Result<String> {
        let new_root = self.root_key()?;
        Ok(RotationRequest::new_for(new_root, old_root, now_ms()).to_link()?)
    }

    /// Run by a guardian of the lost root. Signs the recovery request and returns the
    /// `dissent://approve/...` link to send back.
    pub fn approve_recovery(&self, request_link: &str) -> Result<String> {
        let request = RotationRequest::parse(request_link)?;
        let approval = GuardianApproval::sign(self.root_key()?, request);
        Ok(approval.to_link()?)
    }

    /// Run on the new device once the threshold of approvals is in.
    ///
    /// Order matters. The device first syncs each community from its invite without announcing
    /// a Join, because a Join would make the new root known and the rotation would be refused.
    /// Then it publishes the rotation in every community it holds. Only then does it announce
    /// its Join, as the rotated identity. Returns how many communities took the rotation.
    ///
    /// `guardian_set_link` is the `dissent://guardians/...` link from the owner's
    /// [`GuardianView::share_link`]. The rotation needs the signed list to prove the threshold.
    /// `invites` are invites to the communities the lost root belonged to.
    pub async fn complete_recovery(
        &self,
        approval_links: &[String],
        guardian_set_link: &str,
        invites: &[String],
    ) -> Result<usize> {
        let approvals = approval_links
            .iter()
            .map(|l| GuardianApproval::parse(l))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let request = match approvals.first() {
            Some(first) => first.request,
            None => return Err(CoreError::Invalid("no approvals given".into())),
        };
        let set = GuardianSet::parse(guardian_set_link)?;
        let rotation = KeyRotation::by_guardians(self.root_key()?, set, request, approvals)?;
        rotation.verify()?;

        // 1. Sync, silently.
        let mut synced = Vec::new();
        for invite in invites {
            if let Some(found) = self.sync_invite(invite).await? {
                synced.push(found);
            }
        }
        // 2. Rotate in every community held.
        let ids: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        let mut published = 0;
        for id in ids {
            match self
                .author_publish(id, ChannelId::CONTROL, Payload::KeyRotation(rotation.clone()))
                .await
            {
                Ok(_) => published += 1,
                Err(e) => warn!(community = %id, error = %e, "key rotation not accepted"),
            }
        }
        // 3. Join as the rotated identity.
        for (id, served) in synced {
            self.announce_join(id, &served, true).await;
            self.emit(EngineEvent::CommunitiesChanged);
        }
        Ok(published)
    }

    /// The devices of our root, this one first.
    pub fn devices(&self) -> Result<Vec<DeviceView>> {
        let me = self.inner.identity.device_id();
        let mut out = vec![DeviceView {
            device_id: me.to_string(),
            name: self.inner.identity.device_name.clone(),
            this_device: true,
            added_ms: self.inner.identity.cert.issued_at_ms,
            last_seen_ms: now_ms(),
        }];
        for record in self.load_devices()? {
            if record.cert.device == me {
                continue;
            }
            out.push(DeviceView {
                device_id: record.cert.device.to_string(),
                name: record.name,
                this_device: false,
                added_ms: record.added_ms,
                last_seen_ms: record.last_seen_ms,
            });
        }
        Ok(out)
    }

    /// Certifies the device that sent `link_request`, and returns the `dissent://grant/...` link
    /// for it. Call it on a device that holds the root key.
    pub fn approve_device(&self, link_request: &str) -> Result<String> {
        let root = self.root_key()?;
        let request = LinkRequest::parse(link_request)?;
        let now = now_ms();
        let grant = root.grant(&request, now)?;
        let mut devices = self.load_devices()?;
        devices.retain(|d| d.cert.device != request.device);
        devices.push(DeviceRecord {
            cert: grant.cert.clone(),
            name: request.device_name.clone(),
            added_ms: now,
            last_seen_ms: now,
        });
        self.save_devices(&devices)?;
        // The user record lists our devices, so the new one goes on it.
        self.spawn_publish_user_record();
        Ok(grant.to_link()?)
    }

    /// Revokes a device of our root. The revocation is published in every community, so the
    /// device's later events are rejected. Events it sent before the revocation stay valid.
    /// Returns the devices that remain.
    pub async fn revoke_device(&self, device: DeviceId) -> Result<Vec<DeviceView>> {
        let revocation = self.root_key()?.revoke(device, now_ms());
        let mut devices = self.load_devices()?;
        devices.retain(|d| d.cert.device != device);
        self.save_devices(&devices)?;
        self.spawn_publish_user_record();
        let ids: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for id in ids {
            self.author_publish(id, ChannelId::CONTROL, Payload::DeviceRevoke(revocation))
                .await?;
        }
        self.devices()
    }
}
