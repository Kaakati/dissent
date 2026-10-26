import { noteImages } from './images';
import type {
  AudioDevices,
  Bookmark,
  Capabilities,
  CallState,
  ChannelView,
  CommunitySummary,
  CommunityView,
  ContactsView,
  ConversationSummary,
  DeviceView,
  ShellSettings,
  EngineEvent,
  GuardianList,
  Id,
  InvitePreview,
  MessageRef,
  Me,
  NetStatus,
  ServerPrefs,
  NetworkSettings,
  StoreStats,
  Trait,
  Reminder,
  TransferItem,
  TransferRates,
  TransferSettings,
  TypingTarget,
  UpdateInfo,
  UserStatus,
  StatusName,
  UserProfileView,
  VaultStatus,
  VoiceSettings,
} from './types';
import type { Unlisten } from './tauri';
import type { ShareSource } from './types';

// Every Tauri command the UI calls, in one place. Args are camelCase (Tauri maps them to snake_case params). Names and
// argument shapes match the old UI's api.js, so the two UIs can run against the same backend and mock.
// Commands marked "not in the engine yet" are the contracts in docs/design/07-visual-system.md, Part 6. The UI calls
// them only after `capabilities` reports the feature.
// Return types are narrowed where a ported view reads the result. Other results are `unknown` until that view is ported.

const invoke = <T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  const tauri = window.__TAURI__;
  if (!tauri) return Promise.reject(new Error('No backend: the app has not loaded the Tauri globals or the mock.'));
  return tauri.core.invoke<T>(cmd, args);
};

// Every response passes through noteImages, so avatars and server icons reach the components that show them.
const call = async <T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  const out = await invoke<T>(cmd, args);
  noteImages(out);
  return out;
};

// ---- payload arguments that are shared between commands ----

// What `update_channel` changes. Only the fields given are sent.
export interface ChannelPatch {
  name?: string;
  slowmode_s?: number;
  visibility?: string;
  bitrate_kbps?: number;
  user_limit?: number;
  relay?: string; // 'automatic', or the URL of one of the server's relays
}

export interface SendFileOptions {
  seed?: boolean;
  mentions?: { users: Id[]; everyone: boolean };
}

export const api = {
  // ---- you ----
  me: () => call<Me>('me'),
  setDisplayName: (name: string) => call<void>('set_display_name', { name }),
  // Profile picture and server icon: `bytes` is the 256 px WebP or PNG as base64.
  setAvatar: (bytes: string) => call<void>('set_avatar', { bytes }),
  clearAvatar: () => call<void>('clear_avatar'),
  // Profile banner: a 16:9 WebP or PNG as base64 (at most 960 x 540), or a preset id.
  setProfileBanner: (bytes: string) => call<void>('set_profile_banner', { bytes }),
  setProfileBannerPreset: (preset: number) => call<void>('set_profile_banner_preset', { preset }),
  clearProfileBanner: () => call<void>('clear_profile_banner'),
  setCommunityIcon: (communityId: Id, bytes: string) => call<void>('set_community_icon', { communityId, bytes }),
  clearCommunityIcon: (communityId: Id) => call<void>('clear_community_icon', { communityId }),
  // Server Settings: the owner renames; owners and admins set the banner (a 16:9 WebP or PNG as base64, or a preset id)
  // and the profile (a description and up to five traits).
  renameCommunity: (communityId: Id, name: string) => call<void>('rename_community', { communityId, name }),
  // An animated GIF banner: the engine crops, scales and loops it off the UI thread, then it is set like a picture.
  prepareAnimatedBanner: (bytes: string, crop: { x: number; y: number; w: number; h: number; rot: number }) =>
    call<{ gif: string; poster: string }>('prepare_animated_banner', { bytes, crop }),
  setCommunityBannerAnimated: (communityId: Id, gif: string, poster: string) =>
    call<void>('set_community_banner_animated', { communityId, gif, poster }),
  setProfileBannerAnimated: (gif: string, poster: string) => call<void>('set_profile_banner_animated', { gif, poster }),
  setCommunityBannerPreset: (communityId: Id, preset: number) => call<void>('set_community_banner_preset', { communityId, preset }),
  setCommunityProfile: (communityId: Id, description: string, traits: Trait[]) =>
    call<void>('set_community_profile', { communityId, description, traits }),
  setCommunityBanner: (communityId: Id, bytes: string) => call<void>('set_community_banner', { communityId, bytes }),
  clearCommunityBanner: (communityId: Id) => call<void>('clear_community_banner', { communityId }),
  contactLink: () => call<string>('contact_link'),
  linkPreviews: () => call<boolean>('link_previews'),
  setLinkPreviews: (on: boolean) => call<void>('set_link_previews', { on }),
  linkPreviewPrompted: () => call<boolean>('link_preview_prompted'),
  markLinkPreviewPrompted: () => call<void>('mark_link_preview_prompted'),
  addContact: (link: string) => call<unknown>('add_contact', { link }),

  // ---- servers ----
  listCommunities: () => call<CommunitySummary[]>('list_communities'),
  setServerOrder: (ids: Id[]) => call<void>('set_server_order', { ids }),
  createCommunity: (name: string) => call<unknown>('create_community', { name }),
  createInvite: (communityId: Id) => call<unknown>('create_invite', { communityId }),
  joinCommunity: (invite: string) => call<unknown>('join_community', { invite }),
  invitePreview: (link: string) => call<InvitePreview>('invite_preview', { link }),
  createFullInvite: (communityId: Id) => call<string>('create_full_invite', { communityId }),
  leaveCommunity: (communityId: Id) => call<void>('leave_community', { communityId }),
  communityView: (communityId: Id) => call<CommunityView>('community_view', { communityId }),
  createChannel: (communityId: Id, name: string) => call<unknown>('create_channel', { communityId, name }),
  createVoiceChannel: (communityId: Id, name: string) => call<unknown>('create_voice_channel', { communityId, name }),
  updateChannel: (communityId: Id, channelId: Id, patch: ChannelPatch) =>
    call<ChannelView>('update_channel', { communityId, channelId, ...patch }),
  deleteChannel: (communityId: Id, channelId: Id) => call<void>('delete_channel', { communityId, channelId }),
  listInvites: (communityId: Id) => call<unknown>('list_invites', { communityId }),
  revokeInvite: (communityId: Id, inviteId: Id) => call<void>('revoke_invite', { communityId, inviteId }),
  channelMessages: (communityId: Id, channelId: Id, limit: number) =>
    call<unknown[]>('channel_messages', { communityId, channelId, limit }),
  // mentions is sent only when the text mentions someone (see lib/mentions.ts mentionArg).
  sendMessage: (communityId: Id, channelId: Id, text: string, mentions?: { users: Id[]; everyone: boolean }) =>
    call<unknown>('send_message', mentions ? { communityId, channelId, text, mentions } : { communityId, channelId, text }),

  // ---- typing indicators ----
  typing: (target: TypingTarget) => call<void>('typing', { target }),
  stopTyping: (target: TypingTarget) => call<void>('stop_typing', { target }),
  typingIn: (target: TypingTarget) => call<string[]>('typing_in', { target }),

  // ---- messages in a channel ----
  editMessage: (communityId: Id, channelId: Id, messageId: Id, text: string) =>
    call<unknown>('edit_message', { communityId, channelId, messageId, text }),
  deleteMessage: (communityId: Id, channelId: Id, messageId: Id) =>
    call<void>('delete_message', { communityId, channelId, messageId }),
  sendReply: (communityId: Id, channelId: Id, messageId: Id, text: string, mentions?: { users: Id[]; everyone: boolean }) =>
    call<unknown>('send_reply', mentions ? { communityId, channelId, messageId, text, mentions } : { communityId, channelId, messageId, text }),
  markMentionsRead: (communityId: Id, channelId: Id) => call<void>('mark_mentions_read', { communityId, channelId }),
  getServerPrefs: (communityId: Id) => call<ServerPrefs>('get_server_prefs', { communityId }),
  setServerPrefs: (communityId: Id, prefs: ServerPrefs) => call<void>('set_server_prefs', { communityId, prefs }),
  serverAlerts: (communityId: Id, mentioned: boolean, byName: boolean) => call<boolean>('server_alerts', { communityId, mentioned, byName }),
  react: (communityId: Id, channelId: Id, messageId: Id, emoji: string) =>
    call<void>('react', { communityId, channelId, messageId, emoji }),
  unreact: (communityId: Id, channelId: Id, messageId: Id, emoji: string) =>
    call<void>('unreact', { communityId, channelId, messageId, emoji }),

  // ---- the same actions in a direct message (`peer`) and a group (`groupId`) ----
  dmReply: (peer: Id, messageId: Id, text: string) => call<unknown>('dm_reply', { peer, messageId, text }),
  dmReact: (peer: Id, messageId: Id, emoji: string) => call<void>('dm_react', { peer, messageId, emoji }),
  dmUnreact: (peer: Id, messageId: Id, emoji: string) => call<void>('dm_unreact', { peer, messageId, emoji }),
  dmEdit: (peer: Id, messageId: Id, text: string) => call<unknown>('dm_edit', { peer, messageId, text }),
  dmDelete: (peer: Id, messageId: Id) => call<void>('dm_delete', { peer, messageId }),
  groupReply: (groupId: Id, messageId: Id, text: string) => call<unknown>('group_reply', { groupId, messageId, text }),
  groupReact: (groupId: Id, messageId: Id, emoji: string) => call<void>('group_react', { groupId, messageId, emoji }),
  groupUnreact: (groupId: Id, messageId: Id, emoji: string) => call<void>('group_unreact', { groupId, messageId, emoji }),
  groupEdit: (groupId: Id, messageId: Id, text: string) => call<unknown>('group_edit', { groupId, messageId, text }),
  groupDelete: (groupId: Id, messageId: Id) => call<void>('group_delete', { groupId, messageId }),

  // ---- file attachments ----
  pickFiles: () => call<string[]>('pick_files'),
  // `seed` keeps an encrypted copy on the community seeder (servers only). Defaults to true on the engine side.
  sendFiles: (communityId: Id, channelId: Id, paths: string[], text: string, opts: SendFileOptions = {}) =>
    call<unknown>('send_files', { communityId, channelId, paths, text, seed: opts.seed ?? true, ...(opts.mentions ? { mentions: opts.mentions } : {}) }),
  sendDmFiles: (peer: Id, paths: string[], text: string) => call<unknown>('send_dm_files', { peer, paths, text }),
  downloadAttachment: (communityId: Id | null, messageId: Id, attachmentId: Id) =>
    call<unknown>('download_attachment', { communityId, messageId, attachmentId }),
  // Both accept only a file Dissent saved, in whichever download folder it used.
  openAttachment: (path: string) => call<void>('open_attachment', { path }),
  revealAttachment: (path: string) => call<void>('reveal_attachment', { path }),

  // ---- moderation and server settings ----
  moderate: (communityId: Id, action: string, user: Id) => call<void>('moderate', { communityId, action, user }),
  memberOnlyStatus: (communityId: Id) => call<unknown>('member_only_status', { communityId }),
  setMemberOnly: (communityId: Id, on: boolean) => call<void>('set_member_only', { communityId, on }),
  setHistoryVisibility: (communityId: Id, setting: string) => call<void>('set_history_visibility', { communityId, setting }),
  claimHandle: (communityId: Id, handle: string) => call<unknown>('claim_handle', { communityId, handle }),
  userProfile: (user: Id) => call<UserProfileView>('user_profile', { user }),
  safetyNumber: (user: Id) => call<unknown>('safety_number', { user }),

  // ---- updates ----
  checkUpdate: () => call<UpdateInfo | null>('check_update'),
  installUpdate: () => call<void>('install_update'),
  openReleasePage: () => call<void>('open_release_page'),

  // ---- portable exes: `chooser` until a first-run choice is saved, then `locked` until the passphrase opens the profile ----
  vaultStatus: () => call<VaultStatus>('vault_status'),
  chooseProfile: (choice: string) => call<void>('choose_profile', { choice }),
  // Resolves to 'link' (approve this device first) or 'open'.
  unlockProfile: (passphrase: string) => call<'link' | 'open'>('unlock_profile', { passphrase }),
  finishPortableLink: (grant: string) => call<void>('finish_portable_link', { grant }),

  // ---- direct messages ----
  listConversations: () => call<ConversationSummary[]>('list_conversations'),
  dmMessages: (peer: Id, limit: number) => call<unknown[]>('dm_messages', { peer, limit }),
  sendDm: (peer: Id, text: string) => call<unknown>('send_dm', { peer, text }),
  retryMessage: (messageId: Id) => call<void>('retry_message', { messageId }),

  // ---- app and network ----
  appVersion: () => call<string>('app_version'),
  netStatus: () => call<NetStatus>('net_status'),
  getNetworkSettings: () => call<NetworkSettings>('get_network_settings'),
  shellSettings: () => call<ShellSettings>('shell_settings'),
  setShellSettings: (settings: Omit<ShellSettings, 'start_at_login_available' | 'start_at_login_note'>) =>
    call<ShellSettings>('set_shell_settings', { settings }),
  setNetworkSettings: (settings: NetworkSettings) => call<NetworkSettings>('set_network_settings', { settings }),
  identityExportPhrase: () => call<string>('identity_export_phrase'),
  identityImportPhrase: (phrase: string) => call<unknown>('identity_import_phrase', { phrase }),

  // ---- not in the engine yet: see Part 6 of the design doc ----
  capabilities: () => call<Capabilities | string[]>('capabilities'),
  setNote: (user: Id, kind: string, text: string) => call<void>('set_note', { user, kind, text }),

  // ---- contacts ----
  listContacts: () => call<ContactsView>('list_contacts'),
  sendContactRequest: (link: string) => call<unknown>('send_contact_request', { link }),
  addressDiscovery: () => call<boolean>('address_discovery'),
  setAddressDiscovery: (on: boolean) => call<void>('set_address_discovery', { on }),
  friendsShared: () => call<boolean>('friends_shared'),
  setFriendsShared: (on: boolean) => call<boolean>('set_friends_shared', { on }),
  acceptContact: (rootId: Id) => call<void>('accept_contact', { rootId }),
  declineContact: (rootId: Id) => call<void>('decline_contact', { rootId }),
  cancelContactRequest: (rootId: Id) => call<void>('cancel_contact_request', { rootId }),
  removeContact: (rootId: Id) => call<void>('remove_contact', { rootId }),
  blockUser: (rootId: Id) => call<void>('block_user', { rootId }),
  unblockUser: (rootId: Id) => call<void>('unblock_user', { rootId }),
  closeConversation: (key: string) => call<void>('close_conversation', { key }),
  ignoreUser: (user: Id) => call<void>('ignore_user', { user }),
  unignoreUser: (user: Id) => call<void>('unignore_user', { user }),
  ignoredUsers: () => call<Id[]>('ignored_users'),
  peerContactLink: (rootId: Id) => call<string>('peer_contact_link', { rootId }),

  // ---- group conversations ----
  createGroup: (name: string, peers: Id[]) => call<unknown>('create_group', { name, peers }),
  groupMessages: (groupId: Id, limit: number) => call<unknown[]>('group_messages', { groupId, limit }),
  sendGroupMessage: (groupId: Id, text: string) => call<unknown>('send_group_message', { groupId, text }),
  addGroupMembers: (groupId: Id, peers: Id[]) => call<void>('add_group_members', { groupId, peers }),
  leaveGroup: (groupId: Id) => call<void>('leave_group', { groupId }),
  groupInviteLink: (groupId: Id) => call<string>('group_invite_link', { groupId }),

  // ---- calls ----
  joinCall: (communityId: Id, channelId: Id) => call<unknown>('join_call', { communityId, channelId }),
  startCall: (peer: Id) => call<unknown>('start_call', { peer }),
  leaveCall: () => call<void>('leave_call'),
  callState: () => call<CallState | null>('call_state'),
  setMuted: (muted: boolean) => call<void>('set_muted', { muted }),
  setDeafened: (deafened: boolean) => call<void>('set_deafened', { deafened }),
  setCamera: (on: boolean) => call<void>('set_camera', { on }),
  setCameraPreview: (on: boolean) => call<void>('set_camera_preview', { on }),
  setScreenShare: (on: boolean, target?: ShareSource['target']) => call<void>('set_screen_share', { on, target: target ?? null }),
  listShareSources: () => call<ShareSource[]>('list_share_sources'),
  sendCallMessage: (text: string) => call<unknown>('send_call_message', { text }),
  acceptCall: (callId: string) => call<unknown>('accept_call', { callId }),
  declineCall: (callId: string) => call<void>('decline_call', { callId }),
  joinGroupCall: (groupId: Id) => call<unknown>('join_group_call', { groupId }),
  joinDmCall: (peer: Id) => call<unknown>('join_dm_call', { peer }),
  audioDevices: () => call<AudioDevices>('audio_devices'),
  setAudioDevices: (input: string | null, output: string | null) => call<void>('set_audio_devices', { input, output }),
  voiceSettings: () => call<VoiceSettings>('voice_settings'),
  setVoiceSettings: (settings: VoiceSettings) => call<void>('set_voice_settings', { settings }),
  micTestStart: (playback: boolean) => call<void>('mic_test_start', { playback }),
  micTestStop: () => call<void>('mic_test_stop'),
  storeStats: () => call<StoreStats>('store_stats'),

  // ---- transfers ----
  // A bare array in older builds; `{ transfers, rates }` once the engine reports transfer rates.
  listTransfers: () => call<TransferItem[] | { transfers: TransferItem[]; rates?: TransferRates | null }>('list_transfers'),
  pauseTransfer: (transferId: Id) => call<void>('pause_transfer', { transferId }),
  resumeTransfer: (transferId: Id) => call<void>('resume_transfer', { transferId }),
  cancelTransfer: (transferId: Id) => call<void>('cancel_transfer', { transferId }),
  retryTransfer: (transferId: Id) => call<void>('retry_transfer', { transferId }),
  removeTransfer: (transferId: Id) => call<void>('remove_transfer', { transferId }),
  transferSettings: () => call<TransferSettings>('transfer_settings'),
  setTransferSettings: (settings: TransferSettings) => call<void>('set_transfer_settings', { settings }),
  pickDownloadDir: () => call<TransferSettings>('pick_download_dir'),
  resetDownloadDir: () => call<TransferSettings>('reset_download_dir'),

  // ---- recovery guardians and devices ----
  listGuardians: () => call<GuardianList>('list_guardians'),
  signGuardians: (guardians: Id[], threshold: number) => call<GuardianList>('sign_guardians', { guardians, threshold }),
  listDevices: () => call<DeviceView[]>('list_devices'),
  revokeDevice: (deviceId: Id) => call<void>('revoke_device', { deviceId }),
  linkDeviceCode: () => call<string>('link_device_code'),

  // ---- safety numbers, pins and read markers ----
  markSafetyVerified: (user: Id) => call<void>('mark_safety_verified', { user }),
  pinnedMessages: (communityId: Id, channelId: Id) => call<unknown[]>('pinned_messages', { communityId, channelId }),
  pinConversation: (key: string, pinned: boolean) => call<void>('pin_conversation', { key, pinned }),
  markRead: (key: string) => call<void>('mark_read', { key }),
  completeFirstRun: () => call<void>('complete_first_run'),
  pinMessage: (communityId: Id, channelId: Id, messageId: Id, pinned: boolean) =>
    call<void>('pin_message', { communityId, channelId, messageId, pinned }),
  pinnedConversations: () => call<string[]>('pinned_conversations'),
  readMarkers: () => call<Record<string, unknown>>('read_markers'),
  approveDevice: (link: string) => call<unknown>('approve_device', { link }),
  finishDeviceLink: (grant: string) => call<unknown>('finish_device_link', { grant }),

  // ---- saved messages, reminders and unread markers ----
  addBookmark: (message: MessageRef) => call<void>('add_bookmark', { message }),
  removeBookmark: (key: string) => call<void>('remove_bookmark', { key }),
  listBookmarks: () => call<Bookmark[]>('list_bookmarks'),
  addReminder: (message: MessageRef, atMs: number) => call<Reminder>('add_reminder', { message, atMs }),
  listReminders: () => call<Reminder[]>('list_reminders'),
  removeReminder: (id: Id) => call<void>('remove_reminder', { id }),
  markUnread: (key: string, beforeMs: number) => call<void>('mark_unread', { key, beforeMs }),

  // ---- your own status; untilMs is epoch milliseconds, or null for no end ----
  getStatus: () => call<UserStatus>('get_status'),
  setStatus: (status: StatusName, untilMs: number | null) => call<void>('set_status', { status, untilMs }),
  // Any user input, for auto-idle. The UI calls it at most once every 30 seconds.
  recordActivity: () => call<void>('record_activity'),
};

// Calls handler with each EngineEvent payload. Resolves to the unlisten function.
export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<Unlisten> {
  return listenTo<EngineEvent>('dissent://event', handler);
}

// Subscribes to any named Tauri event. Resolves to the unlisten function.
export function listenTo<T = unknown>(name: string, handler: (payload: T) => void): Promise<Unlisten> {
  const tauri = window.__TAURI__;
  if (!tauri) return Promise.reject(new Error('No backend: the app has not loaded the Tauri globals or the mock.'));
  return tauri.event.listen<T>(name, (e) => handler(e.payload));
}

