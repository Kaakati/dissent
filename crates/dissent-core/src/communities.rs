//! Community lifecycle: create, invite, join, leave, and the read side of one community.

use crate::community::Community;
use crate::engine::{clean_text, not_found, now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::net::ReceiverTask;
use crate::rpc::{Request, Response};
use crate::store::{write_bin, write_bytes_atomic};
use crate::views::{CommunitySummary, CommunityView, EngineEvent};
use dissent_discovery::{CommunityRecord, Discovery, Invite};
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

        if let Some(discovery) = self.inner.discovery.clone() {
            let peers = self.read().my_addr.iter().map(|a| a.to_string()).collect();
            let record = CommunityRecord {
                name,
                peers,
                relays: Vec::new(),
            };
            self.spawn_task(publish_record(discovery, key, record));
        }
        self.emit(EngineEvent::CommunitiesChanged);
        self.summary(&id)
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
        let peers: Vec<PeerAddr> = invite
            .peers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .collect();
        if peers.is_empty() {
            return Err(CoreError::Invalid("invite lists no usable peer address".into()));
        }
        {
            let mut guard = self.write();
            let w = &mut *guard;
            for peer in &peers {
                self.learn_address(w, peer.clone());
            }
        }

        let events = self.fetch_invited(&invite.genesis, &peers, &my_addr).await?;
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

/// Sends the community's DHT record. Failure is logged, not returned.
async fn publish_record(discovery: Arc<Discovery>, key: CommunityKey, record: CommunityRecord) {
    if let Err(e) = discovery.publish(&key, &record).await {
        warn!(error = %e, "could not publish community record to the DHT");
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
