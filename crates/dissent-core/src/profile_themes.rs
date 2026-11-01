//! Profile colours: a primary and an accent the UI draws as a gradient behind a person's profile card, DM panel
//! and full profile. They travel like a banner: as `Payload::ProfileThemeSet` in each community the person is in,
//! and as a `Request::ProfileTheme` sent directly to contacts. The newest by HLC wins, from any path. A contact
//! made after a change gets the colours from `exchange_themes`, which runs with the profile exchange. See
//! `profile_sync`.

use std::collections::BTreeMap;

use dissent_events::{ChannelId, Hlc, Payload, ProfileTheme};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::avatars::{peers_vault, write_sealed, Owner};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::presence::beacon_devices;
use crate::rpc::{Request, Response};
use crate::world::World;

const THEME_DOMAIN: &[u8] = b"dissent/profile-theme/v1";

/// The HLC of the colours of someone who never set any, which any set or clear beats.
const NEVER: Hlc = Hlc { wall_ms: 0, counter: 0 };

/// This device's own colours, and those contacts sent directly, each with the HLC of the change. A file of its own,
/// as `banners.bin` has a fixed shape. Sealed in `themes.bin`, like the other profile stores.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeBook {
    pub own: Option<(Hlc, Option<ProfileTheme>)>,
    pub peers: BTreeMap<RootId, (Hlc, Option<ProfileTheme>)>,
}

/// Colours sent directly to a contact's device, signed by the sending device, as `SignedBanner` is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SignedTheme {
    pub root: RootId,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub theme: Option<ProfileTheme>,
    pub cert: DeviceCert,
    pub sig: Signature,
}

fn signing_bytes(root: RootId, device: DeviceId, hlc: Hlc, theme: Option<ProfileTheme>) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(root, device, hlc, theme)).map_err(|e| CoreError::Encoding(e.to_string()))?;
    Ok([THEME_DOMAIN, body.as_slice()].concat())
}

impl SignedTheme {
    pub(crate) fn sign(key: &DeviceKey, cert: DeviceCert, hlc: Hlc, theme: Option<ProfileTheme>) -> Result<Self> {
        let (root, device) = (cert.root, cert.device);
        let sig = key.sign(&signing_bytes(root, device, hlc, theme)?);
        Ok(Self {
            root,
            device,
            hlc,
            theme,
            cert,
            sig,
        })
    }

    /// Checks the cert, that it binds the named device and root, and the device signature.
    pub(crate) fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.device || self.cert.root != self.root {
            return Err(CoreError::Invalid("theme does not match its certificate".into()));
        }
        self.device.verify(&signing_bytes(self.root, self.device, self.hlc, self.theme)?, &self.sig)?;
        Ok(())
    }
}

impl World {
    /// The colours a person shows: the newest from a community, a contact's direct message or, for us, this
    /// device, by HLC. A restored or second device so picks up its own colours from its communities.
    pub(crate) fn profile_theme(&self, root: &RootId) -> Option<ProfileTheme> {
        let direct = if *root == self.me { self.themes.own } else { self.themes.peers.get(root).copied() };
        let mut newest = direct;
        for c in self.communities.values() {
            if let Some(&(hlc, theme)) = c.state().profile_themes.get(root) {
                if newest.is_none_or(|(h, _)| hlc > h) {
                    newest = Some((hlc, theme));
                }
            }
        }
        newest.and_then(|(_, theme)| theme)
    }
}

impl Engine {
    /// Sets this device's profile colours. They go to every community we are in, and directly to our contacts.
    pub async fn set_profile_theme(&self, primary: [u8; 3], accent: [u8; 3]) -> Result<()> {
        self.publish_profile_theme(Some(ProfileTheme { primary, accent })).await
    }

    /// Removes this device's profile colours, everywhere they were published.
    pub async fn clear_profile_theme(&self) -> Result<()> {
        self.publish_profile_theme(None).await
    }

    async fn publish_profile_theme(&self, theme: Option<ProfileTheme>) -> Result<()> {
        let hlc = self.stamp();
        let communities: Vec<_> = {
            let mut w = self.write();
            w.themes.own = Some((hlc, theme));
            write_sealed(&w.paths.themes(), &peers_vault(&self.inner.vault), &w.themes)?;
            w.communities.keys().copied().collect()
        };
        for community in communities {
            let payload = Payload::ProfileThemeSet { theme };
            if let Err(e) = self.author_publish(community, ChannelId::CONTROL, payload).await {
                warn!(community = %community, error = %e, "profile theme not published to a community");
            }
        }
        self.send_theme_direct();
        self.emit_owner(Owner::Person(self.inner.me));
        Ok(())
    }

    /// Our colours, signed by this device with the HLC of the change, so an old copy never beats a newer one.
    fn own_theme(&self) -> Result<SignedTheme> {
        let (hlc, theme) = self.read().themes.own.unwrap_or((NEVER, None));
        SignedTheme::sign(&self.inner.identity.device, self.inner.identity.cert.clone(), hlc, theme)
    }

    /// Sends our colours to each contact device, over the direct connection, in the background.
    fn send_theme_direct(&self) {
        let bytes = self.own_theme().and_then(|signed| self.request_bytes(&Request::ProfileTheme(Box::new(signed))));
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not prepare our theme");
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
                    debug!(device = %device, error = %e, "theme not delivered directly");
                }
            });
        }
    }

    /// Sends our colours to a contact's device and takes theirs from the answer. An older device answers with
    /// an error, which changes nothing.
    pub(crate) async fn exchange_themes(&self, device: DeviceId) {
        let Ok(signed) = self.own_theme() else { return };
        match self.call(device, &Request::ProfileTheme(Box::new(signed))).await {
            Ok(Response::ProfileTheme(theirs)) => {
                if let Err(e) = self.accept_profile_theme(*theirs) {
                    debug!(device = %device, error = %e, "synced theme not taken");
                }
            }
            Ok(_) => debug!(device = %device, "peer cannot sync themes"),
            Err(e) => debug!(device = %device, error = %e, "theme sync not delivered"),
        }
    }

    /// Answers a contact's `ProfileTheme`: takes their colours and sends ours. The colours must be signed by the
    /// device that sends them, and come from a contact who is not blocked.
    pub(crate) fn answer_profile_theme(&self, from: DeviceId, st: SignedTheme) -> Response {
        if st.device != from {
            return Response::Error("theme is not from the sending device".into());
        }
        if let Err(e) = self.accept_profile_theme(st) {
            return Response::Error(format!("invalid profile theme: {e}"));
        }
        match self.own_theme() {
            Ok(theme) => Response::ProfileTheme(Box::new(theme)),
            Err(e) => Response::Error(format!("could not sign our theme: {e}")),
        }
    }

    /// Accepts a contact's colours. The newest by HLC wins. A clear from someone we hold nothing for changes nothing.
    pub(crate) fn accept_profile_theme(&self, st: SignedTheme) -> Result<()> {
        st.verify()?;
        let root = st.root;
        if root == self.inner.me {
            return Ok(());
        }
        let newer = {
            let mut guard = self.write();
            let w = &mut *guard;
            if !w.is_contact(&root) || w.contact_book.blocked.contains(&root) {
                return Err(CoreError::Invalid("theme from someone who is not a contact".into()));
            }
            let newer = w.themes.peers.get(&root).map_or(st.theme.is_some(), |(hlc, _)| st.hlc > *hlc);
            if newer {
                w.themes.peers.insert(root, (st.hlc, st.theme));
                write_sealed(&w.paths.themes(), &peers_vault(&self.inner.vault), &w.themes)?;
            }
            newer
        };
        if newer {
            self.emit_owner(Owner::Person(root));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    #[test]
    fn a_signed_theme_verifies_and_a_changed_one_does_not() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        let hlc = Hlc { wall_ms: 5, counter: 0 };
        let theme = ProfileTheme { primary: [1, 2, 3], accent: [4, 5, 6] };
        let signed = SignedTheme::sign(&device, cert, hlc, Some(theme)).unwrap();
        signed.verify().unwrap();
        let mut changed = signed.clone();
        changed.theme = Some(ProfileTheme { primary: [9, 2, 3], accent: [4, 5, 6] });
        assert!(changed.verify().is_err(), "a changed theme must not verify");
        let mut older = signed.clone();
        older.hlc = NEVER;
        assert!(older.verify().is_err(), "a changed HLC must not verify");
    }

    /// The wire shape is exactly six colour bytes: nothing a peer could fill with text.
    #[test]
    fn a_theme_encodes_as_six_bytes() {
        let theme = ProfileTheme { primary: [1, 2, 3], accent: [4, 5, 6] };
        assert_eq!(postcard::to_allocvec(&theme).unwrap(), [1, 2, 3, 4, 5, 6]);
    }
}
