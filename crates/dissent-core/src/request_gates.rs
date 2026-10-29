//! Who may ask this device for what, for requests that would otherwise tell a stranger something. See
//! `docs/design/06-threat-model.md`, "Who learns what from a device's answers".
//!
//! - Member addresses (`Genesis`, `Reconcile`) go only to a current, unbanned member of the community.
//!   An invite holder who is not a member yet gets the events it needs to join, and no addresses.
//! - Key packages carry the device's certificate, so they link a device id to its person. `KeyPackages`
//!   is answered only to a current member of the community the device is joining. `DmKeyPackage` is
//!   answered only to a device whose person this device already knows (a contact, a co-member, someone
//!   who sent a contact request or a group join), so a stranger who only knows a device id learns
//!   nothing. Each is limited per requester, so a pool cannot be drained. An inbox's `TakeKeyPackage` names its owner, so the
//!   requester learns no link it did not ask with. The inbox does not know whom its owner accepts, so
//!   it serves anyone, within a per-requester limit and an overall one.
//! - Version probes (`*Support`) are answered only to a device in scope: a contact, or a member of a
//!   community this device is in.
//! - Refusals use one text, `REFUSED`, so a stranger cannot tell an unknown community from one it may
//!   not see.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_identity::{CommunityId, DeviceId, RootId};

use crate::engine::Engine;
use crate::rpc::Response;
use crate::wire::SignedAddr;

/// The one refusal a requester gets when it may not see what it asked about, whatever the reason.
pub(crate) const REFUSED: &str = "access denied";

/// Key packages one requester may take from this device per `KEY_PACKAGE_WINDOW`. A group invite takes
/// one per device and an MLS join one, so this leaves room for a busy hour and stops a drain.
const KEY_PACKAGES_PER_WINDOW: usize = 12;
const KEY_PACKAGE_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Inbox key packages this device hands out per `KEY_PACKAGE_WINDOW`, from all requesters together, so
/// many fresh device ids cannot drain an inbox either.
const INBOX_TAKES_PER_WINDOW: usize = 60;
/// Most requesters tracked at once. The ones that asked longest ago are dropped first.
const MAX_TRACKED: usize = 1024;

/// Key packages handed to each requester, within `KEY_PACKAGE_WINDOW`, and an inbox's takes from
/// everyone. In memory only.
#[derive(Default)]
pub(crate) struct KeyPackageAsks(Mutex<BTreeMap<DeviceId, VecDeque<Instant>>>, Mutex<VecDeque<Instant>>);

impl KeyPackageAsks {
    /// Records one inbox take, and says whether the overall limit still allows it.
    pub(crate) fn admit_inbox_take(&self, now: Instant) -> bool {
        let mut takes = self.1.lock().unwrap_or_else(PoisonError::into_inner);
        while takes.front().is_some_and(|t| now.duration_since(*t) >= KEY_PACKAGE_WINDOW) {
            takes.pop_front();
        }
        if takes.len() >= INBOX_TAKES_PER_WINDOW {
            return false;
        }
        takes.push_back(now);
        true
    }

    /// Records one key package for `from`, and says whether it may have it.
    pub(crate) fn admit(&self, from: DeviceId, now: Instant) -> bool {
        let mut asks = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if asks.len() >= MAX_TRACKED && !asks.contains_key(&from) {
            asks.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < KEY_PACKAGE_WINDOW));
            while asks.len() >= MAX_TRACKED {
                let oldest = asks.iter().min_by_key(|(_, times)| times.back().copied()).map(|(id, _)| *id);
                match oldest {
                    Some(id) => asks.remove(&id),
                    None => break,
                };
            }
        }
        let times = asks.entry(from).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= KEY_PACKAGE_WINDOW) {
            times.pop_front();
        }
        if times.len() >= KEY_PACKAGES_PER_WINDOW {
            return false;
        }
        times.push_back(now);
        true
    }
}

impl Engine {
    /// The person behind `device`, when this device knows it.
    fn root_of_requester(&self, device: &DeviceId) -> Option<RootId> {
        self.read().directory.root_of(device)
    }

    /// Whether `device` belongs to a current, unbanned member of `community`. Storage members count.
    pub(crate) fn is_unbanned_member_device(&self, device: &DeviceId, community: CommunityId) -> bool {
        let Some(root) = self.root_of_requester(device) else {
            return false;
        };
        self.read()
            .communities
            .get(&community)
            .is_some_and(|c| c.is_member(&root) && !c.state().banned.contains(&root))
    }

    /// The member addresses `requester` may have for `community`: all of them for a current, unbanned
    /// member, and none for anyone else, such as an invite holder that has not joined yet. A joiner
    /// learns them once its join is in: from the members' gossip, and from its next reconcile.
    pub(crate) fn member_addrs_for(&self, requester: &DeviceId, community: CommunityId) -> Vec<SignedAddr> {
        if self.is_unbanned_member_device(requester, community) {
            self.member_addrs(community)
        } else {
            Vec::new()
        }
    }

    /// Whether `device` is in scope: a device of our own person, a contact, or a member of a community
    /// this device is in.
    pub(crate) fn in_scope_device(&self, device: &DeviceId) -> bool {
        let Some(root) = self.root_of_requester(device) else {
            return false;
        };
        crate::peer_exchange::in_scope(&self.read(), &root)
    }

    /// Whether `device` may take one of this device's direct-message key packages. Its person must be
    /// one this device knows already, from a certificate it was shown: a contact, a co-member, someone
    /// who asked to be a contact or to join a group. That person knows whose device this is anyway.
    /// Whether they may then message this person is the DM policy's decision, made when the message
    /// arrives, as before. A blocked person gets none. Under the `Everyone` policy a device this one does
    /// not know may ask too, since anyone may then message this person.
    fn may_take_dm_key_package(&self, device: &DeviceId) -> bool {
        let w = self.read();
        match w.directory.root_of(device) {
            Some(root) => !w.contact_book.blocked.contains(&root),
            None => w.contact_book.policy == crate::contacts::DmPolicy::Everyone,
        }
    }

    /// Answers `KeyPackages`: a key package for the member that adds this device to `community`'s group.
    /// Only a current member may ask, within the per-requester limit.
    pub(crate) fn serve_join_key_packages(&self, from: DeviceId, community: CommunityId) -> Response {
        if !self.is_unbanned_member_device(&from, community) {
            return Response::Error(REFUSED.into());
        }
        self.hand_out_key_package(from)
    }

    /// Answers `DmKeyPackage`. See `may_take_dm_key_package`.
    pub(crate) fn serve_dm_key_package(&self, from: DeviceId) -> Response {
        if !self.may_take_dm_key_package(&from) {
            return Response::Error(REFUSED.into());
        }
        self.hand_out_key_package(from)
    }

    fn hand_out_key_package(&self, from: DeviceId) -> Response {
        if !self.inner.key_package_asks.admit(from, Instant::now()) {
            return Response::Error(REFUSED.into());
        }
        match self.with_mls(|mls| mls.take_key_packages(1)) {
            Ok(packages) => Response::KeyPackages(packages),
            Err(e) => Response::Error(format!("no key packages: {e}")),
        }
    }

    /// Answers an inbox's `TakeKeyPackage`, within the per-requester limit and the overall one.
    pub(crate) fn serve_inbox_key_package(&self, from: DeviceId, root: RootId) -> Response {
        let now = Instant::now();
        if !self.inner.key_package_asks.admit(from, now) || !self.inner.key_package_asks.admit_inbox_take(now) {
            return Response::Error(REFUSED.into());
        }
        self.take_key_package(root)
    }

    /// Answers a version probe (`*Support`) for a device in scope, and refuses anyone else, so a
    /// stranger cannot read this device's version from it.
    pub(crate) fn answer_probe(&self, from: &DeviceId, answer: Response) -> Response {
        if self.in_scope_device(from) {
            answer
        } else {
            Response::Error(REFUSED.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requester_gets_a_dozen_key_packages_an_hour() {
        let asks = KeyPackageAsks::default();
        let device = dissent_identity::DeviceKey::generate().id();
        let other = dissent_identity::DeviceKey::generate().id();
        let now = Instant::now();
        for _ in 0..KEY_PACKAGES_PER_WINDOW {
            assert!(asks.admit(device, now));
        }
        assert!(!asks.admit(device, now), "the pool is not drained");
        assert!(asks.admit(other, now), "another requester has its own allowance");
        assert!(asks.admit(device, now + KEY_PACKAGE_WINDOW), "the window moves on");
    }

    #[test]
    fn an_inbox_hands_out_a_bounded_number_of_packages_to_everyone_together() {
        let asks = KeyPackageAsks::default();
        let now = Instant::now();
        for _ in 0..INBOX_TAKES_PER_WINDOW {
            assert!(asks.admit_inbox_take(now));
        }
        assert!(!asks.admit_inbox_take(now), "fresh device ids cannot drain the inbox");
    }
}

/// Over loopback: what a member, an invite holder that never joined, and a stranger each get back.
#[cfg(test)]
mod net_tests {
    use super::*;
    use crate::membership::tests::{three_members, Node};
    use crate::rpc::Request;

    /// Teaches `to` the cert and addresses of `of`'s device, as a contact card would, without making
    /// them contacts.
    fn introduce(to: &Engine, of: &Engine) {
        let card = dissent_discovery::ContactCard::parse(&of.contact_link()).expect("card parses");
        let mut guard = to.write();
        guard.directory.learn(&card.cert);
        for peer in card.peers.iter().filter_map(|p| p.parse::<dissent_net::PeerAddr>().ok()) {
            to.learn_address(&mut guard, peer);
        }
    }

    /// Gives `node` the community secret from `invite`, as a leaked link would, without joining.
    fn hold_secret(node: &Node, invite: &str) -> CommunityId {
        let dissent_discovery::InviteLink::Full(invite) = dissent_discovery::InviteLink::parse(invite).expect("invite") else {
            panic!("a full invite");
        };
        let id = invite.genesis.community;
        let keys = crate::access::Keys::from_invite(invite.secret_generation, invite.secret);
        node.engine.write().access.insert(id, keys);
        id
    }

    fn device(node: &Node) -> DeviceId {
        node.engine.inner.identity.device_id()
    }

    fn refusal(reply: &Response) -> Option<&str> {
        match reply {
            Response::Error(why) => Some(why.as_str()),
            _ => None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_members_get_member_addresses_and_strangers_get_one_refusal() {
        crate::testutil::bounded(async {
            let (alice, bob, _carol, id, invite) = three_members().await;
            let alice_dev = device(&alice);

            // A member gets the members' signed addresses, with their IPs.
            let reply = bob.engine.call(alice_dev, &Request::Genesis { community: id }).await.expect("bob asks");
            let Response::Genesis { addrs, .. } = reply else { panic!("genesis for a member: {reply:?}") };
            assert!(!addrs.is_empty(), "a member gets member addresses");

            // An invite holder that never published a Join gets the genesis it needs, and no addresses.
            let lurker = Node::named("lurker").await;
            introduce(&lurker.engine, &alice.engine);
            assert_eq!(hold_secret(&lurker, &invite), id);
            let reply = lurker.engine.call(alice_dev, &Request::Genesis { community: id }).await.expect("lurker asks");
            let Response::Genesis { addrs, .. } = reply else { panic!("genesis for a secret holder: {reply:?}") };
            assert!(addrs.is_empty(), "no member addresses before a Join: {addrs:?}");
            let from = lurker.engine.read().my_addr.clone().expect("an address");
            let reply = lurker
                .engine
                .call(alice_dev, &Request::Reconcile { community: id, msg: Vec::new(), from })
                .await
                .expect("lurker reconciles");
            if let Response::Reconcile { addrs, .. } = &reply {
                assert!(addrs.is_empty(), "no member addresses from a reconcile either");
            }

            // A stranger cannot tell a community alice is in from one she is not: one refusal for both.
            let stranger = Node::named("stranger").await;
            introduce(&stranger.engine, &alice.engine);
            let known = stranger.engine.call(alice_dev, &Request::Genesis { community: id }).await.expect("asks");
            let unknown = stranger
                .engine
                .call(alice_dev, &Request::Genesis { community: CommunityId([9; 32]) })
                .await
                .expect("asks");
            assert!(refusal(&known).is_some_and(|why| why.ends_with(REFUSED)), "{known:?}");
            assert_eq!(refusal(&known), refusal(&unknown), "the same refusal for both");

            // A version probe tells the stranger nothing, and a member still gets its answer.
            let probe = stranger.engine.call(alice_dev, &Request::HistorySupport).await.expect("probe");
            assert_eq!(refusal(&probe), Some(REFUSED));
            let probe = bob.engine.call(alice_dev, &Request::HistorySupport).await.expect("probe");
            assert!(matches!(probe, Response::HistorySupport(true)), "a member reads it: {probe:?}");
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn key_packages_go_only_to_people_who_may_use_them_and_not_too_many() {
        crate::testutil::bounded(async {
            let (alice, bob, _carol, id, _invite) = three_members().await;
            let alice_dev = device(&alice);
            // The stranger knows alice's device, but alice was never shown the stranger's certificate.
            let stranger = Node::named("stranger").await;
            introduce(&stranger.engine, &alice.engine);

            // A stranger gets no key package, which would name the person behind the device.
            let dm = stranger.engine.call(alice_dev, &Request::DmKeyPackage).await.expect("asks");
            assert_eq!(refusal(&dm), Some(REFUSED), "{dm:?}");
            let join = stranger.engine.call(alice_dev, &Request::KeyPackages { community: id }).await.expect("asks");
            assert_eq!(refusal(&join), Some(REFUSED), "{join:?}");

            // A co-member may message alice under the default policy, so it gets one, up to the limit.
            for n in 0..KEY_PACKAGES_PER_WINDOW {
                let reply = bob.engine.call(alice_dev, &Request::DmKeyPackage).await.expect("asks");
                assert!(matches!(reply, Response::KeyPackages(ref p) if !p.is_empty()), "package {n}: {reply:?}");
            }
            let drained = bob.engine.call(alice_dev, &Request::DmKeyPackage).await.expect("asks");
            assert_eq!(refusal(&drained), Some(REFUSED), "the pool is not drained: {drained:?}");
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn our_address_goes_only_to_dm_peers_still_in_scope() {
        crate::testutil::bounded(async {
            let (alice, bob, _carol, _id, _invite) = three_members().await;
            let stranger = Node::named("stranger").await;
            introduce(&alice.engine, &stranger.engine);
            // A legacy DM from someone who is neither a contact nor a co-member.
            alice.engine.write().dms.insert(stranger.root(), Vec::new());
            assert!(
                !alice.engine.announce_targets().contains(&device(&stranger)),
                "a legacy DM peer out of scope gets no address announcements"
            );
            // A DM peer who shares a server is still in scope.
            alice.engine.write().dms.insert(bob.root(), Vec::new());
            assert!(alice.engine.announce_targets().contains(&device(&bob)), "a co-member still gets them");
        })
        .await;
    }
}
