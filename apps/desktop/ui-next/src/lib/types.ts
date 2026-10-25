// Wire types for the Tauri commands and events. Shapes are read from the current UI (apps/desktop/ui) and the engine
// it talks to. Ids are lowercase base32 strings. Times are epoch milliseconds. Payloads that no ported view reads yet
// are typed as `unknown` in api.ts; the batch that ports a view narrows them here.

export type Id = string;

export type Role = 'owner' | 'admin' | 'moderator' | 'member';

// The statuses a person can choose for themselves. `invisible` shows as offline to others.
export type StatusName = 'online' | 'idle' | 'dnd' | 'invisible';

// The glyph shape a presence dot takes. Each has its own shape, so colour is never the only cue.
export type Presence = 'online' | 'idle' | 'dnd' | 'offline';

export interface UserStatus {
  status: StatusName;
  until_ms: number | null;
}

export interface Me {
  root_id: Id;
  device_id?: Id;
  display_name: string;
  first_run?: boolean;
}

export interface CommunitySummary {
  id: Id;
  name: string;
  hue?: number;
  mentions?: number;
}

export interface VoiceParticipant {
  root_id: Id;
  display_name: string;
  muted: boolean;
  deafened: boolean;
  speaking: boolean;
  forwarder?: boolean | string; // relays the call for the others (true from the engine; a name in older fixtures)
}

export interface ChannelView {
  id: Id;
  name: string;
  voice?: boolean;
  slowmode_s?: number;
  visibility?: 'default' | 'spoiler' | 'age';
  bitrate_kbps?: number;
  user_limit?: number;
  relay?: string;
  topic?: string | null; // shown in the header when the topics feature is on
}

// A member of a server, as community_view reports it. Banned members are kept in the list and hidden by the views.
export interface Member {
  root_id: Id;
  suffix?: string;
  display_name: string;
  avatar?: string | null;
  handle?: string | null;
  role: Role;
  banned: boolean;
  status?: Presence;
  online?: boolean;
}

export interface VoiceChannelView {
  id: Id;
  participants: VoiceParticipant[];
}

export interface CommunityView extends CommunitySummary {
  my_role: Role | null;
  channels: ChannelView[];
  members: Member[];
  relays?: string[]; // the relays voice channels can use; Automatic is always there
  voice_channels?: VoiceChannelView[];
  history_locked?: boolean; // member-only history: older control events wait for a member's key
}

// A call another device said is live in a DM or a group (LiveCallView). Only the people in it show, not this device's.
export interface LiveCall {
  scope?: 'channel' | 'dm' | 'group';
  started_ms?: number;
  started_by?: string;
  total?: number; // everyone in the conversation, this device's person included
  route?: { kind: string };
  participants?: LiveParticipant[];
  not_in?: { root_id: Id; display_name: string }[];
}

export interface LiveParticipant {
  root_id: Id;
  display_name: string;
  muted: boolean;
  speaking: boolean;
  forwarder?: boolean | string;
}

export interface DirectSummary {
  kind: 'dm';
  peer: Id;
  display_name: string;
  status?: Presence;
  online?: boolean;
  pinned?: boolean;
  last_text?: string;
  last_ms?: number;
  live_call?: LiveCall | null;
}

export interface GroupSummary {
  kind: 'group';
  id: Id;
  name: string;
  members: { root_id: Id; display_name: string }[];
  creator?: Id; // the root id of the person who made the group
  pinned?: boolean;
  last_ms?: number;
  live_call?: LiveCall | null;
}

export type ConversationSummary = DirectSummary | GroupSummary;

export type Page = 'contacts' | 'transfers';

export type Route = { kind: 'home' } | { kind: 'server'; communityId: Id };

// The engine writes the route's sentence (`title`, `detail`). `kind` is p2p, mesh, forwarded or cascade.
export interface CallRoute {
  kind: string;
  title: string;
  detail: string;
  forwarder?: string; // older fixtures name the forwarder and the backup directly
  backup?: string;
}

export interface CallQuality {
  rtt_ms: number | null;
  loss_pct: number;
  jitter_ms: number;
  flagged: number; // frames whose signature failed; a healthy call shows zero
}

export interface CallMessage {
  id: Id;
  root_id: Id;
  author_name: string;
  ms: number | string;
  text?: string;
  file?: { name: string; kind: string; meta: string };
}

export interface CallState {
  id?: string; // the room id: a new id is a different call
  scope: 'channel' | 'dm' | 'group';
  peer?: Id | null; // the other person of a direct call
  muted: boolean;
  deafened: boolean;
  camera?: boolean;
  started_ms?: number;
  participants: VoiceParticipant[];
  route?: CallRoute;
  quality?: CallQuality;
  channel_name?: string;
  community_name?: string;
  where?: string;
}

// `dissent://call-ringing`: a direct or group call that rings this device.
export interface IncomingCall {
  call_id: string;
  scope: 'dm' | 'group';
  peer?: Id;
  group_id?: Id;
  name: string;
  caller: { root_id: Id; display_name: string };
  verified: boolean;
  in_call: number;
  total: number;
  members_in: { root_id: Id; display_name: string }[];
}

// Audio devices and the saved voice settings (audio_devices, voice_settings), and the mic level event.
export interface AudioDevice {
  name: string;
  is_default: boolean;
  subtitle: string | null;
  kind: 'headset' | 'display' | 'other';
}

export interface AudioDevices {
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  default_input: string | null;
  default_output: string | null;
  input: string | null; // the chosen device, or null for the system default
  output: string | null;
}

export type VoiceProfile = 'voice_isolation' | 'studio' | 'custom';

export interface VoiceSettings {
  profile: VoiceProfile;
  input_volume: number; // percent, 0 to 200
  output_volume: number;
  noise_suppression: 'standard' | 'off';
  echo_cancellation: boolean;
  agc: boolean;
  auto_sensitivity: boolean;
  threshold_db: number;
  push_to_talk: boolean;
  ptt_key: string | null;
}

export interface MicLevel {
  db: number; // processed level before the gate; -100 is silence
  open: boolean; // the frame passed the gate, so it is being sent
  threshold_db: number;
}

export interface VoiceConnection {
  communityId: Id;
  channelId: Id;
}

// One file transfer, from list_transfers and dissent://transfers. `route_kind` picks the colour of the route tag.
export type TransferState = 'waiting' | 'active' | 'paused' | 'done' | 'failed';
export type TransferRouteKind = 'direct' | 'relay' | 'server' | 'seeder' | 'waiting' | 'refused' | 'failed';

export interface TransferItem {
  id: Id;
  direction: 'upload' | 'download';
  name: string;
  size: number; // bytes
  state: TransferState;
  progress: number; // 0 to 1
  route: string;
  route_kind: TransferRouteKind;
  route_note?: string;
  where?: { kind: 'server' | 'direct'; name: string };
  sources?: number;
  status_text?: string;
  error?: string;
  local_path?: string | null;
}

export interface TransferRates {
  down_bps: number;
  up_bps: number;
}

export interface TransferSettings {
  max_accept_bytes: number;
  download_dir: string;
  auto_accept_bytes?: number | null; // null: always ask
}

// The encrypted file store, from store_stats. Each part is bytes; the parts are absent on builds that do not report them.
export interface StoreStats {
  used_bytes: number;
  budget_bytes: number;
  files?: number;
  shared_bytes?: number;
  downloaded_bytes?: number;
  partial_bytes?: number;
}

export interface UpdateInfo {
  version: string;
  current_version: string;
  date?: string;
  notes?: string;
  portable?: boolean; // a portable build cannot update in place: the dialog links to the release page
}

export type TypingTarget =
  | { kind: 'channel'; communityId: Id; channelId: Id }
  | { kind: 'dm'; peer: Id }
  | { kind: 'group'; groupId: Id };

// Every engine event carries `kind`. The shell reads the kinds listed here; any other kind reaches the default case of
// the store's switch and is ignored.
export type EngineEvent =
  | { kind: 'communities_changed' }
  | { kind: 'community_updated'; community_id: Id }
  | { kind: 'message'; community_id: Id; channel_id: Id }
  | { kind: 'dm'; peer: Id }
  | { kind: 'group'; group_id: Id }
  | { kind: 'contacts' }
  | { kind: 'presence' }
  | { kind: 'call_missed'; call_id: string }
  | { kind: 'call_answered_elsewhere'; call_id: string };

export interface Capabilities {
  [feature: string]: boolean;
}

// A person in a list: a contact, a member of a group, a request. `suffix` is the short address tag shown with the name.
export interface Person {
  root_id: Id;
  display_name: string;
  suffix?: string;
  avatar?: string | null;
  status?: Presence;
  online?: boolean;
}

// The contact lists from list_contacts: your friends, the requests either way, and the people you blocked.
export interface ContactsView {
  contacts: Person[];
  incoming: Person[];
  outgoing: Person[];
  blocked: Person[];
}

// The portable profile gate: `chooser` until a first-run choice is saved, then `locked` until the passphrase opens it.
export interface VaultStatus {
  chooser: boolean;
  locked: boolean;
  new_profile: boolean;
  choice: 'link' | 'copy' | 'fresh' | 'installed' | null;
  installed_profile: boolean;
}

export interface NetDials {
  by_source?: Partial<Record<'cache' | 'peer' | 'seeder' | 'lan' | 'dht' | 'open', number>>;
  last_via?: string;
  last_connect_ms?: number;
  failed?: number;
}

export interface NetStatus {
  dht: { size_estimate: number; confidence: number; firewalled: boolean } | null;
  peers: number;
  relay: string | null;
  dials: NetDials | null;
}

export interface NetworkSettings {
  use_public_relays: boolean;
  relays: string[];
  lan_discovery?: boolean;
}

// The guardians of this identity, and the threshold of them that must agree.
export interface GuardianView {
  root_id: Id;
  display_name: string;
  suffix?: string;
  shared_servers: number;
}

export interface GuardianList {
  threshold: number;
  guardians: GuardianView[];
}

export interface DeviceView {
  device_id: Id;
  name: string;
  this_device: boolean;
  last_seen_ms: number;
}

// What user_profile returns: the card, the DM panel and the full profile read the same view. Times are epoch ms.
export interface UserProfileView {
  root_id: Id;
  display_name: string;
  suffix?: string;
  avatar?: string | null;
  status?: Presence;
  online?: boolean;
  is_contact?: boolean;
  key_changed_ms?: number | null;
  verified_ms?: number | null;
  first_seen_ms?: number | null;
  contact_since_ms?: number | null;
  handles?: { community_id: Id; community_name: string; handle: string | null }[];
  mutual_communities: { id: Id; name: string }[];
}

// ---- messages: a channel (server) and a direct or group conversation. Read by the chat batch. ----

export interface Reaction {
  emoji: string;
  count: number;
  me?: boolean;
  names?: string[]; // up to three names, when the engine sends them
}

export type AttachmentState =
  | 'available' | 'downloading' | 'sending' | 'paused' | 'failed' | 'disk_full' | 'too_large' | 'waiting' | 'downloaded';

export interface Attachment {
  id: Id;
  name: string;
  size: number | null; // bytes
  mime?: string;
  state: AttachmentState;
  progress?: number; // 0 to 1
  transfer_id?: Id | null;
  local_path?: string | null; // set once downloaded or sent: the only path an image or open action uses
  verified?: boolean;
  needed?: number; // bytes, for disk_full
  note?: string; // why a download waits
  status?: string; // a sending line
  sources?: FileSourceView[];
  rate_bps?: number;
  eta_s?: number;
  transfer?: unknown;
  pieces?: { total?: number; verified?: number }; // the verified pieces of a multi-source download
  shared_in?: string; // where the file is shared, as the details panel says it
  fingerprint?: string;
  direct?: { peer: Id; sent: number; total: number; speed_bps: number; time_left_s: number } | null; // a transfer between this user's devices
}

export interface FileSourceView {
  display_name?: string;
  note?: string;
  route?: 'direct' | 'relay' | 'seeder';
  rate_bps?: number;
}

// The card for an invite link, from invite_preview. `icon` is a local path or null; `invite` is live, revoked, expired or unknown.
export interface InvitePreview {
  community: Id;
  name: string;
  icon: string | null;
  member_count: number;
  online_count: number;
  created_ms: number;
  invite: 'live' | 'revoked' | 'expired' | 'unknown';
  already_member: boolean;
}

// A link preview from the sender's sealed message: a plain link card, or a rich card with a title, chip and tile.
export interface Embed {
  kind?: 'card';
  url?: string;
  site?: string;
  title?: string;
  description?: string;
  image?: string | null; // a local path the backend wrote, never a URL from the message
  image_w?: number;
  image_h?: number;
  subtitle?: string;
  tile?: { color?: string; label?: string } | null;
}

export interface ReplyRef {
  id: Id;
  author?: Id;
  author_name: string;
  text: string | null; // null when the original is gone or locked
  locked?: boolean;
}

interface MessageBase {
  id: Id;
  text: string;
  ms: number;
  edited?: boolean;
  deleted?: boolean;
  queued?: boolean;
  reply_to?: ReplyRef | null;
  reactions?: Reaction[] | null;
  attachments?: Attachment[] | null;
  embeds?: Embed[] | null;
}

// A message in a server channel, as channel_messages returns it.
export interface ChannelMessage extends MessageBase {
  author: Id;
  author_name: string;
}

// A message in a direct conversation or a group, as dm_messages and group_messages return it.
export interface ConversationMessage extends MessageBase {
  from: Id;
  pending?: boolean;
  system?: boolean; // a membership line in a group, e.g. "added Omar and Rana."
}

// A chat target for typing signals and the chat's sends: a channel, a direct conversation or a group.
export type ChatTarget =
  | { kind: 'channel'; communityId: Id; channelId: Id; key: string }
  | { kind: 'dm'; peer: Id; key: string }
  | { kind: 'group'; groupId: Id; key: string };
