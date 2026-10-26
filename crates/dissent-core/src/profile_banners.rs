//! Personal profile banners: a preset gradient or a 16:9 picture behind a person's profile card, DM
//! panel and full profile. A banner travels like the profile picture, without touching its
//! encodings: as `Payload::ProfileBannerSet` in each community the person is in, and as a
//! `Request::ProfileBanner` sent directly to contacts. A picture is stored, fetched and decrypted into
//! `avatars/` like any other. The newest banner by HLC wins, from any community or the direct path.

use std::collections::BTreeMap;

use dissent_events::{Banner, ChannelId, Hlc, Payload, BANNER_PRESETS, MAX_BANNER_BYTES};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::avatars::{peers_vault, picture_file, write_sealed, Owner};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::presence::beacon_devices;
use crate::rpc::Request;
use crate::world::World;

const BANNER_DOMAIN: &[u8] = b"dissent/profile-banner/v1";

/// This device's own banner, and those contacts sent directly, by root with the HLC of each. Kept
/// sealed in `banners.bin`, as the picture keys inside must not be readable at rest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BannerBook {
    pub own: Option<Banner>,
    pub peers: BTreeMap<RootId, (Hlc, Option<Banner>)>,
}

/// A banner sent directly to a contact's device, signed by that device, as `SignedProfile` is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SignedBanner {
    pub root: RootId,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub banner: Option<Banner>,
    pub cert: DeviceCert,
    pub sig: Signature,
}

fn signing_bytes(root: RootId, device: DeviceId, hlc: Hlc, banner: Option<Banner>) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(root, device, hlc, banner)).map_err(|e| CoreError::Encoding(e.to_string()))?;
    Ok([BANNER_DOMAIN, body.as_slice()].concat())
}

impl SignedBanner {
    pub(crate) fn sign(key: &DeviceKey, cert: DeviceCert, hlc: Hlc, banner: Option<Banner>) -> Result<Self> {
        let (root, device) = (cert.root, cert.device);
        let sig = key.sign(&signing_bytes(root, device, hlc, banner)?);
        Ok(Self {
            root,
            device,
            hlc,
            banner,
            cert,
            sig,
        })
    }

    /// Checks the cert, that it binds the named device and root, the device signature, and the caps.
    pub(crate) fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.device || self.cert.root != self.root {
            return Err(CoreError::Invalid("banner does not match its certificate".into()));
        }
        self.device.verify(&signing_bytes(self.root, self.device, self.hlc, self.banner)?, &self.sig)?;
        if self.banner.is_some_and(|b| !b.is_valid()) {
            return Err(CoreError::Invalid("the banner is out of range".into()));
        }
        Ok(())
    }
}

impl World {
    /// The banner a person shows: this device's own, or the newest one a community or a direct
    /// banner gave us, by HLC. `None` when there is none.
    pub(crate) fn profile_banner(&self, root: &RootId) -> Option<Banner> {
        if *root == self.me {
            return self.banners.own;
        }
        let mut newest: Option<(Hlc, Option<Banner>)> = self.banners.peers.get(root).copied();
        for c in self.communities.values() {
            if let Some(&(hlc, banner)) = c.state().profile_banners.get(root) {
                if newest.is_none_or(|(h, _)| hlc > h) {
                    newest = Some((hlc, banner));
                }
            }
        }
        newest.and_then(|(_, banner)| banner)
    }

    /// The banner of a person as the views carry it: the picture's file once this device holds it,
    /// the still poster of an animated one, and the preset id when the banner is a preset.
    pub(crate) fn profile_banner_view(&self, root: &RootId) -> (Option<String>, Option<String>, Option<u8>) {
        let banner = self.profile_banner(root);
        let file = |p: Option<dissent_events::Picture>| p.and_then(|p| picture_file(&self.paths, &p));
        let preset = match banner {
            Some(Banner::Preset(id)) => Some(id),
            _ => None,
        };
        (file(banner.and_then(|b| b.picture())), file(banner.and_then(|b| b.poster())), preset)
    }
}

impl Engine {
    /// Sets this device's profile banner to a picture: an already-cropped 16:9 WebP or PNG of at most
    /// 512 KB. It goes to every community we are in, and is sent directly to our contacts.
    pub async fn set_profile_banner(&self, bytes: Vec<u8>) -> Result<()> {
        let picture = self.store_avatar_picture(bytes, MAX_BANNER_BYTES).await?;
        self.publish_profile_banner(Some(Banner::Image(picture))).await
    }

    /// Sets this device's profile banner to one of the `BANNER_PRESETS` gradients.
    pub async fn set_profile_banner_preset(&self, preset: u8) -> Result<()> {
        if preset >= BANNER_PRESETS {
            return Err(CoreError::Invalid(format!("a banner preset is below {BANNER_PRESETS}")));
        }
        self.publish_profile_banner(Some(Banner::Preset(preset))).await
    }

    /// Removes this device's profile banner, everywhere it was published.
    pub async fn clear_profile_banner(&self) -> Result<()> {
        self.publish_profile_banner(None).await
    }

    pub(crate) async fn publish_profile_banner(&self, banner: Option<Banner>) -> Result<()> {
        let communities: Vec<_> = {
            let mut w = self.write();
            w.banners.own = banner;
            write_sealed(&w.paths.banners(), &peers_vault(&self.inner.vault), &w.banners)?;
            w.communities.keys().copied().collect()
        };
        for community in communities {
            let payload = Payload::ProfileBannerSet { banner };
            if let Err(e) = self.author_publish(community, ChannelId::CONTROL, payload).await {
                warn!(community = %community, error = %e, "profile banner not published to a community");
            }
        }
        self.send_banner_direct(banner);
        self.emit_owner(Owner::Person(self.inner.me));
        Ok(())
    }

    /// Sends the signed banner to each contact device, over the direct connection, in the background.
    fn send_banner_direct(&self, banner: Option<Banner>) {
        let hlc = self.stamp();
        let signed = match SignedBanner::sign(&self.inner.identity.device, self.inner.identity.cert.clone(), hlc, banner) {
            Ok(signed) => signed,
            Err(e) => {
                warn!(error = %e, "could not sign our banner");
                return;
            }
        };
        let bytes = match self.request_bytes(&Request::ProfileBanner(Box::new(signed))) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our banner");
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
                    debug!(device = %device, error = %e, "banner not delivered directly");
                }
            });
        }
    }

    /// Accepts a banner from a contact. The newest by HLC wins. A picture is fetched in the background.
    pub(crate) fn accept_profile_banner(&self, sb: SignedBanner) -> Result<()> {
        sb.verify()?;
        let root = sb.root;
        if root == self.inner.me {
            return Ok(());
        }
        let newer = {
            let mut guard = self.write();
            let w = &mut *guard;
            if !w.is_contact(&root) || w.contact_book.blocked.contains(&root) {
                return Err(CoreError::Invalid("banner from someone who is not a contact".into()));
            }
            let newer = w.banners.peers.get(&root).is_none_or(|(hlc, _)| sb.hlc > *hlc);
            if newer {
                w.banners.peers.insert(root, (sb.hlc, sb.banner));
                write_sealed(&w.paths.banners(), &peers_vault(&self.inner.vault), &w.banners)?;
            }
            newer
        };
        if newer {
            let pictures = sb.banner.map(|b| b.pictures()).unwrap_or_default();
            if pictures.is_empty() {
                self.emit_owner(Owner::Person(root));
            }
            for picture in pictures {
                self.fetch_picture(Owner::Person(root), picture, vec![root]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_events::Picture;
    use dissent_identity::RootKey;

    #[test]
    fn a_signed_banner_verifies_and_a_changed_one_does_not() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        let hlc = Hlc { wall_ms: 5, counter: 0 };
        let banner = Banner::Image(Picture {
            hash: [1; 32],
            key: [2; 32],
            nonce_prefix: [3; 8],
            size: 300 * 1024,
        });
        let sb = SignedBanner::sign(&device, cert, hlc, Some(banner)).unwrap();
        sb.verify().unwrap();
        let mut preset = sb.clone();
        preset.banner = Some(Banner::Preset(2));
        assert!(preset.verify().is_err(), "a changed banner must not verify");
        let mut huge = sb.clone();
        huge.banner = Some(Banner::Preset(BANNER_PRESETS));
        assert!(huge.verify().is_err());
    }
}
