//! Mutual friends on profiles. A person may share their contact list with their own contacts, and
//! with nobody else. Their contacts learn the whole list, not only the overlap with the viewer. The
//! viewer intersects that list with their own contacts, so the mutual list is computed locally.
//!
//! The list is asked for over RPC (`Request::Friends`), answered only to a device whose root is an
//! accepted, unblocked contact. Everyone else gets nothing. Older versions do not know the request
//! and answer with an error, which counts as nothing. See `docs/design/06-threat-model.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dissent_identity::{DeviceId, RootId};
use serde::Serialize;
use tracing::debug;

use crate::engine::Engine;
use crate::error::Result;
use crate::rpc::{Request, Response};

/// How long a contact's list is reused before it is asked for again. Adds, removals and turning the
/// setting off show within this. Two minutes keeps the asks few, and a withdrawal short-lived.
const CACHE_FOR: Duration = Duration::from_secs(120);
/// How long a "shares nothing" answer is reused. Short, so turning sharing on shows at once.
const CACHE_NOTHING_FOR: Duration = Duration::from_secs(30);
/// Requests one requester may make of this device within `ASK_WINDOW`.
const ASKS_PER_WINDOW: usize = 6;
const ASK_WINDOW: Duration = Duration::from_secs(60);
/// Requesters remembered at once. Older ones are dropped once the table grows past this.
const MAX_TRACKED_ASKERS: usize = 1024;
/// How long one device may take to answer before the next is tried.
const ASK_WAIT: Duration = Duration::from_secs(10);

/// A person on a mutual-friends list, as the profile shows them.
#[derive(Clone, Debug, Serialize)]
pub struct FriendView {
    pub root_id: String,
    pub display_name: String,
    pub suffix: String,
    /// Path of the decrypted picture on this device, once fetched. `None` when there is none yet.
    pub avatar: Option<String>,
}

/// The people on both lists, without `me` and without `peer` themselves, in id order.
pub(crate) fn mutual(mine: &[RootId], theirs: &[RootId], me: RootId, peer: RootId) -> Vec<RootId> {
    let mine: BTreeSet<RootId> = mine.iter().copied().collect();
    let theirs: BTreeSet<RootId> = theirs.iter().copied().collect();
    theirs
        .into_iter()
        .filter(|r| *r != me && *r != peer && mine.contains(r))
        .collect()
}

/// A contact's list, or `None` when it shared nothing.
type FriendList = Option<Vec<RootId>>;

/// A cached answer and when it was fetched.
type Cached = (Instant, FriendList);

/// Friend lists this device holds, and how often each requester has asked it. In memory only.
#[derive(Default)]
pub(crate) struct FriendsState {
    /// The list each contact answered, for `CACHE_FOR`, or for `CACHE_NOTHING_FOR` when it shared nothing.
    cache: Mutex<BTreeMap<RootId, Cached>>,
    /// When each requester asked this device, within `ASK_WINDOW`.
    asks: Mutex<BTreeMap<DeviceId, VecDeque<Instant>>>,
}

impl FriendsState {
    fn admit(&self, from: DeviceId, now: Instant) -> bool {
        let mut asks = self.asks.lock().unwrap_or_else(PoisonError::into_inner);
        if asks.len() >= MAX_TRACKED_ASKERS {
            asks.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < ASK_WINDOW));
        }
        let times = asks.entry(from).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= ASK_WINDOW) {
            times.pop_front();
        }
        if times.len() >= ASKS_PER_WINDOW {
            return false;
        }
        times.push_back(now);
        true
    }

    fn cached(&self, peer: &RootId) -> Option<FriendList> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .get(peer)
            .filter(|(at, list)| at.elapsed() < ttl(list))
            .map(|(_, list)| list.clone())
    }

    fn remember(&self, peer: RootId, list: FriendList) {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.retain(|_, (at, list)| at.elapsed() < ttl(list));
        cache.insert(peer, (Instant::now(), list));
    }
}

fn ttl(list: &FriendList) -> Duration {
    if list.is_some() {
        CACHE_FOR
    } else {
        CACHE_NOTHING_FOR
    }
}

impl Engine {
    /// Whether this device shows its friends list to its friends. On unless turned off.
    pub fn friends_shared(&self) -> bool {
        let paths = self.read().paths.clone();
        !paths.read_bin::<bool>(&paths.friends_hidden()).ok().flatten().unwrap_or_default()
    }

    /// Turns "Show my friends list to my friends" on or off. Off sends nothing, and profiles show no
    /// mutual friends from this device.
    pub fn set_friends_shared(&self, on: bool) -> Result<()> {
        let paths = self.read().paths.clone();
        paths.write_bin(&paths.friends_hidden(), &!on)
    }

    /// Answers `Request::Friends`. The list is our accepted contacts, and only for a device whose root
    /// is one of them and is not blocked. Anything else gets `None`, so the reply does not show who
    /// is a contact.
    pub(crate) fn serve_friends(&self, from: DeviceId) -> Response {
        let shared = self.friends_shared();
        let contacts = {
            let w = self.read();
            let Some(root) = w.directory.root_of(&from) else {
                return Response::Friends(None);
            };
            if !shared || !w.is_contact(&root) || w.contact_book.blocked.contains(&root) {
                return Response::Friends(None);
            }
            w.contacts.iter().map(|c| c.cert.root).collect::<Vec<_>>()
        };
        if !self.inner.friends.admit(from, Instant::now()) {
            return Response::Refused("too many friend list requests; try again in a minute".into());
        }
        Response::Friends(Some(contacts))
    }

    /// The contact list `peer` shares with us, or `None` when it shares nothing, or cannot be reached.
    /// Answers are reused for `CACHE_FOR`. Older versions answer with an error, which counts as nothing.
    pub(crate) async fn fetch_friends(&self, peer: RootId) -> Option<Vec<RootId>> {
        if let Some(hit) = self.inner.friends.cached(&peer) {
            return hit;
        }
        let devices = self.providers_for(&[peer]);
        for device in devices {
            let reply = tokio::time::timeout(ASK_WAIT, self.call(device.id, &Request::Friends)).await;
            match reply {
                Ok(Ok(Response::Friends(list))) => {
                    self.inner.friends.remember(peer, list.clone());
                    return list;
                }
                Ok(Ok(other)) => {
                    debug!(peer = %peer, reply = ?other, "friend list not shared");
                    self.inner.friends.remember(peer, None);
                    return None;
                }
                Ok(Err(e)) => debug!(peer = %peer, error = %e, "friend list not fetched"),
                Err(_) => debug!(peer = %peer, "friend list request timed out"),
            }
        }
        None
    }

    /// The people on both `peer`'s list and ours. Empty unless `peer` is our contact, and empty when
    /// `peer` shares nothing with us.
    pub async fn mutual_friends(&self, peer: RootId) -> Vec<FriendView> {
        let me = self.inner.me;
        let (mine, is_contact) = {
            let w = self.read();
            (w.contacts.iter().map(|c| c.cert.root).collect::<Vec<_>>(), w.is_contact(&peer))
        };
        if !is_contact {
            return Vec::new();
        }
        let Some(theirs) = self.fetch_friends(peer).await else {
            return Vec::new();
        };
        let w = self.read();
        mutual(&mine, &theirs, me, peer)
            .into_iter()
            .map(|r| FriendView {
                root_id: r.to_string(),
                display_name: w.display_name(&r),
                suffix: r.suffix(),
                avatar: w.avatar_of(&r),
            })
            .collect()
    }
}

impl Engine {
    /// The profile of `user`, with the mutual friends that are asked for over the network.
    pub async fn user_profile_full(&self, user: RootId) -> crate::views::UserView {
        let mut view = self.user_profile(user);
        view.mutual_friends = self.mutual_friends(user).await;
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    fn root() -> RootId {
        RootKey::generate().id()
    }

    #[test]
    fn mutual_is_the_overlap_without_me_or_the_peer() {
        let (me, peer, carol, dave, erin) = (root(), root(), root(), root(), root());
        let mine = [me, peer, carol, dave];
        let theirs = [me, carol, dave, erin, peer, carol];
        assert_eq!(mutual(&mine, &theirs, me, peer), {
            let mut expected = vec![carol, dave];
            expected.sort();
            expected
        });
    }

    #[test]
    fn nothing_in_common_is_empty() {
        let (me, peer, x, y) = (root(), root(), root(), root());
        assert!(mutual(&[me, x], &[y, peer], me, peer).is_empty());
        assert!(mutual(&[], &[x], me, peer).is_empty());
    }

    #[test]
    fn a_requester_is_limited_per_window() {
        let state = FriendsState::default();
        let asker = dissent_identity::DeviceKey::generate().id();
        let now = Instant::now();
        for _ in 0..ASKS_PER_WINDOW {
            assert!(state.admit(asker, now));
        }
        assert!(!state.admit(asker, now));
        assert!(state.admit(dissent_identity::DeviceKey::generate().id(), now));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_we_do_not_know_gets_nothing() {
        crate::testutil::bounded(async {
            let node = crate::membership::tests::Node::named("solo").await;
            let stranger = dissent_identity::DeviceKey::generate().id();
            assert!(matches!(node.engine.serve_friends(stranger), Response::Friends(None)));
        })
        .await;
    }
}
