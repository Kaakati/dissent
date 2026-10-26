//! Exchanging direct profiles with contacts. A contact's picture and banner travel directly when
//! they change. On its own, that misses a contact made after the change, since nothing is sent
//! again, so their initials show for good. So the first time a run hears a contact's direct beacon, it
//! sends `Request::ProfileSync` carrying its own signed profile. The answer carries the contact's
//! profile and banner. The beacon arrives when the contact is accepted, when their device comes
//! online, and after a restart, so a stale or unknown profile is fixed then.
//!
//! A device from before this (v0.1.6) cannot decode `ProfileSync` and answers with an error. It still
//! takes the plain `Request::Profile` push, so it is sent ours that way. It cannot send us its
//! profile until it changes its picture again or updates.

use std::sync::atomic::Ordering;
use std::sync::PoisonError;

use dissent_identity::{DeviceId, RootId};
use tracing::debug;

use crate::avatars::SignedProfile;
use crate::engine::Engine;
use crate::profile_banners::SignedBanner;
use crate::rpc::{Request, Response};

impl Engine {
    /// Our profile and banner, signed now by this device.
    fn own_profile(&self) -> Option<(SignedProfile, Option<SignedBanner>)> {
        let (avatar, banner) = {
            let w = self.read();
            (w.own_avatar, w.banners.own)
        };
        let hlc = self.stamp();
        let (key, cert) = (&self.inner.identity.device, self.inner.identity.cert.clone());
        let profile = SignedProfile::sign(key, cert.clone(), hlc, avatar).ok()?;
        let banner = banner.and_then(|b| SignedBanner::sign(key, cert, hlc, Some(b)).ok());
        Some((profile, banner))
    }

    /// Exchanges direct profiles with `root`'s `device`, once this run, in the background. A failed
    /// exchange is forgotten, so the next beacon tries again.
    pub(crate) fn sync_profile_with(&self, root: RootId, device: DeviceId) {
        if self.inner.legacy_profiles.load(Ordering::Relaxed) || !self.read().is_contact(&root) {
            return;
        }
        if !self
            .inner
            .profile_synced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root)
        {
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            if !engine.exchange_profiles(device).await {
                engine
                    .inner
                    .profile_synced
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&root);
            }
        });
    }

    /// Sends our profile and takes theirs. A peer that cannot answer gets ours pushed instead.
    async fn exchange_profiles(&self, device: DeviceId) -> bool {
        let Some((profile, banner)) = self.own_profile() else {
            return false;
        };
        match self.call(device, &Request::ProfileSync(Box::new(profile.clone()))).await {
            Ok(Response::ProfileSync { profile: theirs, banner: their_banner }) => {
                if let Err(e) = self.accept_profile(*theirs) {
                    debug!(device = %device, error = %e, "synced profile not taken");
                }
                if let Some(b) = their_banner {
                    if let Err(e) = self.accept_profile_banner(*b) {
                        debug!(device = %device, error = %e, "synced banner not taken");
                    }
                }
                true
            }
            Ok(_) => {
                // An older device: it takes a pushed profile, and a banner when it knows banners.
                let pushed = self.call(device, &Request::Profile(Box::new(profile))).await;
                if let Some(b) = banner {
                    let _ = self.call(device, &Request::ProfileBanner(Box::new(b))).await;
                }
                debug!(device = %device, ok = pushed.is_ok(), "peer cannot sync profiles; pushed ours");
                pushed.is_ok()
            }
            Err(e) => {
                debug!(device = %device, error = %e, "profile sync not delivered");
                false
            }
        }
    }

    /// Answers a contact's `ProfileSync`: takes their profile and sends ours. The profile must be
    /// signed by the device that asks, and come from a contact who is not blocked.
    pub(crate) fn answer_profile_sync(&self, from: DeviceId, sp: SignedProfile) -> Response {
        if self.inner.legacy_profiles.load(Ordering::Relaxed) {
            return Response::Error("bad request: unknown variant".into());
        }
        if sp.device != from {
            return Response::Error("profile is not from the asking device".into());
        }
        let root = sp.root;
        if let Err(e) = self.accept_profile(sp) {
            return Response::Error(format!("invalid profile: {e}"));
        }
        // They have ours from this answer, so this run need not start another exchange with them.
        self.inner
            .profile_synced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root);
        match self.own_profile() {
            Some((profile, banner)) => Response::ProfileSync {
                profile: Box::new(profile),
                banner: banner.map(Box::new),
            },
            None => Response::Error("could not sign our profile".into()),
        }
    }
}
