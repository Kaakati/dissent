//! Sender authentication for SFrame frames.
//!
//! SFrame alone cannot tell members apart, because every member holds the same media key
//! (RFC 9605 section 7.2). This layer adds a signature by each sender's device key.
//!
//! The sender hashes every encrypted frame. After every ten frames, or 200 ms, whichever comes
//! first, it sends a digest: the hashes of those frames, signed with its device key. Receivers
//! play frames as they arrive, then check each one against the next digest from its sender.
//!
//! - A frame whose hash matches its digest is verified. Its counter then enters the replay
//!   window.
//! - A frame that no digest covers within 500 ms counts as unauthenticated. A lost digest causes
//!   this, so it is tolerated and never mutes anyone.
//! - A frame whose hash differs from the digest for its counter is a forgery. Its sender is
//!   flagged as impersonated and muted for the rest of the epoch. The real frame, if it arrives,
//!   still plays once its digest verifies.
//!
//! A frame more than [`AHEAD_SLACK`] counters past the last verified counter is held until a
//! digest covers it, so a far-ahead forgery never plays.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dissent_identity::{DeviceCert, DeviceKey, Signature};

use crate::e2ee::ReplayWindow;

/// Marks a digest packet on the wire. It is the first byte, where a frame has its audio level.
pub(crate) const DIGEST_MARK: u8 = 0xFF;
/// Domain separation for the digest signature.
const DOMAIN: &[u8] = b"dissent/sframe-auth/v1";
/// Bytes of each frame hash kept in a digest.
pub(crate) const HASH_LEN: usize = 8;
/// Frames per digest for audio: 25 frames is 500 ms, which keeps the signature cost low.
pub const AUDIO_DIGEST_FRAMES: usize = 25;
/// Frames per digest for video, which must be checked sooner.
pub const VIDEO_DIGEST_FRAMES: usize = 10;
/// Audio frames are 20 ms apart.
const FRAME_MS: u64 = 20;
/// How long a batch of `frames` frames takes to fill. A batch also closes at this age.
pub(crate) fn batch_span(frames: usize) -> Duration {
    Duration::from_millis(FRAME_MS * frames as u64)
}

/// How long a frame may wait for its digest after its batch closes, before it counts as
/// unauthenticated. The batch's own span is added on top, so a long batch is not penalised.
pub(crate) const PENDING_DEADLINE: Duration = Duration::from_millis(500);
/// How far past the last verified counter a frame may be before it is held.
pub(crate) const AHEAD_SLACK: u64 = 50;
/// Earlier starts of sending remembered per sender and key id. Contexts are random, so one older than these cannot be
/// told from a new one. A session is retired only by a digest its sender signed, so only the sender's own sessions push
/// one out: replaying an old session takes this many later ones in one MLS epoch, and a place on the media path.
pub(crate) const MAX_RETIRED_SESSIONS: usize = 32;
/// Key ids this link takes under the current key until the next rekey. An honest call has one or two.
pub(crate) const MAX_ADOPTED_IDS: usize = 4;
/// Senders with no certificate this link keeps state for. A member holds the key, so it can name any sender id.
pub(crate) const MAX_UNKNOWN_SENDERS: usize = 64;

/// The authentication a participant brings to a call: its device key, and the device
/// certificate of every member, keyed by member index.
#[derive(Clone)]
pub struct Auth {
    pub(crate) device: DeviceKey,
    pub(crate) certs: Arc<HashMap<u64, DeviceCert>>,
    pub(crate) digest_frames: usize,
}

impl Auth {
    /// `device` signs this participant's frames. `certs` maps each member's index to its
    /// device certificate, so every member's digests can be checked.
    pub fn new(device: DeviceKey, certs: HashMap<u64, DeviceCert>) -> Self {
        Self {
            device,
            certs: Arc::new(certs),
            digest_frames: AUDIO_DIGEST_FRAMES,
        }
    }

    /// Sets how many frames each digest covers. The default is 25 for audio, about 500 ms.
    /// Shorter batches verify sooner and cost more signatures.
    pub fn with_digest_frames(mut self, frames: usize) -> Self {
        self.digest_frames = frames.clamp(1, 255);
        self
    }

    /// Digests for video: 10 frames, so a forgery is caught sooner than audio allows.
    pub fn for_video(self) -> Self {
        self.with_digest_frames(VIDEO_DIGEST_FRAMES)
    }

    /// How many frames each digest covers.
    pub fn digest_frames(&self) -> usize {
        self.digest_frames
    }
}

/// Adds or replaces a member's certificate in a shared receiver state. Fails on an
/// unauthenticated state, which has no roster to update.
pub(crate) fn learn_into(
    state: &Mutex<RxState>,
    sender: u64,
    cert: DeviceCert,
) -> crate::error::Result<()> {
    let mut state = state.lock().expect("receiver state");
    if !state.is_authenticated() {
        return Err(crate::error::MediaError::BadState(
            "an unauthenticated link has no certificates to learn",
        ));
    }
    state.learn_cert(sender, cert);
    Ok(())
}

/// Stops verifying a member's digests in a shared receiver state.
pub(crate) fn forget_into(state: &Mutex<RxState>, sender: u64) {
    state.lock().expect("receiver state").forget_cert(sender);
}

/// The receiver-side state of one link, or shared by several links of one participant.
#[derive(Clone)]
pub(crate) struct LinkConfig {
    /// Present when this participant signs its frames and checks others' digests.
    pub(crate) auth: Option<Auth>,
    /// Replay windows, pending frames and mute flags, shared across the participant's links.
    pub(crate) state: Arc<Mutex<RxState>>,
}

impl LinkConfig {
    /// No authentication. Frames are decrypted and replay-checked, as in the unauthenticated path.
    pub(crate) fn plain() -> Self {
        Self {
            auth: None,
            state: Arc::new(Mutex::new(RxState::new(None))),
        }
    }

    /// Authenticated, with a receiver state shared by every link of one participant.
    pub(crate) fn authenticated(auth: Auth, state: Arc<Mutex<RxState>>) -> Self {
        Self {
            auth: Some(auth),
            state,
        }
    }

    /// A fresh receiver state for a participant with the given authentication, to be shared by
    /// all of its links.
    pub(crate) fn shared_state(auth: &Auth) -> Arc<Mutex<RxState>> {
        Arc::new(Mutex::new(RxState::with_deadline(
            Some((*auth.certs).clone()),
            batch_span(auth.digest_frames) + PENDING_DEADLINE,
        )))
    }
}

/// Hash of a wire frame, as the digest carries it.
pub(crate) fn frame_hash(wire: &[u8]) -> [u8; HASH_LEN] {
    let digest = blake3::hash(wire);
    let mut out = [0u8; HASH_LEN];
    out.copy_from_slice(&digest.as_bytes()[..HASH_LEN]);
    out
}

/// The sender a digest names, read without any key.
pub(crate) fn digest_sender(wire: &[u8]) -> Option<u64> {
    if !is_digest(wire) {
        return None;
    }
    Some(u64::from_le_bytes(wire.get(1..9)?.try_into().ok()?))
}

/// Whether a wire packet is a digest rather than a frame.
pub(crate) fn is_digest(wire: &[u8]) -> bool {
    wire.first() == Some(&DIGEST_MARK)
}

/// The digest fields that the signature covers, after the mark and before the signature.
fn fields(sender: u64, epoch: u8, first: u64, hashes: &[[u8; HASH_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(18 + hashes.len() * HASH_LEN);
    out.extend_from_slice(&sender.to_le_bytes());
    out.push(epoch);
    out.extend_from_slice(&first.to_le_bytes());
    out.push(hashes.len() as u8);
    for hash in hashes {
        out.extend_from_slice(hash);
    }
    out
}

fn signed_bytes(fields: &[u8]) -> Vec<u8> {
    [DOMAIN, fields].concat()
}

/// Builds a digest packet: the mark, the fields, then a 64-byte signature.
pub(crate) fn encode_digest(
    device: &DeviceKey,
    sender: u64,
    epoch: u8,
    first: u64,
    hashes: &[[u8; HASH_LEN]],
) -> Vec<u8> {
    let body = fields(sender, epoch, first, hashes);
    let sig = device.sign(&signed_bytes(&body));
    let mut out = Vec::with_capacity(1 + body.len() + sig.0.len());
    out.push(DIGEST_MARK);
    out.extend_from_slice(&body);
    out.extend_from_slice(&sig.0);
    out
}

struct ParsedDigest {
    sender: u64,
    epoch: u8,
    first: u64,
    hashes: Vec<[u8; HASH_LEN]>,
    body: Vec<u8>,
    sig: Signature,
}

fn parse_digest(wire: &[u8]) -> Option<ParsedDigest> {
    const HEADER: usize = 18; // sender, epoch, first, count
    if !is_digest(wire) || wire.len() < 1 + HEADER + 64 {
        return None;
    }
    let body = &wire[1..wire.len() - 64];
    let sig: [u8; 64] = wire[wire.len() - 64..].try_into().ok()?;
    let sender = u64::from_le_bytes(body[0..8].try_into().ok()?);
    let epoch = body[8];
    let first = u64::from_le_bytes(body[9..17].try_into().ok()?);
    let count = body[17] as usize;
    if count == 0 || body.len() != HEADER + count * HASH_LEN {
        return None;
    }
    let hashes = body[HEADER..].as_chunks::<HASH_LEN>().0.to_vec();
    Some(ParsedDigest {
        sender,
        epoch,
        first,
        hashes,
        body: body.to_vec(),
        sig: Signature(sig),
    })
}

/// The device certificate must chain to a root, and the device must have signed the digest.
fn verify_digest(parsed: &ParsedDigest, cert: &DeviceCert) -> bool {
    cert.verify().is_ok()
        && cert
            .device
            .verify(&signed_bytes(&parsed.body), &parsed.sig)
            .is_ok()
}

/// The sender side: batches the hashes of this participant's frames and signs them.
pub(crate) struct FrameSigner {
    device: DeviceKey,
    sender: u64,
    epoch: u8,
    batch: Vec<[u8; HASH_LEN]>,
    first: u64,
    last: Option<u64>,
    opened: Option<Instant>,
    frames: usize,
}

impl FrameSigner {
    pub(crate) fn new(device: DeviceKey, sender: u64, epoch: u8, frames: usize) -> Self {
        Self {
            device,
            sender,
            epoch,
            frames,
            batch: Vec::new(),
            first: 0,
            last: None,
            opened: None,
        }
    }

    /// Records one frame sent with `counter`, and returns the digests that closed. A batch also
    /// closes when a counter skips, so each digest covers a run of consecutive counters.
    pub(crate) fn record(&mut self, counter: u64, wire: &[u8], now: Instant) -> Vec<Vec<u8>> {
        let mut closed = Vec::new();
        if self.last.is_some_and(|last| counter != last + 1) {
            closed.extend(self.flush());
        }
        if self.batch.is_empty() {
            self.first = counter;
            self.opened = Some(now);
        }
        self.batch.push(frame_hash(wire));
        self.last = Some(counter);
        let aged = self
            .opened
            .is_some_and(|opened| now.duration_since(opened) >= batch_span(self.frames));
        if self.batch.len() >= self.frames || aged {
            closed.extend(self.flush());
        }
        closed
    }

    /// When the open batch must close, if one is open.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.opened.map(|opened| opened + batch_span(self.frames))
    }

    /// Closes the open batch and returns its digest, if it has frames.
    pub(crate) fn flush(&mut self) -> Vec<Vec<u8>> {
        if self.batch.is_empty() {
            return Vec::new();
        }
        let packet = encode_digest(
            &self.device,
            self.sender,
            self.epoch,
            self.first,
            &self.batch,
        );
        self.batch.clear();
        self.opened = None;
        vec![packet]
    }

    /// Starts a new epoch. The open batch of the old epoch is dropped unsent. Its frames count as
    /// unauthenticated at the receivers, which tolerates a missing digest.
    pub(crate) fn set_epoch(&mut self, epoch: u8) {
        self.batch.clear();
        self.opened = None;
        self.last = None;
        self.epoch = epoch;
    }
}

/// A frame waiting for its digest.
struct Pending {
    hash: [u8; HASH_LEN],
    /// The plaintext, while the frame is held and not yet played.
    held: Option<Vec<u8>>,
    arrived: Instant,
}

impl Step {
    /// Adds another step's results to this one.
    pub(crate) fn extend(&mut self, other: Step) {
        self.deliver.extend(other.deliver);
        self.replayed += other.replayed;
        self.unauthenticated += other.unauthenticated;
        self.impersonations += other.impersonations;
        self.retired += other.retired;
        self.capped += other.capped;
    }
}

/// Receiver state for one (sender, epoch).
#[derive(Default)]
struct SenderRx {
    window: ReplayWindow,
    pending: BTreeMap<u64, Vec<Pending>>,
    verified_high: Option<u64>,
    muted: bool,
    /// The sender's start of sending this state is for. Each start counts from 0, so it has its own window.
    context: u64,
    /// Starts of sending this one replaced, whose frames are refused. See [`MAX_RETIRED_SESSIONS`].
    retired: VecDeque<u64>,
    /// A start of sending not yet proven to be the sender's. See [`RxState::on_frame_in`].
    candidate: Option<Box<SenderRx>>,
    /// This session is such a candidate, so its frames wait for a digest.
    unproven: bool,
}

impl SenderRx {
    /// Makes `next` the sender's session, keeping the record of this one and of the earlier ones.
    fn retire_for(&mut self, next: SenderRx) {
        let (muted, old, mut retired) = (self.muted, self.context, std::mem::take(&mut self.retired));
        retired.push_back(old);
        if retired.len() > MAX_RETIRED_SESSIONS {
            retired.pop_front();
        }
        *self = next;
        self.unproven = false;
        self.muted = muted;
        self.retired = retired;
        self.candidate = None;
    }

    /// Takes a decrypted frame of this session.
    #[allow(clippy::too_many_arguments)]
    fn take_frame(&mut self, authenticated: bool, sender: u64, counter: u64, hash: [u8; HASH_LEN], plaintext: Vec<u8>, now: Instant, step: &mut Step) {
        if !self.window.accepts(counter) {
            step.replayed += 1;
            return;
        }
        if !authenticated {
            // Without authentication the window is the only guard, so it advances at once.
            self.window.accept(counter);
            step.deliver.push((sender, plaintext));
            return;
        }
        let seen = self
            .pending
            .get(&counter)
            .is_some_and(|list| list.iter().any(|p| p.hash == hash));
        if seen {
            step.replayed += 1;
            return;
        }
        let far_ahead = self
            .verified_high
            .is_some_and(|high| counter > high + AHEAD_SLACK);
        let hold = self.muted || far_ahead || self.unproven;
        let held = if hold { Some(plaintext.clone()) } else { None };
        self.pending.entry(counter).or_default().push(Pending {
            hash,
            held,
            arrived: now,
        });
        if !hold {
            step.deliver.push((sender, plaintext));
        }
    }

    /// Drops pending frames past their deadline, and returns how many were never verified.
    fn expire(&mut self, now: Instant, deadline: Duration) -> u64 {
        let mut expired = 0;
        self.pending.retain(|_, list| {
            list.retain(|p| {
                let late = now.duration_since(p.arrived) > deadline;
                if late {
                    expired += 1;
                }
                !late
            });
            !list.is_empty()
        });
        expired
    }
}

/// What processing one frame or digest produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Step {
    /// Plaintexts to play now, with their sender.
    pub(crate) deliver: Vec<(u64, Vec<u8>)>,
    /// Frames rejected as replays, or as older than the replay window.
    pub(crate) replayed: u64,
    /// Frames that no digest verified in time, or that a digest never covered.
    pub(crate) unauthenticated: u64,
    /// Senders newly flagged as impersonated by a mismatched frame.
    pub(crate) impersonations: u64,
    /// Frames of a start of sending already replaced, counted in `replayed` too.
    pub(crate) retired: u64,
    /// Frames of a new sender this link has no room for.
    pub(crate) capped: u64,
}

/// Receiver-side authentication and replay state, shared by a participant's links.
pub(crate) struct RxState {
    /// The device certificate of every member, when the link authenticates. Members can join and
    /// leave, so this changes through `learn_cert` and `forget_cert`.
    certs: Option<HashMap<u64, DeviceCert>>,
    senders: BTreeMap<(u64, u64), SenderRx>,
    /// Epoch bits this link accepts, the current one and the previous one during its grace period.
    epochs: BTreeSet<u64>,
    /// Key ids of frames that opened under the current key: they last as long as that key, whatever their id's epoch
    /// does. See `SframeReceiver::open`.
    adopted: BTreeSet<u64>,
    deadline: Duration,
}

impl RxState {
    pub(crate) fn new(certs: Option<HashMap<u64, DeviceCert>>) -> Self {
        Self::with_deadline(certs, PENDING_DEADLINE)
    }

    pub(crate) fn with_deadline(certs: Option<HashMap<u64, DeviceCert>>, deadline: Duration) -> Self {
        Self {
            certs,
            senders: BTreeMap::new(),
            epochs: BTreeSet::new(),
            adopted: BTreeSet::new(),
            deadline,
        }
    }

    /// Whether this state verifies digests. An unauthenticated link has no certificates.
    pub(crate) fn is_authenticated(&self) -> bool {
        self.certs.is_some()
    }

    /// Starts verifying `sender`'s digests with `cert`, replacing any earlier one.
    pub(crate) fn learn_cert(&mut self, sender: u64, cert: DeviceCert) {
        if let Some(certs) = self.certs.as_mut() {
            certs.insert(sender, cert);
        }
    }

    /// Stops verifying `sender`'s digests, and forgets its replay and pending state, so an id
    /// reused by a later member starts clean.
    pub(crate) fn forget_cert(&mut self, sender: u64) {
        if let Some(certs) = self.certs.as_mut() {
            certs.remove(&sender);
        }
        self.senders.retain(|(id, _), _| *id != sender);
    }

    /// Whether this link keeps state for a new `sender`.
    fn has_room_for(&self, sender: u64) -> bool {
        let known = |id: &u64| self.certs.as_ref().is_some_and(|certs| certs.contains_key(id));
        if known(&sender) {
            return true;
        }
        let unknown: BTreeSet<u64> = self.senders.keys().map(|(id, _)| *id).filter(|id| !known(id)).collect();
        unknown.contains(&sender) || unknown.len() < MAX_UNKNOWN_SENDERS
    }

    /// Handles a decrypted frame.
    ///
    /// A start of sending counts from 0 again, so it needs its own replay window. A frame of a context not seen yet is
    /// kept apart as a candidate and does not play. It takes over when a digest its sender signed covers one of its
    /// frames, which a member naming that sender cannot make, and its frames then play. Until then the sender's session
    /// stands, so someone who comes back is heard about half a second late.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_frame_in(
        &mut self,
        sender: u64,
        lsb: u64,
        context: u64,
        counter: u64,
        hash: [u8; HASH_LEN],
        plaintext: Vec<u8>,
        now: Instant,
    ) -> Step {
        let authenticated = self.certs.is_some();
        let deadline = self.deadline;
        let fresh = !self.senders.contains_key(&(sender, lsb));
        if fresh && !self.has_room_for(sender) {
            return Step { capped: 1, ..Step::default() };
        }
        let state = self.senders.entry((sender, lsb)).or_default();
        if fresh {
            state.context = context;
        }
        let mut step = Step {
            unauthenticated: state.expire(now, deadline),
            ..Step::default()
        };
        if context == state.context {
            state.take_frame(authenticated, sender, counter, hash, plaintext, now, &mut step);
            return step;
        }
        if state.retired.contains(&context) {
            step.replayed = 1;
            step.retired = 1;
            return step;
        }
        if !authenticated {
            // Nothing can prove a session here, so a new one takes over at once.
            state.retire_for(SenderRx { context, ..SenderRx::default() });
            state.take_frame(authenticated, sender, counter, hash, plaintext, now, &mut step);
            return step;
        }
        let candidate = match &mut state.candidate {
            Some(candidate) if candidate.context == context => candidate,
            slot => slot.insert(Box::new(SenderRx { context, unproven: true, ..SenderRx::default() })),
        };
        step.unauthenticated += candidate.expire(now, deadline);
        candidate.take_frame(authenticated, sender, counter, hash, plaintext, now, &mut step);
        step
    }

    /// Handles a frame recovered from the redundancy of another frame. A copy the state already
    /// holds is dropped quietly, and is not counted as a replay, because redundancy sends copies
    /// on purpose.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_redundant(
        &mut self,
        sender: u64,
        lsb: u64,
        context: u64,
        counter: u64,
        hash: [u8; HASH_LEN],
        plaintext: Vec<u8>,
        now: Instant,
    ) -> Step {
        let known = self.senders.get(&(sender, lsb)).is_some_and(|state| {
            let session = match &state.candidate {
                _ if state.context == context => Some(state),
                Some(candidate) if candidate.context == context => Some(&**candidate),
                _ => None,
            };
            session.is_some_and(|session| {
                !session.window.accepts(counter)
                    || session
                        .pending
                        .get(&counter)
                        .is_some_and(|list| list.iter().any(|p| p.hash == hash))
            })
        });
        if known {
            return Step::default();
        }
        self.on_frame_in(sender, lsb, context, counter, hash, plaintext, now)
    }
    /// Handles a digest packet from a sender.
    pub(crate) fn on_digest(&mut self, wire: &[u8], now: Instant) -> Step {
        let mut step = Step::default();
        let deadline = self.deadline;
        let Some(parsed) = parse_digest(wire) else {
            return step;
        };
        let Some(cert) = self
            .certs
            .as_ref()
            .and_then(|certs| certs.get(&parsed.sender))
            .cloned()
        else {
            return step;
        };
        if !self.epochs.contains(&(parsed.epoch as u64)) {
            // A digest for an epoch this link does not accept, current or in grace, is ignored.
            return step;
        }

        let state = self
            .senders
            .entry((parsed.sender, parsed.epoch as u64))
            .or_default();
        step.unauthenticated += state.expire(now, deadline);

        if !verify_digest(&parsed, &cert) {
            // A digest that does not verify is a forgery in the sender's name.
            if !state.muted {
                step.impersonations = 1;
            }
            state.muted = true;
            return step;
        }

        // A digest the sender signed covers a frame of a new start of sending: that session is the sender's now.
        let covers = |session: &SenderRx| {
            parsed.hashes.iter().enumerate().any(|(offset, hash)| {
                session.pending.get(&(parsed.first + offset as u64)).is_some_and(|list| list.iter().any(|p| p.hash == *hash))
            })
        };
        if !covers(state) && state.candidate.as_deref().is_some_and(covers) {
            if let Some(candidate) = state.candidate.take() {
                // What stood as the session has another frame at a counter the sender signed: it was not the sender.
                let forged = (0..parsed.hashes.len()).any(|offset| state.pending.contains_key(&(parsed.first + offset as u64)));
                if forged {
                    if !state.muted {
                        step.impersonations += 1;
                    }
                    state.muted = true;
                }
                state.retire_for(*candidate);
            }
        }
        // A frame of another context at a counter this digest covers, with another hash, is sealed in the sender's name.
        // It never played. It is the same forgery as a mismatch in the session itself, and is treated the same.
        if let Some(candidate) = state.candidate.as_mut() {
            let mut forged = false;
            for offset in 0..parsed.hashes.len() {
                forged |= candidate.pending.remove(&(parsed.first + offset as u64)).is_some();
            }
            if forged {
                if !state.muted {
                    step.impersonations += 1;
                }
                state.muted = true;
            }
        }

        // Frames before this digest's range were never covered by it.
        let stale: Vec<u64> = state
            .pending
            .range(..parsed.first)
            .map(|(c, _)| *c)
            .collect();
        for counter in stale {
            if let Some(list) = state.pending.remove(&counter) {
                step.unauthenticated += list.len() as u64;
            }
        }

        for (offset, hash) in parsed.hashes.iter().enumerate() {
            let counter = parsed.first + offset as u64;
            let Some(list) = state.pending.remove(&counter) else {
                continue; // The frame never arrived, which is loss and not a forgery.
            };
            let mut verified_plaintext = None;
            let mut mismatched = 0usize;
            for pending in list {
                if verified_plaintext.is_none() && pending.hash == *hash {
                    verified_plaintext = Some(pending.held);
                } else {
                    mismatched += 1;
                }
            }
            if mismatched > 0 {
                if !state.muted {
                    step.impersonations += 1;
                }
                state.muted = true;
            }
            if let Some(held) = verified_plaintext {
                state.window.accept(counter);
                if let Some(plaintext) = held {
                    step.deliver.push((parsed.sender, plaintext));
                }
            }
        }

        let end = parsed.first + parsed.hashes.len() as u64 - 1;
        state.verified_high = Some(state.verified_high.map_or(end, |high| high.max(end)));
        step
    }

    /// Starts accepting digests of the epoch with these key id bits.
    pub(crate) fn accept_epoch(&mut self, lsb: u64) {
        self.epochs.insert(lsb);
    }

    /// Takes a key id whose frame opened under the current key: its digests are accepted, and its replay state stays.
    ///
    /// Returns false when the link already took [`MAX_ADOPTED_IDS`]: a member with the key could name any of them.
    pub(crate) fn adopt_epoch(&mut self, lsb: u64) -> bool {
        if !self.adopted.contains(&lsb) && self.adopted.len() >= MAX_ADOPTED_IDS {
            return false;
        }
        self.epochs.insert(lsb);
        self.adopted.insert(lsb);
        true
    }

    /// The key changed, so the ids adopted under the old one mean nothing now, unless the new epoch uses that id itself.
    pub(crate) fn drop_adopted(&mut self, except: u64) {
        for lsb in std::mem::take(&mut self.adopted) {
            if lsb != except {
                self.epochs.remove(&lsb);
                self.senders.retain(|(_, epoch), _| *epoch != lsb);
            }
        }
    }

    /// Forgets a sender's state for an epoch whose grace period has ended.
    pub(crate) fn drop_epoch(&mut self, lsb: u64) {
        if self.adopted.contains(&lsb) {
            return;
        }
        self.epochs.remove(&lsb);
        self.senders.retain(|(_, epoch), _| *epoch != lsb);
    }

    /// Whether a sender is muted in an epoch. Used by tests and stats.
    #[cfg(test)]
    pub(crate) fn is_muted(&self, sender: u64, lsb: u64) -> bool {
        self.senders
            .get(&(sender, lsb))
            .is_some_and(|state| state.muted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2ee::{peek_counter, SframeReceiver, SframeSender, DEFAULT_LEVEL, REPLAY_WINDOW};
    use crate::key::MediaKey;
    use dissent_identity::RootKey;
    use std::time::Duration;

    const SENDER: u64 = 2;
    /// The victim's context in the forgery tests. A forger reads it off the wire.
    const VICTIM_CONTEXT: u64 = 1234;

    fn media_key() -> MediaKey {
        MediaKey::new([5; 32])
    }

    /// One member's device key, and a certificate for it signed by a root.
    fn member() -> (DeviceKey, DeviceCert) {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        (device, cert)
    }

    /// A sender that encrypts, batches and signs, as a participant does on the wire.
    struct Sender {
        tx: SframeSender,
        signer: FrameSigner,
        now: Instant,
    }

    impl Sender {
        fn new(device: DeviceKey) -> Self {
            Self {
                tx: SframeSender::in_context(&media_key(), SENDER, 0, VICTIM_CONTEXT).unwrap(),
                signer: FrameSigner::new(device, SENDER, 0, 10),
                now: Instant::now(),
            }
        }

        /// Encrypts one frame. Returns the wire bytes and any digests that closed.
        fn frame(&mut self, byte: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
            let wire = self.tx.encrypt(&[byte; 160], None, DEFAULT_LEVEL).unwrap();
            let counter = peek_counter(&wire).unwrap();
            self.now += Duration::from_millis(20);
            let digests = self.signer.record(counter, &wire, self.now);
            (wire, digests)
        }
    }

    /// A receiver for one participant: decrypts and runs the authentication state.
    struct Receiver {
        rx: SframeReceiver,
        state: RxState,
        now: Instant,
    }

    impl Receiver {
        fn new(certs: HashMap<u64, DeviceCert>) -> Self {
            let mut state = RxState::new(Some(certs));
            state.accept_epoch(0);
            Self {
                rx: SframeReceiver::new(media_key(), 0),
                state,
                now: Instant::now(),
            }
        }

        fn plain() -> Self {
            let mut state = RxState::new(None);
            state.accept_epoch(0);
            Self {
                rx: SframeReceiver::new(media_key(), 0),
                state,
                now: Instant::now(),
            }
        }

        fn frame(&mut self, wire: &[u8]) -> Step {
            self.now += Duration::from_millis(1);
            let opened = self.rx.open(wire, self.now).unwrap();
            self.state.on_frame_in(
                opened.sender,
                opened.lsb,
                opened.context,
                opened.counter,
                frame_hash(wire),
                opened.plaintext,
                self.now,
            )
        }

        fn digest(&mut self, wire: &[u8]) -> Step {
            self.now += Duration::from_millis(1);
            self.state.on_digest(wire, self.now)
        }

        fn muted(&self, sender: u64) -> bool {
            self.state.is_muted(sender, 0)
        }
    }

    fn certs_for(sender: u64, cert: DeviceCert) -> HashMap<u64, DeviceCert> {
        HashMap::from([(sender, cert)])
    }

    fn payloads(step: &Step) -> Vec<Vec<u8>> {
        step.deliver
            .iter()
            .map(|(_, p)| crate::frame::payload_of_stamp(p).unwrap().to_vec())
            .collect()
    }

    #[test]
    fn honest_traffic_verifies_with_no_unauthenticated_frames() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let mut delivered = 0;
        let mut totals = Step::default();
        for byte in 0..50u8 {
            let (wire, digests) = sender.frame(byte);
            let step = receiver.frame(&wire);
            delivered += step.deliver.len();
            totals.unauthenticated += step.unauthenticated;
            totals.impersonations += step.impersonations;
            for digest in digests {
                let step = receiver.digest(&digest);
                totals.unauthenticated += step.unauthenticated;
                totals.impersonations += step.impersonations;
                assert!(
                    step.deliver.is_empty(),
                    "verified frames were already played"
                );
            }
        }
        assert_eq!(delivered, 50, "every frame plays on arrival");
        assert_eq!(
            totals.unauthenticated, 0,
            "honest traffic leaves nothing unauthenticated"
        );
        assert_eq!(totals.impersonations, 0);
        assert!(!receiver.muted(SENDER));
    }

    #[test]
    fn a_batch_closes_at_ten_frames_or_two_hundred_milliseconds() {
        let (device, _) = member();
        let mut sender = Sender::new(device);
        let mut closed_at = Vec::new();
        for byte in 0..10u8 {
            let (_, digests) = sender.frame(byte);
            if !digests.is_empty() {
                closed_at.push(byte);
            }
        }
        assert_eq!(closed_at, vec![9], "the tenth frame closes the batch");

        // A slow sender closes the batch by age, before ten frames.
        let (device, _) = member();
        let mut slow = FrameSigner::new(device, SENDER, 0, 10);
        let mut tx = SframeSender::new(&media_key(), SENDER, 0).unwrap();
        let start = Instant::now();
        let wire0 = tx.encrypt(&[0u8; 160], None, DEFAULT_LEVEL).unwrap();
        assert!(slow.record(0, &wire0, start).is_empty());
        let wire1 = tx.encrypt(&[1u8; 160], None, DEFAULT_LEVEL).unwrap();
        let digests = slow.record(1, &wire1, start + batch_span(10));
        assert_eq!(digests.len(), 1, "a batch older than 200 ms closes");
    }

    #[test]
    fn a_replayed_frame_is_dropped_before_and_after_verification() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));

        let (wire, _) = sender.frame(1);
        assert_eq!(payloads(&receiver.frame(&wire)).len(), 1);
        // Replayed while still waiting for its digest.
        let step = receiver.frame(&wire);
        assert_eq!(step.replayed, 1);
        assert!(step.deliver.is_empty());

        for byte in 2..10u8 {
            let (w, digests) = sender.frame(byte);
            receiver.frame(&w);
            for digest in digests {
                receiver.digest(&digest);
            }
        }
        // Replayed again, now that it is verified.
        let step = receiver.frame(&wire);
        assert_eq!(step.replayed, 1);
        assert!(step.deliver.is_empty());
    }

    #[test]
    fn an_out_of_order_frame_inside_the_window_is_accepted() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let frames: Vec<(Vec<u8>, Vec<Vec<u8>>)> = (0..10u8).map(|b| sender.frame(b)).collect();
        let order = [4usize, 0, 9, 1, 7, 2, 8, 3, 6, 5];
        let mut delivered = 0;
        let mut digest = Vec::new();
        for index in order {
            delivered += receiver.frame(&frames[index].0).deliver.len();
            if let Some(d) = frames[index].1.first() {
                digest = d.clone();
            }
        }
        assert_eq!(delivered, 10, "each out-of-order frame plays");
        let step = receiver.digest(&digest);
        assert_eq!(step.unauthenticated, 0);
        assert_eq!(step.replayed, 0);
    }

    #[test]
    fn a_frame_older_than_the_window_is_dropped() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let (oldest, _) = sender.frame(0);
        for byte in 1..=(REPLAY_WINDOW as usize + 10) {
            let (w, digests) = sender.frame((byte % 250) as u8);
            receiver.frame(&w);
            for d in digests {
                receiver.digest(&d);
            }
        }
        let step = receiver.frame(&oldest);
        assert_eq!(
            step.replayed, 1,
            "a frame more than the window behind is too old"
        );
        assert!(step.deliver.is_empty());
    }

    #[test]
    fn a_mismatched_frame_mutes_the_sender_and_the_real_frame_still_plays() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));

        // A forger shares the media key, and sends under the victim's sender id from counter 0.
        let mut forger = SframeSender::in_context(&media_key(), SENDER, 0, VICTIM_CONTEXT).unwrap();
        let forged = forger.encrypt(b"forged voice", None, DEFAULT_LEVEL).unwrap();

        receiver.frame(&forged);
        let (real0, _) = sender.frame(7);
        let step = receiver.frame(&real0);
        assert_eq!(
            payloads(&step),
            vec![vec![7u8; 160]],
            "the real frame is accepted"
        );

        let mut detections = 0;
        for byte in 8..17u8 {
            let (w, digests) = sender.frame(byte);
            receiver.frame(&w);
            for digest in digests {
                detections += receiver.digest(&digest).impersonations;
            }
        }
        assert_eq!(detections, 1, "the forged frame is detected once");
        assert!(receiver.muted(SENDER));
    }

    #[test]
    fn a_muted_sender_plays_only_frames_that_a_digest_verifies() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let mut forger = SframeSender::in_context(&media_key(), SENDER, 0, VICTIM_CONTEXT).unwrap();
        let forged = forger.encrypt(b"forged", None, DEFAULT_LEVEL).unwrap();

        // First batch: the forged frame collides with the real frame 0, and the digest exposes it.
        receiver.frame(&forged);
        let mut first_digest = Vec::new();
        for byte in 0..10u8 {
            let (w, digests) = sender.frame(byte);
            receiver.frame(&w);
            if let Some(digest) = digests.into_iter().next() {
                first_digest = digest;
            }
        }
        let step = receiver.digest(&first_digest);
        assert_eq!(step.impersonations, 1);
        assert!(receiver.muted(SENDER));

        // Second batch: held as it arrives, then played once its digest verifies.
        let mut arrived_played = 0;
        let mut second_digest = Vec::new();
        for byte in 10..20u8 {
            let (w, digests) = sender.frame(byte);
            arrived_played += receiver.frame(&w).deliver.len();
            if let Some(digest) = digests.into_iter().next() {
                second_digest = digest;
            }
        }
        assert_eq!(
            arrived_played, 0,
            "a muted sender's frames wait for their digest"
        );
        let step = receiver.digest(&second_digest);
        assert_eq!(step.deliver.len(), 10, "the verified frames play");
        assert_eq!(step.impersonations, 0, "no second detection");
    }

    #[test]
    fn a_far_ahead_frame_is_held_until_a_digest_covers_it() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        for byte in 0..10u8 {
            let (w, digests) = sender.frame(byte);
            receiver.frame(&w);
            for digest in digests {
                receiver.digest(&digest);
            }
        }
        // A frame 60 counters beyond the last verified one, which nobody has sent yet.
        let mut ahead = SframeSender::new(&media_key(), SENDER, 0).unwrap();
        for _ in 0..70 {
            ahead.encrypt(&[0u8; 160], None, DEFAULT_LEVEL).unwrap();
        }
        let far = ahead.encrypt(b"far ahead", None, DEFAULT_LEVEL).unwrap();
        let step = receiver.frame(&far);
        assert!(step.deliver.is_empty(), "a far-ahead frame does not play");
        assert!(
            !receiver.muted(SENDER),
            "a far-ahead frame mutes no one by itself"
        );
    }

    #[test]
    fn a_lost_digest_is_tolerated_without_muting() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let mut digests = Vec::new();
        for byte in 0..20u8 {
            let (w, d) = sender.frame(byte);
            receiver.frame(&w);
            digests.extend(d);
        }
        // Drop the first digest, which covers counters 0..10. Deliver the second, 10..20.
        let step = receiver.digest(&digests[1]);
        assert_eq!(
            step.unauthenticated, 10,
            "the frames of the lost digest are unauthenticated"
        );
        assert_eq!(step.impersonations, 0);
        assert!(!receiver.muted(SENDER), "a missing digest never mutes");
    }

    #[test]
    fn frames_no_digest_covers_within_the_deadline_count_as_unauthenticated() {
        let (device, cert) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let (w, _) = sender.frame(1);
        receiver.frame(&w);
        let late = receiver.now + PENDING_DEADLINE + Duration::from_millis(1);
        let step = receiver
            .state
            .on_frame_in(SENDER, 0, 0, 99, [0; HASH_LEN], vec![0], late);
        assert_eq!(step.unauthenticated, 1, "the old frame expired unverified");
    }

    #[test]
    fn a_digest_that_does_not_verify_is_a_forgery_and_mutes_the_sender() {
        let (device, cert) = member();
        let (attacker, _) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let (w, _) = sender.frame(0);
        receiver.frame(&w);
        // The attacker signs a digest in the victim's name with its own device key.
        let hashes = [frame_hash(&w)];
        let forged_digest = encode_digest(&attacker, SENDER, 0, 0, &hashes);
        let step = receiver.digest(&forged_digest);
        assert_eq!(step.impersonations, 1);
        assert!(receiver.muted(SENDER));
    }

    #[test]
    fn plain_links_check_replays_without_digests() {
        let mut sender = SframeSender::new(&media_key(), SENDER, 0).unwrap();
        let wire = sender.encrypt(&[9u8; 160], None, DEFAULT_LEVEL).unwrap();
        let mut receiver = Receiver::plain();
        assert_eq!(receiver.frame(&wire).deliver.len(), 1);
        assert_eq!(receiver.frame(&wire).replayed, 1);
    }

    #[test]
    fn a_digest_on_a_plain_link_is_ignored() {
        let (device, _) = member();
        let mut sender = Sender::new(device);
        let mut receiver = Receiver::plain();
        let mut digest = Vec::new();
        for byte in 0..10u8 {
            let (w, digests) = sender.frame(byte);
            receiver.frame(&w);
            if let Some(x) = digests.into_iter().next() {
                digest = x;
            }
        }
        let step = receiver.digest(&digest);
        assert_eq!(step, Step::default());
    }

    #[test]
    fn a_digest_for_an_epoch_the_link_does_not_accept_is_ignored() {
        let (device, cert) = member();
        let mut sender = Sender::new(device.clone());
        let mut receiver = Receiver::new(certs_for(SENDER, cert));
        let (w, _) = sender.frame(0);
        receiver.frame(&w);
        // A correctly signed digest for epoch bits 5, which this link never accepted.
        let digest = encode_digest(&device, SENDER, 5, 0, &[frame_hash(&w)]);
        let step = receiver.digest(&digest);
        assert_eq!(
            step,
            Step::default(),
            "an unknown epoch's digest is ignored"
        );
        assert!(!receiver.muted(SENDER));
    }

    // ---- Starts of sending: see `RxState::on_frame_in` ----

    /// A second start of sending by the same device, as after a rejoin: same key, same sender, counting from 0.
    fn rejoin(seed: u8) -> (Sender, HashMap<u64, DeviceCert>) {
        let device = DeviceKey::from_seed([seed; 32]);
        let cert = RootKey::from_seed([seed; 32]).certify(device.id(), 0);
        let mut sender = Sender::new(device);
        sender.tx = SframeSender::new(&media_key(), SENDER, 0).unwrap();
        (sender, certs_for(SENDER, cert))
    }

    fn played_after_digest(rx: &mut Receiver, tx: &mut Sender, frames: u8) -> (usize, Vec<Vec<u8>>) {
        let (mut played, mut wires) = (0, Vec::new());
        for i in 0..frames {
            let (wire, digests) = tx.frame(i);
            played += rx.frame(&wire).deliver.len();
            wires.push(wire);
            for digest in digests {
                played += rx.digest(&digest).deliver.len();
            }
        }
        (played, wires)
    }

    #[test]
    fn a_sender_that_starts_again_is_heard_and_its_old_session_stays_refused() {
        let (mut first, certs) = rejoin(1);
        let mut rx = Receiver::new(certs);
        let (played, old) = played_after_digest(&mut rx, &mut first, 30);
        assert_eq!(played, 30);
        let (mut second, _) = rejoin(1);
        let (played, _) = played_after_digest(&mut rx, &mut second, 30);
        assert_eq!(played, 30, "the new start is heard, though its counters are ones the old one used");
        let session = rx.state.senders.get(&(SENDER, 0)).unwrap();
        assert_eq!(session.retired.len(), 1, "the digest made it the sender's session");
        for wire in &old {
            let step = rx.frame(wire);
            assert!(step.deliver.is_empty(), "a frame of the old session is not played again");
            assert_eq!(step.replayed, 1);
        }
    }

    /// A member holds the key, so it can seal frames in another's name, under any context. It cannot sign their digests.
    #[test]
    fn a_member_naming_a_sender_under_new_contexts_does_not_replace_its_session() {
        let (mut real, certs) = rejoin(2);
        let mut rx = Receiver::new(certs);
        played_after_digest(&mut rx, &mut real, 20);
        for byte in 0..100u8 {
            let forged = SframeSender::new(&media_key(), SENDER, 0).unwrap().encrypt(&[byte; 160], None, DEFAULT_LEVEL).unwrap();
            rx.frame(&forged);
        }
        let session = rx.state.senders.get(&(SENDER, 0)).unwrap();
        assert!(session.retired.is_empty(), "no forged context took over, or pushed out a record");
        assert_eq!(rx.state.senders.len(), 1);
        let (played, _) = played_after_digest(&mut rx, &mut real, 20);
        assert_eq!(played, 20, "the sender is still heard");
        assert!(!rx.muted(SENDER));
    }

    /// The forger's frame arrives first, so it stands as the session until the real sender's digest proves its own.
    #[test]
    fn a_forged_first_frame_under_its_own_context_gives_way_to_the_signed_session() {
        let (mut real, certs) = rejoin(4);
        let mut rx = Receiver::new(certs);
        let forged = SframeSender::new(&media_key(), SENDER, 0).unwrap().encrypt(b"forged voice", None, DEFAULT_LEVEL).unwrap();
        assert_eq!(rx.frame(&forged).deliver.len(), 1, "a sender's first frame plays before its digest, as ever");
        let mut flagged = 0;
        let mut played = 0;
        for i in 0..10 {
            let (wire, digests) = real.frame(i);
            played += rx.frame(&wire).deliver.len();
            for digest in digests {
                let step = rx.digest(&digest);
                flagged += step.impersonations;
                played += step.deliver.len();
            }
        }
        assert_eq!(played, 10, "the real frames wait for the digest, then play");
        assert_eq!(flagged, 1, "the forged frame at a counter the digest covers is caught");
        assert!(rx.muted(SENDER));
    }

    #[test]
    fn the_record_of_old_sessions_is_bounded() {
        let (mut first, certs) = rejoin(3);
        let mut rx = Receiver::new(certs);
        played_after_digest(&mut rx, &mut first, 11);
        for _ in 0..MAX_RETIRED_SESSIONS * 2 {
            let (mut again, _) = rejoin(3);
            played_after_digest(&mut rx, &mut again, 11);
        }
        assert_eq!(rx.state.senders.get(&(SENDER, 0)).unwrap().retired.len(), MAX_RETIRED_SESSIONS);
    }

    #[test]
    fn senders_nobody_knows_and_adopted_ids_are_bounded() {
        let mut state = RxState::new(Some(HashMap::new()));
        let now = Instant::now();
        for sender in 0..100_000u64 {
            state.on_frame_in(sender, 0, 7, 0, [0; HASH_LEN], vec![0], now);
        }
        assert_eq!(state.senders.len(), MAX_UNKNOWN_SENDERS);
        for lsb in 0..MAX_ADOPTED_IDS as u64 {
            assert!(state.adopt_epoch(lsb));
        }
        assert!(state.adopt_epoch(0), "one already taken stays taken");
        assert!(!state.adopt_epoch(100), "no more are taken until the next rekey");
        state.drop_adopted(100);
        assert!(state.adopt_epoch(100));
    }
}

#[cfg(test)]
mod cost_tests {
    use super::*;
    use crate::e2ee::{SframeSender, DEFAULT_LEVEL};
    use crate::key::MediaKey;
    use dissent_identity::RootKey;

    /// Measures what the authentication layer adds to a speaker's stream. Digest bytes are summed
    /// over five seconds of audio, the exact packets a sender writes, and divided by the time.
    #[test]
    fn digest_overhead_per_speaker_at_the_default_batch() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let _ = root.certify(device.id(), 0);
        let media_key = MediaKey::new([4; 32]);
        let mut tx = SframeSender::new(&media_key, 2, 0).unwrap();
        let mut signer = FrameSigner::new(device, 2, 0, AUDIO_DIGEST_FRAMES);
        let frames = 250u64; // five seconds at 20 ms
        let start = Instant::now();
        let mut digest_bytes = 0usize;
        let mut frame_bytes = 0usize;
        for index in 0..frames {
            let wire = tx.encrypt(&[7u8; 160], None, DEFAULT_LEVEL).unwrap();
            frame_bytes += wire.len();
            let now = start + Duration::from_millis(20 * index);
            let counter = crate::e2ee::peek_counter(&wire).unwrap();
            for digest in signer.record(counter, &wire, now) {
                digest_bytes += digest.len();
            }
        }
        let seconds = (frames * 20) as f64 / 1000.0;
        let kbps = digest_bytes as f64 * 8.0 / seconds / 1000.0;
        let frame_kbps = frame_bytes as f64 * 8.0 / seconds / 1000.0;
        println!(
            "digest overhead: {kbps:.2} kbps per speaker at {AUDIO_DIGEST_FRAMES} frames per digest, {HASH_LEN}-byte hashes; audio payload {frame_kbps:.1} kbps"
        );
        assert!(kbps < 5.0, "digest overhead {kbps:.2} kbps is above 5 kbps");
    }
}
