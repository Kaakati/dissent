//! A mesh call: every participant sends its audio to every other participant.
//!
//! The mesh holds one [`Call`] per remote peer. All of them share the call's media key, and each
//! peer's frames carry that peer's MLS member index. A frame is encrypted once and written to
//! every call.

use std::collections::BTreeMap;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Instant;

use crate::auth::{Auth, LinkConfig, RxState};
use crate::call::{Call, CallStats};
use crate::e2ee::{epoch_lsb, SframeSender, DEFAULT_LEVEL};
use crate::error::{MediaError, Result};
use crate::key::MediaKey;
use crate::signal::Signal;
use crate::transport::PacketTransport;

type Boxed<'a, O> = Pin<Box<dyn Future<Output = O> + Send + 'a>>;
type FrameOut = Result<Option<(u64, Vec<u8>)>>;

/// The audio mesh of one participant.
///
/// Connecting follows one rule so that two peers never offer at once. The peer with the lower
/// id calls [`Mesh::offer`], and the peer with the higher id calls [`Mesh::accept`]. Each side
/// then calls [`Mesh::apply`] with the other's answer, or the offer is consumed by `accept`.
///
/// Rekeying uses [`Mesh::set_key`]. Frames of the previous epoch keep decrypting for a short
/// grace period, see [`crate::e2ee`].
pub struct Mesh<T: PacketTransport> {
    my_id: u64,
    key: MediaKey,
    epoch: u64,
    tx: SframeSender,
    calls: BTreeMap<u64, Call<T>>,
    link: LinkConfig,
}

impl<T: PacketTransport> Mesh<T> {
    /// A mesh with no peers yet, keyed to epoch 0. `my_sender_id` is this participant's MLS
    /// member index.
    pub fn new(key: MediaKey, my_sender_id: u64) -> Result<Self> {
        let tx = SframeSender::new(&key, my_sender_id, 0)?;
        Ok(Self {
            my_id: my_sender_id,
            key,
            epoch: 0,
            tx,
            calls: BTreeMap::new(),
            link: LinkConfig {
                auth: None,
                state: Arc::new(Mutex::new(RxState::new(None))),
            },
        })
    }

    /// A mesh that signs its frames and checks every member's digests. `auth` holds this
    /// member's device key and the device certificate of every member.
    pub fn new_authenticated(key: MediaKey, my_sender_id: u64, auth: Auth) -> Result<Self> {
        let mut mesh = Self::new(key, my_sender_id)?;
        mesh.link = LinkConfig::authenticated(auth.clone(), LinkConfig::shared_state(&auth));
        mesh.link.auth = Some(auth);
        Ok(mesh)
    }

    /// The ids of the peers this mesh is connected to, in ascending order.
    pub fn peers(&self) -> Vec<u64> {
        self.calls.keys().copied().collect()
    }

    /// The key epoch new frames are sent under.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Starts a call to `peer` and returns the offer to send to it. Only the lower id offers.
    pub async fn offer(&mut self, peer: u64, transport: T) -> Result<Signal> {
        if self.my_id >= peer {
            return Err(MediaError::BadState("only the lower peer id offers"));
        }
        self.ensure_new_peer(peer)?;
        let (call, offer) = Call::offer_at(
            transport,
            self.key.clone(),
            self.my_id,
            self.epoch,
            self.link.clone(),
        )
        .await?;
        self.calls.insert(peer, call);
        Ok(offer)
    }

    /// Answers `peer`'s offer and returns the answer to send back. Only the higher id accepts.
    pub async fn accept(&mut self, peer: u64, transport: T, offer: Signal) -> Result<Signal> {
        if self.my_id <= peer {
            return Err(MediaError::BadState("only the higher peer id accepts"));
        }
        self.ensure_new_peer(peer)?;
        let (call, answer) = Call::accept_at(
            transport,
            self.key.clone(),
            self.my_id,
            self.epoch,
            offer,
            self.link.clone(),
        )
        .await?;
        self.calls.insert(peer, call);
        Ok(answer)
    }

    /// Applies the answer to an offer this mesh made to `peer`.
    pub fn apply(&mut self, peer: u64, signal: Signal) -> Result<()> {
        self.calls
            .get_mut(&peer)
            .ok_or(MediaError::BadState("no call with this peer"))?
            .apply(signal)
    }

    /// Drops the call to `peer`. Frames of this mesh stop reaching it at once. The caller should
    /// then rekey with [`Mesh::set_key`], so the peer cannot decrypt later frames either.
    pub fn remove_peer(&mut self, peer: u64) -> bool {
        self.calls.remove(&peer).is_some()
    }

    /// Replaces the media key, typically after the MLS group changes. Frames from now on are
    /// encrypted under `new_key` at `new_epoch`. Frames of the previous epoch still decrypt for
    /// the grace period, and the epoch before that no longer does.
    ///
    /// `new_epoch` must differ from the current epoch in its low key-id bits, which is 8 bits.
    pub fn set_key(&mut self, new_key: MediaKey, new_epoch: u64) -> Result<()> {
        if epoch_lsb(new_epoch) == epoch_lsb(self.epoch) {
            return Err(MediaError::BadState(
                "the new epoch must differ from the current one in its key id bits",
            ));
        }
        let tx = SframeSender::new(&new_key, self.my_id, new_epoch)?;
        let now = Instant::now();
        for call in self.calls.values_mut() {
            call.rotate_keys(new_key.clone(), new_epoch, now)?;
        }
        self.tx = tx;
        self.key = new_key;
        self.epoch = new_epoch;
        Ok(())
    }

    /// Waits until every peer's ICE and DTLS complete. Frames that arrive meanwhile are kept.
    pub async fn wait_connected(&mut self) -> Result<()> {
        let mut pending: Vec<Option<Boxed<'_, Result<()>>>> = self
            .calls
            .values_mut()
            .map(|call| Some(Box::pin(call.wait_connected()) as Boxed<'_, Result<()>>))
            .collect();
        poll_fn(|cx| {
            let mut all_done = true;
            for slot in pending.iter_mut() {
                if let Some(fut) = slot.as_mut() {
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(Ok(())) => *slot = None,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => all_done = false,
                    }
                }
            }
            if all_done {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }

    /// Encrypts one frame once and sends it to every peer.
    ///
    /// Fails with [`MediaError::NotConnected`] while any peer is still connecting, so no peer
    /// misses the start of the stream. Does nothing when the mesh has no peers. A peer whose
    /// link fails is dropped from the mesh, and the frame still goes to the others.
    pub async fn send_audio_frame(&mut self, frame: &[u8]) -> Result<()> {
        if self.calls.is_empty() {
            return Ok(());
        }
        if !self.calls.values().all(Call::is_connected) {
            return Err(MediaError::NotConnected);
        }
        let encrypted = self.tx.encrypt(frame, DEFAULT_LEVEL)?;
        let mut failed = Vec::new();
        for (peer, call) in self.calls.iter_mut() {
            if let Err(e) = call.send_encrypted(&encrypted).await {
                tracing::debug!(peer, error = %e, "dropping peer after a send failure");
                failed.push(*peer);
            }
        }
        for peer in failed {
            self.calls.remove(&peer);
        }
        Ok(())
    }

    /// Waits for the next frame from any peer and returns `(sender id, plaintext)`.
    ///
    /// Returns `Ok(None)` when no peers are left. A peer whose call closes or fails is dropped
    /// from the mesh, and the others keep delivering. This method is cancel-safe.
    pub async fn next_frame(&mut self) -> Result<Option<(u64, Vec<u8>)>> {
        loop {
            if self.calls.is_empty() {
                return Ok(None);
            }
            let (peer, outcome) = {
                let mut futs: Vec<(u64, Boxed<'_, FrameOut>)> = self
                    .calls
                    .iter_mut()
                    .map(|(peer, call)| (*peer, Box::pin(call.next_frame()) as Boxed<'_, FrameOut>))
                    .collect();
                poll_fn(|cx| {
                    for (peer, fut) in futs.iter_mut() {
                        if let Poll::Ready(out) = fut.as_mut().poll(cx) {
                            return Poll::Ready((*peer, out));
                        }
                    }
                    Poll::Pending
                })
                .await
            };
            match outcome {
                Ok(Some(frame)) => return Ok(Some(frame)),
                Ok(None) => {
                    self.calls.remove(&peer);
                }
                Err(e) => {
                    tracing::debug!(peer, error = %e, "dropping peer after a receive failure");
                    self.calls.remove(&peer);
                }
            }
        }
    }

    /// The counters of each peer's call, keyed by peer id.
    pub fn stats(&self) -> BTreeMap<u64, CallStats> {
        self.calls
            .iter()
            .map(|(peer, call)| (*peer, call.stats()))
            .collect()
    }

    fn ensure_new_peer(&self, peer: u64) -> Result<()> {
        if self.calls.contains_key(&peer) {
            return Err(MediaError::BadState("already connected to this peer"));
        }
        Ok(())
    }
}
