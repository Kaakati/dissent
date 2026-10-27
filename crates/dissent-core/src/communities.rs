//! Community lifecycle: create, invite, join, leave, and the read side of one community.

use crate::community::Community;
use crate::engine::{clean_text, not_found, now_ms, Engine};
use std::sync::atomic::Ordering;
use crate::error::{CoreError, Result};
use crate::net::ReceiverTask;
use crate::rpc::{Request, Response};
use crate::store::{write_bin, write_community_key};
use crate::views::{CommunitySummary, CommunityView, EngineEvent};
use crate::wire::SignedAddr;
use dissent_discovery::{CommunityRecord, Invite, InviteLink, ShortInvite};
use dissent_events::{ChannelId, HistorySetting, Payload, SignedEvent};
use dissent_identity::{CommunityId, CommunityKey, DeviceId, Genesis};
use dissent_net::{PeerAddr, TopicId, TopicReceiver, TopicSender};
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tracing::{debug, warn};

/// How long a join waits for a gossip neighbour before it announces itself.
const JOIN_GOSSIP_WAIT: Duration = Duration::from_secs(5);
/// How long leaving waits for the receive loop to stop before the loop is aborted.
const LEAVE_WAIT: Duration = Duration::from_secs(2);
/// How long a join waits for the DHT to return a community record.
/// Resolve can take a DHT warm-up plus retries, so give it room.
pub(crate) const DHT_LOOKUP_WAIT: Duration = Duration::from_secs(90);
/// Tries to join the encrypted group this many times, spaced by `JOIN_RETRY_EVERY`, before maintenance takes over.
const JOIN_ATTEMPTS: usize = 6;
const JOIN_RETRY_EVERY: Duration = Duration::from_millis(500);

impl Engine {
    /// Creates a community with a `general` channel. Publishes its DHT record in the background.
    pub async fn create_community(&self, name: String) -> Result<CommunitySummary> {
        let name = clean_text(&name, "community name")?;
        let key = CommunityKey::generate();
        let id = key.id();
        let genesis = Genesis::create(&key, self.inner.me, name.clone(), now_ms());
        {
            let mut guard = self.write();
            let paths = guard.paths.clone();
            write_bin(&paths.genesis(&id), &genesis)?;
            write_community_key(&paths.community_key(&id), &self.inner.vault, &id, &key.0.seed())?;
            let secret = crate::access::random_secret();
            let keys = crate::access::Keys::from_invite(0, secret);
            crate::access::save(&paths, &id, &keys)?;
            let mut community = Community::new(genesis);
            community.set_ring(keys.history.clone());
            guard.access.insert(id, keys);
            guard.communities.insert(id, community);
        }
        self.inner
            .founded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, key.0.seed());
        self.with_mls(|mls| mls.create_group(id.0)).map_err(crate::sealed::mls_err)?;
        self.note_epoch(id, &[]);
        let topic_rx = self.subscribe_topic(id, Vec::new()).await?;
        self.spawn_receiver(id, topic_rx);

        let display_name = self.read().my_name();
        self.author_publish(id, ChannelId::CONTROL, Payload::Join { display_name })
            .await?;
        self.author_publish(
            id,
            ChannelId::CONTROL,
            Payload::ChannelCreate {
                name: "general".into(),
            },
        )
        .await?;

        // New servers start with "since joined": a joiner sees only messages from after it joined. Owners can
        // switch to "all" later. Servers created before this change keep their replayed setting.
        self.set_history_visibility(id, HistorySetting::SinceJoin).await?;
        self.spawn_publish_records();
        self.emit(EngineEvent::CommunitiesChanged);
        self.summary(&id)
    }

    /// Publishes the DHT records in the background, if we have a DHT client.
    pub(crate) fn spawn_publish_records(&self) {
        if self.inner.discovery.is_some() {
            let engine = self.clone();
            self.spawn_task(async move { engine.publish_records().await });
        }
    }

    /// Publishes the DHT record of every community we founded. Each record names our address
    /// without direct IPs, so the public DHT never learns where we listen. Devices are dialled by
    /// id through the Mainline address lookup.
    pub(crate) async fn publish_records(&self) {
        let Some(discovery) = self.inner.discovery.clone() else {
            return;
        };
        // Publishes of one record must not overlap. The DHT keeps only the newest packet, and an
        // older one that lands later is refused.
        let _publishing = self.inner.publish_lock.lock().await;
        let jobs: Vec<(CommunityKey, [u8; 32], CommunityRecord)> = {
            let w = self.read();
            let Some(addr) = w.my_addr.clone() else {
                return;
            };
            let founded = self
                .inner
                .founded
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            founded
                .iter()
                .flat_map(|(id, seed)| {
                    let (Some(c), Some(keys)) = (w.communities.get(id), w.access.get(id)) else {
                        return Vec::new();
                    };
                    let record = CommunityRecord {
                        name: c.name().to_string(),
                        peers: vec![public_addr(&addr).to_string()],
                        relays: c
                            .state()
                            .relays
                            .iter()
                            .filter_map(|s| url::Url::parse(s).ok())
                            .collect(),
                    };
                    // A short invite made before a rotation still names the retired secret, so the
                    // record stays under each retired secret for as long as its grace period lasts.
                    let now = now_ms();
                    let mut secrets = vec![keys.secret];
                    secrets.extend(keys.retired.iter().filter(|r| r.until_ms > now).map(|r| r.secret));
                    secrets
                        .into_iter()
                        .map(|secret| (CommunityKey::from_seed(*seed), secret, record.clone()))
                        .collect()
                })
                .collect()
        };
        let mut all_ok = true;
        for (key, secret, record) in jobs {
            // Discovery retries until enough nodes hold the record, so Ok means it is stored.
            match discovery.publish(&key, &secret, &record).await {
                Ok(stored_on) => debug!(stored_on, "published community record"),
                Err(e) => {
                    all_ok = false;
                    warn!(error = %e, "could not publish community record to the DHT");
                }
            }
        }
        self.inner.records_current.store(all_ok, Ordering::Relaxed);
    }

    /// An invite link for `community`, carrying our current address.
    pub async fn create_invite(&self, community: CommunityId) -> Result<String> {
        self.create_invite_until(community, None).await
    }

    /// An invite link that stops admitting people at `expires_ms`, when that is set. Its id is
    /// recorded in the community, so the invite can be revoked and listed.
    pub async fn create_invite_until(&self, community: CommunityId, expires_ms: Option<u64>) -> Result<String> {
        self.issue_invite(community, expires_ms, false).await
    }

    /// The long form of an invite, for people on versions before short invites. It works without
    /// the DHT, and is the link to copy when the short one cannot be read.
    pub async fn create_full_invite(&self, community: CommunityId) -> Result<String> {
        self.issue_invite(community, None, true).await
    }

    async fn issue_invite(&self, community: CommunityId, expires_ms: Option<u64>, full: bool) -> Result<String> {
        let addr = self.node().addr().await;
        let genesis = {
            let mut guard = self.write();
            guard.my_addr = Some(addr.clone());
            guard
                .communities
                .get(&community)
                .map(|c| c.genesis.clone())
                .ok_or_else(|| not_found(&community))?
        };
        let keys = self.access_keys(community).ok_or_else(|| not_found(&community))?;
        let invite_id: [u8; 16] = rand::random();
        self.author_publish(
            community,
            ChannelId::CONTROL,
            Payload::InviteCreate {
                invite_id,
                expires_ms,
            },
        )
        .await?;
        let link = if !full && self.founded_here(community) {
            // Only the founder signs the community's DHT record, so only its invites can be
            // resolved through the DHT. Other members hand out the long form.
            ShortInvite {
                community,
                secret: keys.secret,
                secret_generation: keys.generation,
                invite_id: Some(invite_id),
            }
            .to_url()
            .to_string()
        } else {
            Invite {
                genesis,
                peers: vec![addr.to_string()],
                secret: keys.secret,
                secret_generation: keys.generation,
                relays: self.community_relays(community),
                invite_id: Some(invite_id),
            }
            .to_url()
            .to_string()
        };
        self.record_invite(community, invite_id, link.clone(), now_ms())?;
        Ok(link)
    }

    /// Whether this device founded `community`, and so signs its DHT record.
    fn founded_here(&self, community: CommunityId) -> bool {
        self.inner.discovery.is_some()
            && self
                .inner
                .founded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&community)
    }

    /// Joins from an invite link. Joining a community we already have just returns its summary.
    pub async fn join_community(&self, invite: &str) -> Result<CommunitySummary> {
        self.join_inner(invite, true).await
    }

    /// Joins the community's DAG without its encrypted group. A device that is not in the group
    /// cannot read content, so tests use this to model an outside observer.
    #[cfg(test)]
    pub(crate) async fn join_without_group(&self, invite: &str) -> Result<CommunitySummary> {
        self.join_inner(invite, false).await
    }

    /// Joins as a storage member. The community's history is kept and served here, but this device
    /// is never added to the encrypted group, so it cannot read messages or attachments.
    pub async fn join_as_storage(&self, invite: &str) -> Result<CommunitySummary> {
        self.join_as(invite, false, true).await
    }

    async fn join_inner(&self, invite: &str, with_group: bool) -> Result<CommunitySummary> {
        self.join_as(invite, with_group, false).await
    }

    async fn join_as(&self, invite: &str, with_group: bool, storage: bool) -> Result<CommunitySummary> {
        let link = InviteLink::parse(invite)?;
        let id = link.community();
        let Some((_, served)) = self.sync_invite(invite).await? else {
            return self.summary(&id);
        };
        if let Some(invite_id) = link.invite_id() {
            self.refuse_known_dead_invite(id, invite_id)?;
        }
        self.announce_join_as(id, &served, with_group, storage, link.invite_id()).await;
        self.emit(EngineEvent::CommunitiesChanged);
        self.summary(&id)
    }
    /// Join. Returns the community and the devices that served it, or `None` if this device
    /// already holds the community.
    pub(crate) async fn sync_invite(
        &self,
        invite: &str,
    ) -> Result<Option<(CommunityId, Vec<DeviceId>)>> {
        let link = InviteLink::parse(invite)?;
        let id = link.community();
        if self.read().communities.contains_key(&id) {
            return Ok(None);
        }
        let invite = match link {
            InviteLink::Full(invite) => invite,
            InviteLink::Short(short) => {
                if self.inner.discovery.is_none() {
                    return Err(CoreError::Invalid(
                        "a short invite needs the DHT, which this device is not running".into(),
                    ));
                }
                // The genesis fetch below is a community request, so it needs the invite's keys.
                let keys = crate::access::Keys::from_invite(short.secret_generation, short.secret);
                let paths = self.read().paths.clone();
                crate::access::save(&paths, &id, &keys)?;
                self.write().access.insert(id, keys);
                self.resolve_short(&short).await?
            }
        };
        {
            let mut guard = self.write();
            let paths = guard.paths.clone();
            let keys = crate::access::Keys::from_invite(invite.secret_generation, invite.secret);
            crate::access::save(&paths, &id, &keys)?;
            guard.access.insert(id, keys);
        }
        self.adopt_relays(&invite.relays).await;
        self.await_relay().await;
        let my_addr = self
            .read()
            .my_addr
            .clone()
            .ok_or_else(|| CoreError::Invalid("endpoint address is not ready".into()))?;
        let invited: Vec<PeerAddr> = invite
            .peers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .collect();
        {
            let mut guard = self.write();
            let w = &mut *guard;
            for peer in &invited {
                self.learn_address(w, peer.clone());
            }
        }

        let (fetched, peers) = self.fetch_for_join(&invite.genesis, &invite.secret, &invited, &my_addr).await?;
        {
            let mut guard = self.write();
            write_bin(&guard.paths.genesis(&id), &invite.genesis)?;
            let mut community = Community::new(invite.genesis.clone());
            if let Some(keys) = guard.access.get(&id) {
                community.set_ring(keys.history.clone());
            }
            guard.communities.insert(id, community);
        }
        self.ingest(id, fetched.events)?;
        self.learn_member_addrs(id, fetched.addrs);

        let served: Vec<DeviceId> = peers.iter().map(|p| p.id).collect();
        let mut bootstrap: Vec<DeviceId> = served.clone();
        bootstrap.extend(self.bootstrap_for(&id));
        bootstrap.sort();
        bootstrap.dedup();
        let mut topic_rx = self.subscribe_topic(id, bootstrap).await?;
        // Announce only once a neighbour is up. Otherwise the Join would reach nobody.
        let _ = tokio::time::timeout(JOIN_GOSSIP_WAIT, topic_rx.joined()).await;
        self.spawn_receiver(id, topic_rx);
        Ok(Some((id, served)))
    }

    /// Announces this device's Join to the devices that served the invite, then joins the
    /// community's encrypted group.
    pub(crate) async fn announce_join(&self, id: CommunityId, served: &[DeviceId], with_group: bool) {
        self.announce_join_as(id, served, with_group, false, None).await;
    }

    /// As `announce_join`. A storage join announces `JoinAsStorage` and never joins the group.
    pub(crate) async fn announce_join_as(
        &self,
        id: CommunityId,
        served: &[DeviceId],
        with_group: bool,
        storage: bool,
        invite_id: Option<[u8; 16]>,
    ) {
        // A storage member's name stays: it is sealed to the community secret the seeder holds anyway.
        let name = self.read().my_name();
        let display_name = if storage { name } else { self.pre_join_name(id, name) };
        let payload = if storage {
            Payload::JoinAsStorage { display_name }
        } else if let Some(invite_id) = invite_id {
            Payload::JoinVia {
                display_name,
                invite_id,
            }
        } else {
            Payload::Join { display_name }
        };
        match self
            .author_publish(id, ChannelId::CONTROL, payload)
            .await
        {
            Ok(join) => {
                // Hand the Join straight to the peers that served the invite. Gossip may reach
                // them later, and until it does they cannot address us.
                for peer in served {
                    if let Err(e) = self.push(id, *peer, vec![join.clone()]).await {
                        debug!(community = %id, peer = %peer, error = %e, "join not pushed to invite peer");
                    }
                }
                // The peers now know our Join, so they serve the older secrets to us. Their join
                // events are sealed too, which is why we ask them rather than the members we cannot
                // see yet.
                self.fetch_history_from(id, served).await;
            }
            Err(e) => warn!(community = %id, error = %e, "could not announce join"),
        }
        // Join the encrypted group. An invite peer refuses until it has replayed our Join, which the
        // push above usually settles, so retry briefly before leaving it to maintenance.
        if with_group {
            let mut joined = false;
            for _ in 0..JOIN_ATTEMPTS {
                if self.mls_join_via(id, served).await {
                    joined = true;
                    break;
                }
                tokio::time::sleep(JOIN_RETRY_EVERY).await;
            }
            if !joined {
                warn!(community = %id, "could not join the encrypted group yet; will retry");
            }
        }
    }

    /// Announces leaving, leaves the gossip topic, then forgets the community and deletes its
    /// local data.
    pub async fn leave_community(&self, community: CommunityId) -> Result<()> {
        self.summary(&community)?;
        match self
            .author_publish(community, ChannelId::CONTROL, Payload::Leave)
            .await
        {
            Ok(leave) => {
                // Gossip is not acknowledged, and the topic closes at once below. Push the Leave to each device we
                // are connected to, so the others learn of it and remove our device from the group.
                let peers: Vec<DeviceId> = self
                    .inner
                    .neighbours
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&community)
                    .map(|set| set.iter().copied().collect())
                    .unwrap_or_default();
                for peer in peers {
                    if let Err(e) = self.push(community, peer, vec![leave.clone()]).await {
                        debug!(community = %community, peer = %peer, error = %e, "leave not pushed to a peer");
                    }
                }
            }
            Err(CoreError::Rejected(_)) => {}
            Err(e) => return Err(e),
        }
        let removed = self.write().communities.remove(&community);
        if let Some(c) = removed {
            self.stop_topic(c).await;
        }
        self.inner
            .neighbours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&community);
        self.inner
            .founded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&community);
        let dir = self.read().paths.community_dir(&community);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        self.prune_thumbnails();
        self.emit(EngineEvent::CommunitiesChanged);
        Ok(())
    }

    pub fn community_view(&self, community: CommunityId) -> Result<CommunityView> {
        let mut view = {
            let w = self.read();
            let c = w
                .communities
                .get(&community)
                .ok_or_else(|| not_found(&community))?;
            w.community_view(c)
        };
        view.voice_channels = self.voice_channel_views(community);
        Ok(view)
    }

    pub(crate) fn summary(&self, id: &CommunityId) -> Result<CommunitySummary> {
        let w = self.read();
        let c = w.communities.get(id).ok_or_else(|| not_found(id))?;
        Ok(w.summary(c))
    }

    /// Joins a community's gossip topic and keeps the sender on the community.
    pub(crate) async fn subscribe_topic(
        &self,
        id: CommunityId,
        bootstrap: Vec<DeviceId>,
    ) -> Result<TopicReceiver> {
        let secret = self.access_secret(id).ok_or_else(|| not_found(&id))?;
        let topic = TopicId::for_channel(&id, &crate::access::topic_salt(&secret));
        let (sender, receiver) = self.node().subscribe(topic, bootstrap).await?;
        let mut guard = self.write();
        let c = guard
            .communities
            .get_mut(&id)
            .ok_or_else(|| not_found(&id))?;
        c.sender = Some(Arc::new(sender));
        Ok(receiver)
    }

    /// Stops a community's receive loop, then leaves its topic with the sender and receiver.
    async fn stop_topic(&self, mut c: Community) {
        self.stop_parts(c.sender.take(), c.task.take()).await;
    }

    /// Stops a receive loop and leaves its topic, given the parts of a community's wiring.
    pub(crate) async fn stop_parts(&self, sender: Option<Arc<TopicSender>>, task: Option<ReceiverTask>) {
        let Some(task) = task else {
            return;
        };
        let ReceiverTask { handle, stop } = task;
        let _ = stop.send(());
        let abort = handle.abort_handle();
        let rx = match tokio::time::timeout(LEAVE_WAIT, handle).await {
            Ok(Ok(rx)) => Some(rx),
            _ => {
                abort.abort();
                None
            }
        };
        leave_topic(sender, rx);
    }

    /// Fetches the community from the invite's peers. If none of them answers, it tries the peers
    /// named in the community's DHT record. Returns the events and the peers that served them.
    async fn fetch_for_join(
        &self,
        genesis: &Genesis,
        secret: &[u8; 32],
        invited: &[PeerAddr],
        from: &PeerAddr,
    ) -> Result<(Fetched, Vec<PeerAddr>)> {
        let invite_err = if invited.is_empty() {
            "the invite lists no peer".to_string()
        } else {
            match self.fetch_invited(genesis, invited, from).await {
                Ok(fetched) => return Ok((fetched, invited.to_vec())),
                Err(e) => format!("the invite's peers did not answer: {e}"),
            }
        };
        let from_record = self.record_peers(&genesis.community, secret).await;
        if from_record.is_empty() {
            return Err(CoreError::NotFound(format!(
                "could not join: {invite_err}; the DHT has no usable record for this community"
            )));
        }
        match self.fetch_invited(genesis, &from_record, from).await {
            Ok(fetched) => Ok((fetched, from_record)),
            Err(e) => Err(CoreError::NotFound(format!(
                "could not join: {invite_err}; the DHT record's peers did not answer: {e}"
            ))),
        }
    }

    /// Turns a short invite into the parts of a long one. The DHT record, opened with the invite
    /// secret, names the peers and relays. The short form has no genesis, so it is fetched from
    /// those peers and checked against the community id, which the genesis is signed under.
    async fn resolve_short(&self, short: &ShortInvite) -> Result<Invite> {
        let peers = self.record_peers(&short.community, &short.secret).await;
        if peers.is_empty() {
            return Err(CoreError::NotFound(
                "the DHT has no record for this invite yet; try again shortly, or ask for a long link".into(),
            ));
        }
        let genesis = self.fetch_genesis(short.community, &peers).await?;
        Ok(Invite {
            genesis,
            peers: peers.iter().map(ToString::to_string).collect(),
            secret: short.secret,
            secret_generation: short.secret_generation,
            relays: Vec::new(),
            invite_id: short.invite_id,
        })
    }

    /// The community's genesis, from the first of `peers` that serves one it can verify.
    async fn fetch_genesis(&self, id: CommunityId, peers: &[PeerAddr]) -> Result<Genesis> {
        for peer in peers {
            match self.call(peer.id, &Request::Genesis { community: id }).await {
                Ok(Response::Genesis { genesis, .. }) if genesis.community == id && genesis.verify().is_ok() => {
                    return Ok(genesis)
                }
                Ok(_) => debug!(community = %id, peer = %peer.id, "peer did not serve a valid genesis"),
                Err(e) => debug!(community = %id, peer = %peer.id, error = %e, "genesis not fetched"),
            }
        }
        Err(CoreError::NotFound("no DHT peer served the community's genesis".into()))
    }

    /// The peers named in the community's DHT record. They are remembered, so they can be dialled.
    /// Empty when there is no DHT, no record, or the lookup fails.
    pub(crate) async fn record_peers(&self, id: &CommunityId, secret: &[u8; 32]) -> Vec<PeerAddr> {
        let Some(discovery) = self.inner.discovery.clone() else {
            return Vec::new();
        };
        // A DHT lookup can stall for a long time. A join should not wait on it indefinitely.
        let lookup = tokio::time::timeout(DHT_LOOKUP_WAIT, discovery.resolve(id, secret)).await;
        let record = match lookup {
            Ok(Ok(Some(record))) => record,
            Ok(Ok(None)) => return Vec::new(),
            Ok(Err(e)) => {
                debug!(community = %id, error = %e, "could not resolve community record");
                return Vec::new();
            }
            Err(_) => {
                debug!(community = %id, "community record lookup timed out");
                return Vec::new();
            }
        };
        let relay_urls: Vec<String> = record.relays.iter().map(|u| u.to_string()).collect();
        self.adopt_relays(&relay_urls).await;
        let peers: Vec<PeerAddr> = record
            .peers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .collect();
        let mut guard = self.write();
        for peer in &peers {
            self.learn_address(&mut guard, peer.clone());
        }
        peers
    }

    /// Asks the invite's peers for the community, returning every event they hold.
    async fn fetch_invited(
        &self,
        genesis: &Genesis,
        peers: &[PeerAddr],
        from: &PeerAddr,
    ) -> Result<Fetched> {
        for peer in peers {
            match self.fetch_community_from(genesis, peer.id, from).await {
                Ok(fetched) => return Ok(fetched),
                Err(e) => debug!(peer = %peer.id, error = %e, "invite peer could not serve the community"),
            }
        }
        Err(CoreError::NotFound(
            "no invite peer answered for this community".into(),
        ))
    }

    async fn fetch_community_from(
        &self,
        genesis: &Genesis,
        peer: DeviceId,
        from: &PeerAddr,
    ) -> Result<Fetched> {
        let id = genesis.community;
        let mut addrs = match self.call(peer, &Request::Genesis { community: id }).await? {
            Response::Genesis {
                genesis: got,
                addrs,
            } if got == *genesis => addrs,
            _ => {
                return Err(CoreError::Invalid(
                    "peer does not serve the invited community".into(),
                ))
            }
        };
        let reconciled = self.reconcile(id, peer, Vec::new(), from.clone()).await?;
        addrs.extend(reconciled.addrs);
        let mut events = Vec::new();
        self.fetch_into(id, peer, reconciled.need, |batch| {
            events.extend(batch);
            Ok(())
        })
        .await?;
        Ok(Fetched { events, addrs })
    }
}

/// What a join fetches from a peer: the community's events, and the signed addresses of its
/// members. The addresses are kept only once the events have made their membership known.
pub(crate) struct Fetched {
    events: Vec<SignedEvent>,
    addrs: Vec<SignedAddr>,
}

/// `addr` without its direct IP addresses: only the device id and relay URL go on the DHT.
pub(crate) fn public_addr(addr: &PeerAddr) -> PeerAddr {
    PeerAddr {
        id: addr.id,
        relay: addr.relay.clone(),
        direct: Vec::new(),
    }
}

/// Leaves a topic with its sender and receiver. Whatever cannot be paired is dropped, which
/// also leaves the topic once the last half is gone.
fn leave_topic(sender: Option<Arc<TopicSender>>, rx: Option<TopicReceiver>) {
    let tx = sender.and_then(|shared| Arc::try_unwrap(shared).ok());
    match (tx, rx) {
        (Some(tx), Some(rx)) => tx.leave(rx),
        _ => debug!("topic halves were still shared; dropping them"),
    }
}
