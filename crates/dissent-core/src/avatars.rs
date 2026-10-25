//! Profile pictures and community icons. A picture is an encrypted blob in `dissent-files`. The
//! event that names it (`ProfileUpdate` or `CommunityIconSet`), or the direct profile sent to a
//! contact, carries its key, so only members and contacts learn it. A device that learns a picture
//! fetches the blob from a device that has it, verifies it as it arrives, and decrypts it into
//! `avatars/`. The file is named by the blob's hash, so a new picture is a new file, and the views
//! show the file once it exists.

use std::time::Duration;

use dissent_events::{ChannelId, Hlc, Payload, Picture, Role, MAX_PICTURE_BYTES};
use dissent_files::{cipher_len, FileRef, Input};
use dissent_identity::{CommunityId, DeviceCert, DeviceId, DeviceKey, RootId, Signature, Vault};
use std::path::Path;
use dissent_net::PeerAddr;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::presence::beacon_devices;
use crate::rpc::Request;
use crate::store::{write_bytes_atomic, Paths};
use crate::views::EngineEvent;
use crate::community::Community;
use crate::world::World;

const PROFILE_DOMAIN: &[u8] = b"dissent/profile/v1";
/// Purpose of the vault subkey that seals this device's own picture (`avatar.bin`).
const OWN_PURPOSE: &[u8] = b"dissent/avatar-own/v1";
/// Purpose of the vault subkey that seals the pictures of contacts (`peer_avatars.bin`).
const PEERS_PURPOSE: &[u8] = b"dissent/avatar-peers/v1";
/// Bound to every sealed picture file, so a sealed blob cannot be swapped between files.
pub(crate) const FILE_AAD: &[u8] = b"dissent/avatar-file/v1";
/// Tries to fetch one picture before giving up. A later restart tries again.
const FETCH_TRIES: u32 = 30;
const FETCH_WAIT: Duration = Duration::from_secs(2);

/// Whose picture changed, for the notice the views refresh on.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Owner {
    Person(RootId),
    Community(CommunityId),
}

/// A picture to fetch for an accepted event, and who to fetch it from.
#[derive(Debug)]
pub(crate) struct PictureFetch {
    pub owner: Owner,
    pub picture: Option<Picture>,
    pub roots: Vec<RootId>,
}

/// A profile sent directly to a contact's device, signed by that device, as presence is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SignedProfile {
    pub root: RootId,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub avatar: Option<Picture>,
    pub cert: DeviceCert,
    pub sig: Signature,
}

fn signing_bytes(root: RootId, device: DeviceId, hlc: Hlc, avatar: Option<Picture>) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(root, device, hlc, avatar))
        .map_err(|e| CoreError::Encoding(e.to_string()))?;
    Ok([PROFILE_DOMAIN, body.as_slice()].concat())
}

impl SignedProfile {
    pub(crate) fn sign(key: &DeviceKey, cert: DeviceCert, hlc: Hlc, avatar: Option<Picture>) -> Result<Self> {
        let (root, device) = (cert.root, cert.device);
        let sig = key.sign(&signing_bytes(root, device, hlc, avatar)?);
        Ok(Self {
            root,
            device,
            hlc,
            avatar,
            cert,
            sig,
        })
    }

    /// Checks the cert, that it binds the named device and root, and the device signature.
    pub(crate) fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.device || self.cert.root != self.root {
            return Err(CoreError::Invalid("profile does not match its certificate".into()));
        }
        let msg = signing_bytes(self.root, self.device, self.hlc, self.avatar)?;
        self.device.verify(&msg, &self.sig)?;
        Ok(())
    }
}

/// The vault subkey for this device's own picture file.
pub(crate) fn own_vault(vault: &Vault) -> Vault {
    vault.subkey(OWN_PURPOSE)
}

/// The vault subkey for the contact pictures file.
pub(crate) fn peers_vault(vault: &Vault) -> Vault {
    vault.subkey(PEERS_PURPOSE)
}

/// Writes `value` sealed under `vault`. The picture keys in it stay encrypted at rest.
pub(crate) fn write_sealed<T: Serialize>(path: &Path, vault: &Vault, value: &T) -> Result<()> {
    let plain = postcard::to_allocvec(value).map_err(|e| CoreError::Encoding(e.to_string()))?;
    write_bytes_atomic(path, &vault.seal(FILE_AAD, &plain))?;
    Ok(())
}

/// Reads a file written by [`write_sealed`]. A file that does not open is a plaintext file from
/// before sealing: it is read once, then rewritten sealed, so the migration happens on first read.
pub(crate) fn read_sealed<T: Serialize + DeserializeOwned>(path: &Path, vault: &Vault) -> Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let opened = vault
        .open_sealed(FILE_AAD, &bytes)
        .ok()
        .and_then(|plain| postcard::from_bytes::<T>(&plain).ok());
    if let Some(value) = opened {
        return Ok(Some(value));
    }
    // Only a file that is exactly one plain value is migrated. Anything else is left as it is, so a
    // wrong key can never overwrite the file.
    let plain = match postcard::take_from_bytes::<T>(&bytes) {
        Ok((value, [])) => value,
        _ => return Err(CoreError::Encoding("avatar file is neither sealed nor plain".into())),
    };
    write_sealed(path, vault, &plain)?;
    Ok(Some(plain))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The stem of a picture's file: its blob hash in hex.
fn stem(picture: &Picture) -> String {
    hex(&picture.hash)
}

/// PNG for a PNG, WebP otherwise. The engine sends WebP, and falls back to PNG where WebP is not available.
fn extension_of(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "png"
    } else {
        "webp"
    }
}

/// The decrypted file for `picture`, if this device has it.
fn picture_file(paths: &Paths, picture: &Picture) -> Option<String> {
    let dir = paths.avatars_dir();
    ["webp", "png"]
        .iter()
        .map(|ext| dir.join(format!("{}.{ext}", stem(picture))))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

impl World {
    /// The picture a person shows: this device's own, or the newest one a community or a direct
    /// profile gave us, by HLC.
    pub(crate) fn avatar_of(&self, root: &RootId) -> Option<String> {
        if *root == self.me {
            return self.own_avatar.and_then(|p| picture_file(&self.paths, &p));
        }
        let mut newest: Option<(Hlc, Option<Picture>)> = self.peer_avatars.get(root).copied();
        for c in self.communities.values() {
            if let Some(&(hlc, picture)) = c.state().avatars.get(root) {
                if newest.is_none_or(|(h, _)| hlc > h) {
                    newest = Some((hlc, picture));
                }
            }
        }
        newest
            .and_then(|(_, picture)| picture)
            .and_then(|p| picture_file(&self.paths, &p))
    }

    /// The icon of a community, if it has one this device holds.
    pub(crate) fn icon_of(&self, c: &Community) -> Option<String> {
        c.state()
            .icon
            .and_then(|(_, picture)| picture)
            .and_then(|p| picture_file(&self.paths, &p))
    }
}

impl Engine {
    /// Sets this device's profile picture. `bytes` is an already-cropped WebP or PNG of at most
    /// 256 KB. It goes to every community we are in, and is sent directly to our contacts.
    pub async fn set_avatar(&self, bytes: Vec<u8>) -> Result<()> {
        let picture = self.store_picture(bytes).await?;
        {
            let mut w = self.write();
            write_sealed(&w.paths.own_avatar(), &own_vault(&self.inner.vault), &Some(picture))?;
            w.own_avatar = Some(picture);
        }
        self.publish_profile(Some(picture)).await;
        self.emit_owner(Owner::Person(self.inner.me));
        Ok(())
    }

    /// Removes this device's profile picture, everywhere it was published.
    pub async fn clear_avatar(&self) -> Result<()> {
        {
            let mut w = self.write();
            write_sealed(&w.paths.own_avatar(), &own_vault(&self.inner.vault), &None::<Picture>)?;
            w.own_avatar = None;
        }
        self.publish_profile(None).await;
        self.emit_owner(Owner::Person(self.inner.me));
        Ok(())
    }

    /// Sets a community's icon. Owners and admins only. `bytes` is as for [`Engine::set_avatar`].
    pub async fn set_community_icon(&self, community: CommunityId, bytes: Vec<u8>) -> Result<()> {
        self.check_can_set_icon(community)?;
        let picture = self.store_picture(bytes).await?;
        self.publish_icon(community, Some(picture)).await
    }

    /// Removes a community's icon. Owners and admins only.
    pub async fn clear_community_icon(&self, community: CommunityId) -> Result<()> {
        self.check_can_set_icon(community)?;
        self.publish_icon(community, None).await
    }

    fn check_can_set_icon(&self, community: CommunityId) -> Result<()> {
        let me = self.inner.me;
        let w = self.read();
        let c = w
            .communities
            .get(&community)
            .ok_or_else(|| CoreError::NotFound(community.to_string()))?;
        let allowed = c.owner() == me || c.state().roles.get(&me).is_some_and(|r| r.contains(&Role::Admin));
        if allowed {
            Ok(())
        } else {
            Err(CoreError::Rejected("NoPermission".into()))
        }
    }

    async fn publish_icon(&self, community: CommunityId, avatar: Option<Picture>) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::CommunityIconSet { avatar })
            .await?;
        self.emit_owner(Owner::Community(community));
        Ok(())
    }

    /// Adds a picture to the store and to this device's decrypted cache. Pictures over the cap are refused.
    async fn store_picture(&self, bytes: Vec<u8>) -> Result<Picture> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_PICTURE_BYTES {
            return Err(CoreError::Invalid("a picture must be at most 256 KB".into()));
        }
        let fref = self
            .inner
            .files
            .add(Input::Bytes {
                name: "avatar".into(),
                mime: "image/webp".into(),
                data: bytes.clone(),
            })
            .await
            .map_err(|e| CoreError::Invalid(e.to_string()))?;
        let picture = Picture {
            hash: fref.hash,
            key: fref.key,
            nonce_prefix: fref.nonce_prefix,
            size: fref.size,
        };
        let dir = self.read().paths.avatars_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.{}", stem(&picture), extension_of(&bytes)));
        crate::store::write_bytes_atomic(&path, &bytes)?;
        Ok(picture)
    }

    /// Publishes a profile picture to every community we are in, then to our contacts directly.
    async fn publish_profile(&self, avatar: Option<Picture>) {
        let communities: Vec<CommunityId> = self.read().communities.keys().copied().collect();
        for community in communities {
            let payload = Payload::ProfileUpdate { avatar };
            if let Err(e) = self.author_publish(community, ChannelId::CONTROL, payload).await {
                warn!(community = %community, error = %e, "profile picture not published to a community");
            }
        }
        self.send_profile_direct(avatar);
    }

    /// Sends the signed profile to each contact device, over the direct connection.
    fn send_profile_direct(&self, avatar: Option<Picture>) {
        let hlc = self.stamp();
        let profile = match SignedProfile::sign(&self.inner.identity.device, self.inner.identity.cert.clone(), hlc, avatar) {
            Ok(profile) => profile,
            Err(e) => {
                warn!(error = %e, "could not sign our profile");
                return;
            }
        };
        let bytes = match self.request_bytes(&Request::Profile(Box::new(profile))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our profile");
                return;
            }
        };
        let devices = beacon_devices(&self.read());
        let node = self.node();
        for device in devices {
            let node = node.clone();
            let bytes = bytes.clone();
            self.spawn_task(async move {
                if let Err(e) = node.request(device, bytes).await {
                    debug!(device = %device, error = %e, "profile not delivered directly");
                }
            });
        }
    }

    /// Accepts a profile from a contact. The newest by HLC wins. A picture is fetched in the background.
    pub(crate) fn accept_profile(&self, sp: SignedProfile) -> Result<()> {
        sp.verify()?;
        if sp.avatar.is_some_and(|p| !p.is_valid()) {
            return Err(CoreError::Invalid("a picture must be at most 256 KB".into()));
        }
        let root = sp.root;
        if root == self.inner.me {
            return Ok(());
        }
        let newer = {
            let mut guard = self.write();
            let w = &mut *guard;
            if !w.is_contact(&root) || w.contact_book.blocked.contains(&root) {
                return Err(CoreError::Invalid("profile from someone who is not a contact".into()));
            }
            let newer = w.peer_avatars.get(&root).is_none_or(|(hlc, _)| sp.hlc > *hlc);
            if newer {
                w.peer_avatars.insert(root, (sp.hlc, sp.avatar));
                write_sealed(&w.paths.peer_avatars(), &peers_vault(&self.inner.vault), &w.peer_avatars)?;
            }
            newer
        };
        if newer {
            match sp.avatar {
                Some(picture) => self.fetch_picture(Owner::Person(root), picture, vec![root]),
                None => self.emit_owner(Owner::Person(root)),
            }
        }
        Ok(())
    }

    /// Fetches a picture in the background, unless this device has it. `roots` are the people whose
    /// devices may hold the blob. Tries a few times, since a device may not be reachable yet.
    pub(crate) fn fetch_picture(&self, owner: Owner, picture: Picture, roots: Vec<RootId>) {
        if picture_file(&self.read().paths, &picture).is_some() {
            return;
        }
        let engine = Engine {
            inner: self.inner.clone(),
        };
        self.spawn_task(async move {
            for _ in 0..FETCH_TRIES {
                let providers = engine.providers_for(&roots);
                if !providers.is_empty() {
                    match engine.fetch_blob(&picture, &providers).await {
                        Ok(()) => {
                            engine.emit_owner(owner);
                            return;
                        }
                        Err(e) => debug!(error = %e, "picture not fetched yet"),
                    }
                }
                tokio::time::sleep(FETCH_WAIT).await;
            }
            debug!("gave up fetching a picture for now");
        });
    }

    /// Downloads, verifies and decrypts one picture into the avatars directory.
    async fn fetch_blob(&self, picture: &Picture, providers: &[PeerAddr]) -> Result<()> {
        let from: Vec<_> = providers.iter().filter_map(|p| p.to_iroh().ok()).collect();
        let dir = self.read().paths.avatars_dir();
        std::fs::create_dir_all(&dir)?;
        let fref = FileRef {
            hash: picture.hash,
            size: picture.size,
            cipher_size: cipher_len(picture.size),
            name: format!("{}.bin", stem(picture)),
            mime: "image/webp".into(),
            key: picture.key,
            nonce_prefix: picture.nonce_prefix,
        };
        let node = self.node();
        let path = self
            .inner
            .files
            .fetch(node.endpoint(), &from, &fref, &dir, |_, _| {})
            .await
            .map_err(|e| CoreError::Invalid(e.to_string()))?;
        // The plaintext's type decides its extension, which the asset protocol uses as the MIME type.
        let bytes = std::fs::read(&path)?;
        if bytes.len() as u64 > MAX_PICTURE_BYTES {
            let _ = std::fs::remove_file(&path);
            return Err(CoreError::Invalid("the picture is over 256 KB".into()));
        }
        let named = dir.join(format!("{}.{}", stem(picture), extension_of(&bytes)));
        if named != path {
            std::fs::rename(&path, &named)?;
        }
        Ok(())
    }

    /// Fetches the pictures this device names but has not decrypted, for example after a restart.
    pub(crate) fn refetch_pictures(&self) {
        let wanted: Vec<(Owner, Picture, Vec<RootId>)> = {
            let w = self.read();
            let mut out = Vec::new();
            for (root, (_, picture)) in &w.peer_avatars {
                if let Some(picture) = picture {
                    out.push((Owner::Person(*root), *picture, vec![*root]));
                }
            }
            for c in w.communities.values() {
                for (root, (_, picture)) in &c.state().avatars {
                    if let Some(picture) = picture {
                        out.push((Owner::Person(*root), *picture, vec![*root]));
                    }
                }
                if let Some((_, Some(picture))) = c.state().icon {
                    let roots: Vec<RootId> = c.state().members.keys().copied().collect();
                    out.push((Owner::Community(c.id()), picture, roots));
                }
            }
            out
        };
        for (owner, picture, roots) in wanted {
            self.fetch_picture(owner, picture, roots);
        }
    }

    /// Tells the views that `owner`'s picture changed.
    pub(crate) fn emit_owner(&self, owner: Owner) {
        let event = match owner {
            Owner::Person(root) => EngineEvent::AvatarChanged {
                root_id: Some(root.to_string()),
                community_id: None,
            },
            Owner::Community(id) => EngineEvent::AvatarChanged {
                root_id: None,
                community_id: Some(id.to_string()),
            },
        };
        self.emit(event);
    }
}

/// The picture fetches an accepted control event asks for. The caller performs them after the lock is released.
pub(crate) fn picture_fetch_for(community: CommunityId, c: &Community, author: RootId, payload: &Payload) -> Option<PictureFetch> {
    match payload {
        Payload::ProfileUpdate { avatar } => {
            // Only the newest picture is fetched. An older event that lost to a newer one needs nothing.
            let current = c.state().avatars.get(&author).is_some_and(|(_, p)| p == avatar);
            current.then(|| PictureFetch {
                owner: Owner::Person(author),
                picture: *avatar,
                roots: vec![author],
            })
        }
        Payload::CommunityIconSet { avatar } => {
            let current = c.state().icon.is_some_and(|(_, p)| p == *avatar);
            current.then(|| PictureFetch {
                owner: Owner::Community(community),
                picture: *avatar,
                roots: c.state().members.keys().copied().collect(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    #[test]
    fn a_signed_profile_verifies_and_a_changed_one_does_not() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        let picture = Picture {
            hash: [1; 32],
            key: [2; 32],
            nonce_prefix: [3; 8],
            size: 10,
        };
        let hlc = Hlc {
            wall_ms: 5,
            counter: 0,
        };
        let sp = SignedProfile::sign(&device, cert, hlc, Some(picture)).unwrap();
        sp.verify().unwrap();

        let mut removed = sp.clone();
        removed.avatar = None;
        assert!(removed.verify().is_err(), "a changed picture must not verify");

        let mut older = sp.clone();
        older.hlc.wall_ms = 4;
        assert!(older.verify().is_err(), "a changed HLC must not verify");

        let stranger = DeviceKey::generate();
        let mut foreign = sp.clone();
        foreign.cert = root.certify(stranger.id(), 0);
        assert!(foreign.verify().is_err(), "a cert for another device must not verify");
    }

    #[test]
    fn an_extension_comes_from_the_bytes() {
        assert_eq!(extension_of(b"\x89PNG\r\n\x1a\n...."), "png");
        assert_eq!(extension_of(b"RIFF....WEBP"), "webp");
    }
}

#[cfg(test)]
mod sealed_file_tests {
    use super::*;

    fn picture() -> Picture {
        Picture {
            hash: [0x5a; 32],
            key: [0xa5; 32],
            nonce_prefix: [1; 8],
            size: 42,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-avatar-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("avatar.bin")
    }

    #[test]
    fn a_sealed_picture_file_hides_its_key_and_reads_back() {
        let vault = Vault::ephemeral_for_tests();
        let path = scratch("sealed");
        write_sealed(&path, &own_vault(&vault), &Some(picture())).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(32).any(|w| w == picture().key), "the key is in the clear");
        assert!(!bytes.windows(32).any(|w| w == picture().hash), "the hash is in the clear");
        let back: Option<Option<Picture>> = read_sealed(&path, &own_vault(&vault)).unwrap();
        assert_eq!(back, Some(Some(picture())));
    }

    #[test]
    fn a_plaintext_file_from_before_sealing_is_migrated_on_first_read() {
        let vault = Vault::ephemeral_for_tests();
        let path = scratch("migrate");
        // What the previous version wrote: plain postcard.
        std::fs::write(&path, postcard::to_allocvec(&Some(picture())).unwrap()).unwrap();

        let first: Option<Option<Picture>> = read_sealed(&path, &peers_vault(&vault)).unwrap();
        assert_eq!(first, Some(Some(picture())));
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(32).any(|w| w == picture().key), "the migrated file is still plain");

        let second: Option<Option<Picture>> = read_sealed(&path, &peers_vault(&vault)).unwrap();
        assert_eq!(second, Some(Some(picture())));
    }

    #[test]
    fn another_device_or_purpose_cannot_open_the_file() {
        let vault = Vault::ephemeral_for_tests();
        let other = Vault::ephemeral_for_tests();
        let path = scratch("foreign");
        write_sealed(&path, &own_vault(&vault), &Some(picture())).unwrap();
        assert!(read_sealed::<Option<Picture>>(&path, &own_vault(&other)).is_err());
        assert!(read_sealed::<Option<Picture>>(&path, &peers_vault(&vault)).is_err());
        // A refused read leaves the file as it was.
        assert!(read_sealed::<Option<Picture>>(&path, &own_vault(&vault)).unwrap().is_some());
    }

    #[test]
    fn a_missing_file_is_none() {
        let vault = Vault::ephemeral_for_tests();
        let path = scratch("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_sealed::<Option<Picture>>(&path, &own_vault(&vault)).unwrap(), None);
    }
}
