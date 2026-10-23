# Design 02: multiple servers, channels, cross-server users, DMs

Shared contract for all agents. "Server" in the UI = "community" in code.

## User-facing behaviour

1. A user can **create** a server (they become its owner), which starts with a `general` channel.
2. The owner (and any member) can **create an invite link** `dissent://invite/...` and share it out of band.
3. A user can **join many servers** from invite links. A left rail lists them all; selecting one shows its channels and member list.
4. Admins can create more channels. Messages can be sent, edited (own), deleted (own, or mods).
5. Clicking any member in any server opens their **profile**: display name, their handle in each server you share, mutual servers, and a **Message** button.
6. **Direct messages** are 1:1, sent device-to-device over a direct iroh connection (QUIC/TLS end to end between devices; relays only see ciphertext). Undelivered DMs stay in an outbox and are retried.
7. Users who share no server can still DM by exchanging a **contact link** `dissent://contact/...`.

## Already implemented (do not change unless your task says so)

- `dissent-identity`: see CLAUDE.md. Plus `Genesis { community, owner, name, created_ms, sig }` with `Genesis::create(&CommunityKey, owner, name, created_ms)` and `.verify()`. This binds the owner to the community key; invites and sync carry it.
- `dissent-events`: `Event`, `EventBody`, `Payload`, `Dag`, `replay(owner, &dag) -> CommunityState`, `HlcClock`.
- `dissent-net`: `Node::{spawn, id, addr, remember, ping, subscribe, shutdown}`, `PeerAddr` (Display/FromStr), `TopicId::for_channel`.
- `dissent-discovery`: `Invite`, `CommunityRecord`, `Discovery::{publish, resolve}`.

## Additions per crate

### dissent-events (agent: events2)

```rust
// event.rs
Payload::Join { display_name: String }   // CONTROL channel; re-sending updates the name
Payload::Leave                           // CONTROL channel
// state.rs
pub struct Member { pub display_name: String, pub joined: Hlc, pub left: bool }
CommunityState.members: BTreeMap<RootId, Member>   // owner is NOT implicit; creator emits Join like everyone
MessageView.edited: bool
// Rules: Join/Leave allowed for anyone not banned (Leave sets left=true; a later Join clears it).
// Messages do NOT require membership (avoids a new ordering race).

// wire.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvent { pub event: Event, pub cert: DeviceCert }
impl SignedEvent { pub fn verify(&self) -> Result<()>; pub fn encode(&self) -> Vec<u8>; pub fn decode(b: &[u8]) -> Result<Self>; }

// dm.rs
pub struct DmId(pub [u8; 32]);            // base32 Display/FromStr like EventId
pub struct DmBody { pub from: RootId, pub to: RootId, pub device: DeviceId, pub hlc: Hlc, pub text: String }
pub struct DmMessage { pub body: DmBody, pub id: DmId, pub sig: Signature }
impl DmMessage { pub fn sign(body: DmBody, device: &DeviceKey) -> Self; pub fn verify(&self, cert: &DeviceCert) -> Result<()>; }
pub struct SignedDm { pub dm: DmMessage, pub cert: DeviceCert }  // verify/encode/decode like SignedEvent
// domains: id = blake3(b"dissent/dm/v1" ++ postcard(body)), sig over b"dissent/dm-sig/v1" ++ id
```

### dissent-net (agent: net2)

Generic request/response on ALPN `b"dissent/rpc/0"`, one bi-stream per request, `u32` big-endian length prefix, max 16 MiB each way.

```rust
pub trait RpcHandler: Send + Sync + 'static {
    fn handle(&self, from: DeviceId, req: Vec<u8>) -> futures_lite::future::Boxed<Vec<u8>>;
}
NodeConfig.rpc: Option<Arc<dyn RpcHandler>>          // NodeConfig stays Clone; implement Debug manually if needed
impl Node { pub async fn request(&self, peer: DeviceId, req: Vec<u8>) -> Result<Vec<u8>>; } // dials by id (uses remembered addrs / iroh discovery); 15 s timeout
```

### dissent-discovery (agent: discovery2)

```rust
pub struct Invite { pub genesis: Genesis, pub peers: Vec<String>, pub secret: Option<[u8; 16]> }  // replaces community+name; Invite::parse verifies genesis
impl Invite { pub fn community(&self) -> CommunityId; pub fn name(&self) -> &str; }
pub struct ContactCard { pub cert: DeviceCert, pub display_name: String, pub peers: Vec<String> }
impl ContactCard { pub fn to_url(&self) -> Url /* dissent://contact/<base32 postcard> */; pub fn parse(s: &str) -> Result<Self> /* verifies cert */; }
```

### dissent-core (new crate, agent: core) — the engine, no tauri dependency

RPC wire (postcard):
```rust
enum Request {
    Genesis { community: CommunityId },                                   // joiner checks it against the invite
    Reconcile { community: CommunityId, msg: Vec<u8>, from: PeerAddr },   // one negentropy round; responder answers from its own events
    Fetch { community: CommunityId, ids: Vec<EventId> },                  // up to 500 ids; reply holds the ones it has, causal order, capped at 4 MiB
    Push { community: CommunityId, events: Vec<SignedEvent> },            // initiator's events the responder lacks; verified and stored like gossip
    Addr(Box<SignedAddr>),                                                // a device's signed address, sent on start and on change
    Dm { dm: Box<SignedDm>, from: PeerAddr },
}
enum Response { Genesis(Genesis), Reconcile { msg: Option<Vec<u8>> }, Events(Vec<SignedEvent>), Ack, Error(String) }
```
Set reconciliation uses the `negentropy` crate. Items are `(hlc.wall_ms, event id)`, frame limit 256 KiB. The initiator
runs rounds until done, learning `need` (held by the peer, not by us) and `have` (held by us, not by the peer). It then
calls `Fetch` for `need` and `Push` for `have`. Used for joins, gap fills after a pending event, and anti-entropy every 30 s.

Gossip: one topic per community: `TopicId::for_channel(&community, &ChannelId::CONTROL.0)`, payload = postcard `GossipMsg`:
`Event(SignedEvent)` or `Addr(SignedAddr)`. `SignedAddr` is signed by the device key over `b"dissent/addr/v1" || postcard((addr, ts_ms))`.
Receivers keep the newest address per device and persist it in `addresses.bin`.
// chisle: spec says one topic per channel; per-community keeps subscriptions bounded for users in many servers.

Every received event: `verify()` (cert.root == author, sig) → store cert → `Dag::insert` → if `Pending { missing }`, run a sync with the sender → `Replayer::on_insert` → emit `EngineEvent`. Events stamped more than 5 minutes ahead of the local clock are dropped.

Disk layout under `data_dir`:
```
profile.bin                         postcard { display_name }
contacts.bin                        postcard Vec<ContactCard>
communities/<id>/genesis.bin
communities/<id>/community.key      founder only (32-byte seed)
communities/<id>/events.log         repeated [u32 BE len][SignedEvent postcard]; replayed on start
dms/<peer-root-id>.log              repeated [u32 BE len][SignedDm postcard]
dms/outbox.bin                      postcard Vec<SignedDm> not yet acked
```

Public API (all `async` methods return `Result<_, CoreError>`; views are `Serialize` with snake_case fields and ids as base32 strings):

```rust
pub struct Engine;  // Clone
Engine::start(identity: Identity, data_dir: PathBuf, net: NodeConfig) -> Result<(Engine, tokio::sync::broadcast::Receiver<EngineEvent>)>
fn subscribe(&self) -> broadcast::Receiver<EngineEvent>
fn me(&self) -> MeView { root_id, device_id, suffix, display_name }
async fn set_display_name(&self, name: String)          // persists; re-sends Join in every community
fn contact_link(&self) -> String
async fn add_contact(&self, link: &str) -> UserView

fn communities(&self) -> Vec<CommunitySummary { id, name, owner, my_role }>     // my_role: "owner"|"admin"|"moderator"|"member"
async fn create_community(&self, name: String) -> CommunitySummary              // CommunityKey + Genesis + Join + ChannelCreate "general"; publish DHT record best-effort
async fn create_invite(&self, community: CommunityId) -> String                  // Invite{genesis, peers:[my addr], secret:None}.to_url()
async fn join_community(&self, invite: &str) -> CommunitySummary                 // remember invite peers, Sync with each, subscribe gossip, emit Join
async fn leave_community(&self, community: CommunityId)                          // emits Leave, unsubscribes, deletes local dir
fn community_view(&self, community: CommunityId) -> CommunityView {
    id, name, owner, my_role,
    channels: Vec<ChannelInfo { id, name }>,
    members: Vec<MemberInfo { root_id, suffix, display_name, handle: Option<String>, role, banned }>,
}
async fn create_channel(&self, community, name: String) -> ChannelInfo
fn channel_messages(&self, community, channel: ChannelId, limit: usize) -> Vec<MessageItem {
    id, author, author_name, text, ms, edited, deleted }>                      // newest `limit`, oldest first
async fn send_message(&self, community, channel, text: String) -> MessageItem
async fn edit_message(&self, community, channel, message: EventId, text: String)
async fn delete_message(&self, community, channel, message: EventId)
async fn moderate(&self, community, action: ModAction, user: RootId)           // ModAction: Ban|Unban|GrantAdmin|RevokeAdmin|GrantModerator|RevokeModerator
async fn claim_handle(&self, community, handle: String)
// Each mutating call returns CoreError::Rejected(reason) if replay rejected the new event.

fn user_profile(&self, user: RootId) -> UserView {
    root_id, suffix, display_name, is_contact,
    handles: Vec<{ community_id, community_name, handle }>,
    mutual_communities: Vec<{ id, name }> }
fn conversations(&self) -> Vec<ConversationSummary { peer, display_name, last_text, last_ms }>   // newest first
fn dm_messages(&self, peer: RootId, limit: usize) -> Vec<DmItem { id, from, text, ms, pending }>
async fn send_dm(&self, peer: RootId, text: String) -> DmItem                  // deliver to every known device of peer; else outbox (retry every 30 s)
async fn shutdown(self)

#[derive(Clone, Serialize)] #[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    CommunitiesChanged,
    CommunityUpdated { community_id: String },
    Message { community_id: String, channel_id: String },
    Dm { peer: String },
}
```

Known devices of a user = devices from certs seen in any community, DMs or contact cards.
Display name of a user = latest Join display_name in any shared community, else contact card name, else `anon·<suffix>`.

### apps/desktop (agents: ui, wire)

Tauri commands = one per Engine method, same names: `me`, `set_display_name`, `contact_link`, `add_contact`, `list_communities`, `create_community`, `create_invite`, `join_community`, `leave_community`, `community_view`, `create_channel`, `channel_messages`, `send_message`, `edit_message`, `delete_message`, `moderate`, `claim_handle`, `user_profile`, `list_conversations`, `dm_messages`, `send_dm`, plus existing `identity_status`, `identity_export_phrase`, `identity_import_phrase`, `app_version`.
JS passes args camelCase (Tauri default): `communityId`, `channelId`, `messageId`, `user`, `peer`, `text`, `name`, `link`, `invite`, `limit`, `handle`, `action` (`"ban"|"unban"|"grant_admin"|"revoke_admin"|"grant_moderator"|"revoke_moderator"`). Ids are base32 strings.
Backend forwards every `EngineEvent` to the window as Tauri event `"dissent://event"` with the JSON above.
