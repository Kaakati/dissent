//! Community lifecycle: create, invite, join, leave, and the read side of one community.

use crate::community::Community;
use crate::engine::{clean_text, not_found, now_ms, Engine};
use std::sync::atomic::Ordering;
use crate::error::{CoreError, Result};
use crate::net::ReceiverTask;
use crate::rpc::{Request, Response};
use crate::store::{write_bin, write_bytes_atomic};
use crate::views::{CommunitySummary, CommunityView, EngineEvent};
use dissent_discovery::{CommunityRecord, Invite};
use dissent_events::{ChannelId, Payload, SignedEvent};
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
const DHT_LOOKUP_WAIT: Duration = Duration::from_secs(30);

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
            write_bytes_atomic(&paths.community_key(&id), &key.0.seed())?;
            guard.communities.insert(id, Community::new(genesis));
        }
        self.inner
            .founded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, key.0.seed());
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
        let jobs: Vec<(CommunityKey, CommunityRecord)> = {
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
                .filter_map(|(id, seed)| {
                    let c = w.communities.get(id)?;
                    let record = CommunityRecord {
                        name: c.name().to_string(),
                        peers: vec![public_addr(&addr).to_string()],
                        relays: Vec::new(),
                    };
                    Some((CommunityKey::from_seed(*seed), record))
                })
                .collect()
        };
        let mut all_ok = true;
        for (key, record) in jobs {
            let published = discovery.publish(&key, &record).await;
            if let Err(e) = published {
                all_ok = false;
                warn!(error = %e, "could not publish community record to the DHT");
            }
        }
        self.inner.records_current.store(all_ok, Ordering::Relaxed);
    }

    /// An invite link for `community`, carrying our current address.
    pub async fn create_invite(&self, community: CommunityId) -> Result<String> {
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
        let invite = Invite {
            genesis,
            peers: vec![addr.to_string()],
            secret: None,
        };
        Ok(invite.to_url().to_string())
    }

    /// Joins from an invite link. Joining a community we already have just returns its summary.
    pub async fn join_community(&self, invite: &str) -> Result<CommunitySummary> {
        let invite = Invite::parse(invite)?;
        let id = invite.community();
        if self.read().communities.contains_key(&id) {
            return self.summary(&id);
        }
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

        let (events, peers) = self.fetch_for_join(&invite.genesis, &invited, &my_addr).await?;
        {
            let mut guard = self.write();
            write_bin(&guard.paths.genesis(&id), &invite.genesis)?;
            guard
                .communities
                .insert(id, Community::new(invite.genesis.clone()));
        }
        self.ingest(id, events)?;

        let mut bootstrap: Vec<DeviceId> = peers.iter().map(|p| p.id).collect();
        bootstrap.extend(self.bootstrap_for(&id));
        bootstrap.sort();
        bootstrap.dedup();
        let mut topic_rx = self.subscribe_topic(id, bootstrap).await?;
        // Announce only once a neighbour is up. Otherwise the Join would reach nobody.
        let _ = tokio::time::timeout(JOIN_GOSSIP_WAIT, topic_rx.joined()).await;
        self.spawn_receiver(id, topic_rx);

        let display_name = self.read().my_name();
        match self
            .author_publish(id, ChannelId::CONTROL, Payload::Join { display_name })
            .await
        {
            Ok(join) => {
                // Hand the Join straight to the peers that served the invite. Gossip may reach
                // them later, and until it does they cannot address us.
                for peer in &peers {
                    if let Err(e) = self.push(id, peer.id, vec![join.clone()]).await {
                        debug!(community = %id, peer = %peer.id, error = %e, "join not pushed to invite peer");
                    }
                }
            }
            Err(e) => warn!(community = %id, error = %e, "could not announce join"),
        }
        self.emit(EngineEvent::CommunitiesChanged);
        self.summary(&id)
    }

    /// Announces leaving, leaves the gossip topic, then forgets the community and deletes its
    /// local data.
    pub async fn leave_community(&self, community: CommunityId) -> Result<()> {
        self.summary(&community)?;
        match self
            .author_publish(community, ChannelId::CONTROL, Payload::Leave)
            .await
        {
            Ok(_) | Err(CoreError::Rejected(_)) => {}
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
        self.emit(EngineEvent::CommunitiesChanged);
        Ok(())
    }

    pub fn community_view(&self, community: CommunityId) -> Result<CommunityView> {
        let w = self.read();
        let c = w
            .communities
            .get(&community)
            .ok_or_else(|| not_found(&community))?;
        Ok(w.community_view(c))
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
        let topic = TopicId::for_channel(&id, &ChannelId::CONTROL.0);
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
        let sender = c.sender.take();
        let Some(task) = c.task.take() else {
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
        invited: &[PeerAddr],
        from: &PeerAddr,
    ) -> Result<(Vec<SignedEvent>, Vec<PeerAddr>)> {
        let invite_err = if invited.is_empty() {
            "the invite lists no peer".to_string()
        } else {
            match self.fetch_invited(genesis, invited, from).await {
                Ok(events) => return Ok((events, invited.to_vec())),
                Err(e) => format!("the invite's peers did not answer: {e}"),
            }
        };
        let from_record = self.record_peers(&genesis.community).await;
        if from_record.is_empty() {
            return Err(CoreError::NotFound(format!(
                "could not join: {invite_err}; the DHT has no usable record for this community"
            )));
        }
        match self.fetch_invited(genesis, &from_record, from).await {
            Ok(events) => Ok((events, from_record)),
            Err(e) => Err(CoreError::NotFound(format!(
                "could not join: {invite_err}; the DHT record's peers did not answer: {e}"
            ))),
        }
    }

    /// The peers named in the community's DHT record. They are remembered, so they can be dialled.
    /// Empty when there is no DHT, no record, or the lookup fails.
    async fn record_peers(&self, id: &CommunityId) -> Vec<PeerAddr> {
        let Some(discovery) = self.inner.discovery.clone() else {
            return Vec::new();
        };
        // A DHT lookup can stall for a long time. A join should not wait on it indefinitely.
        let lookup = tokio::time::timeout(DHT_LOOKUP_WAIT, discovery.resolve(id)).await;
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
    ) -> Result<Vec<SignedEvent>> {
        for peer in peers {
            match self.fetch_community_from(genesis, peer.id, from).await {
                Ok(events) => return Ok(events),
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
    ) -> Result<Vec<SignedEvent>> {
        let id = genesis.community;
        match self.call(peer, &Request::Genesis { community: id }).await? {
            Response::Genesis(got) if got == *genesis => {}
            _ => {
                return Err(CoreError::Invalid(
                    "peer does not serve the invited community".into(),
                ))
            }
        }
        let (need, _) = self.reconcile(id, peer, Vec::new(), from.clone()).await?;
        let mut events = Vec::new();
        self.fetch_into(id, peer, need, |batch| {
            events.extend(batch);
            Ok(())
        })
        .await?;
        Ok(events)
    }
}

/// `addr` without its direct IP addresses: only the device id and relay URL go on the DHT.
fn public_addr(addr: &PeerAddr) -> PeerAddr {
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
