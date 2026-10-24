// Every Tauri command the UI calls, in one place. Args are camelCase (Tauri maps them to snake_case params).
// Commands below the "not in the engine yet" line are the contracts in docs/design/07-visual-system.md, Part 6.
// The UI calls them only after `capabilities` reports the feature, so an engine without them never sees the call.
const call = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

export const api = {
  me: () => call('me'),
  setDisplayName: (name) => call('set_display_name', { name }),
  contactLink: () => call('contact_link'),
  addContact: (link) => call('add_contact', { link }),

  listCommunities: () => call('list_communities'),
  createCommunity: (name) => call('create_community', { name }),
  createInvite: (communityId) => call('create_invite', { communityId }),
  joinCommunity: (invite) => call('join_community', { invite }),
  leaveCommunity: (communityId) => call('leave_community', { communityId }),
  communityView: (communityId) => call('community_view', { communityId }),
  createChannel: (communityId, name) => call('create_channel', { communityId, name }),
  channelMessages: (communityId, channelId, limit) =>
    call('channel_messages', { communityId, channelId, limit }),
  sendMessage: (communityId, channelId, text) => call('send_message', { communityId, channelId, text }),
  // Typing indicators. `target` is { kind: 'channel', communityId, channelId }, { kind: 'dm', peer } or { kind: 'group', groupId }.
  typing: (target) => call('typing', { target }),
  stopTyping: (target) => call('stop_typing', { target }),
  typingIn: (target) => call('typing_in', { target }),
  editMessage: (communityId, channelId, messageId, text) =>
    call('edit_message', { communityId, channelId, messageId, text }),
  deleteMessage: (communityId, channelId, messageId) =>
    call('delete_message', { communityId, channelId, messageId }),
  // Replies and reactions. The UI shows them only when message items carry a `reactions` array.
  sendReply: (communityId, channelId, messageId, text) =>
    call('send_reply', { communityId, channelId, messageId, text }),
  react: (communityId, channelId, messageId, emoji) => call('react', { communityId, channelId, messageId, emoji }),
  unreact: (communityId, channelId, messageId, emoji) => call('unreact', { communityId, channelId, messageId, emoji }),
  // File attachments. The UI shows them only when message items carry an `attachments` array.
  pickFiles: () => call('pick_files'),
  // `seed` keeps an encrypted copy on the community seeder (servers only). Defaults to true on the engine side.
  sendFiles: (communityId, channelId, paths, text, opts = {}) => call('send_files', { communityId, channelId, paths, text, seed: opts.seed ?? true }),
  sendDmFiles: (peer, paths, text) => call('send_dm_files', { peer, paths, text }),
  downloadAttachment: (communityId, messageId, attachmentId) =>
    call('download_attachment', { communityId, messageId, attachmentId }),
  // Both accept only a path inside the app's downloads directory.
  openAttachment: (path) => call('open_attachment', { path }),
  revealAttachment: (path) => call('reveal_attachment', { path }),
  moderate: (communityId, action, user) => call('moderate', { communityId, action, user }),
  claimHandle: (communityId, handle) => call('claim_handle', { communityId, handle }),
  userProfile: (user) => call('user_profile', { user }),
  safetyNumber: (user) => call('safety_number', { user }),
  checkUpdate: () => call('check_update'),
  installUpdate: () => call('install_update'),

  listConversations: () => call('list_conversations'),
  dmMessages: (peer, limit) => call('dm_messages', { peer, limit }),
  sendDm: (peer, text) => call('send_dm', { peer, text }),

  appVersion: () => call('app_version'),
  netStatus: () => call('net_status'),
  getNetworkSettings: () => call('get_network_settings'),
  setNetworkSettings: (settings) => call('set_network_settings', { settings }),
  identityExportPhrase: () => call('identity_export_phrase'),
  identityImportPhrase: (phrase) => call('identity_import_phrase', { phrase }),

  // ---- not in the engine yet: see Part 6 of the design doc ----
  capabilities: () => call('capabilities'),
  setNote: (user, kind, text) => call('set_note', { user, kind, text }),
  // contacts
  listContacts: () => call('list_contacts'),
  sendContactRequest: (link) => call('send_contact_request', { link }),
  acceptContact: (rootId) => call('accept_contact', { rootId }),
  declineContact: (rootId) => call('decline_contact', { rootId }),
  cancelContactRequest: (rootId) => call('cancel_contact_request', { rootId }),
  removeContact: (rootId) => call('remove_contact', { rootId }),
  blockUser: (rootId) => call('block_user', { rootId }),
  unblockUser: (rootId) => call('unblock_user', { rootId }),
  closeConversation: (key) => call('close_conversation', { key }),
  ignoreUser: (user) => call('ignore_user', { user }),
  unignoreUser: (user) => call('unignore_user', { user }),
  ignoredUsers: () => call('ignored_users'),
  peerContactLink: (rootId) => call('peer_contact_link', { rootId }),
  // group conversations
  createGroup: (name, peers) => call('create_group', { name, peers }),
  groupMessages: (groupId, limit) => call('group_messages', { groupId, limit }),
  sendGroupMessage: (groupId, text) => call('send_group_message', { groupId, text }),
  addGroupMembers: (groupId, peers) => call('add_group_members', { groupId, peers }),
  leaveGroup: (groupId) => call('leave_group', { groupId }),
  groupInviteLink: (groupId) => call('group_invite_link', { groupId }),
  // calls
  joinCall: (communityId, channelId) => call('join_call', { communityId, channelId }),
  startCall: (peer) => call('start_call', { peer }),
  leaveCall: () => call('leave_call'),
  callState: () => call('call_state'),
  setMuted: (muted) => call('set_muted', { muted }),
  setDeafened: (deafened) => call('set_deafened', { deafened }),
  setCamera: (on) => call('set_camera', { on }),
  sendCallMessage: (text) => call('send_call_message', { text }),
  acceptCall: (callId) => call('accept_call', { callId }),
  declineCall: (callId) => call('decline_call', { callId }),
  joinGroupCall: (groupId) => call('join_group_call', { groupId }),
  joinDmCall: (peer) => call('join_dm_call', { peer }),
  audioDevices: () => call('audio_devices'),
  setAudioDevices: (input, output) => call('set_audio_devices', { input, output }),
  storeStats: () => call('store_stats'),
  // transfers
  listTransfers: () => call('list_transfers'),
  pauseTransfer: (transferId) => call('pause_transfer', { transferId }),
  resumeTransfer: (transferId) => call('resume_transfer', { transferId }),
  cancelTransfer: (transferId) => call('cancel_transfer', { transferId }),
  retryTransfer: (transferId) => call('retry_transfer', { transferId }),
  removeTransfer: (transferId) => call('remove_transfer', { transferId }),
  transferSettings: () => call('transfer_settings'),
  setTransferSettings: (settings) => call('set_transfer_settings', { settings }),
  pickDownloadDir: () => call('pick_download_dir'),
  // recovery guardians and devices
  listGuardians: () => call('list_guardians'),
  signGuardians: (guardians, threshold) => call('sign_guardians', { guardians, threshold }),
  listDevices: () => call('list_devices'),
  revokeDevice: (deviceId) => call('revoke_device', { deviceId }),
  linkDeviceCode: () => call('link_device_code'),
  // safety numbers and pins
  markSafetyVerified: (user) => call('mark_safety_verified', { user }),
  pinnedMessages: (communityId, channelId) => call('pinned_messages', { communityId, channelId }),
  pinConversation: (key, pinned) => call('pin_conversation', { key, pinned }),
  markRead: (key) => call('mark_read', { key }),
  completeFirstRun: () => call('complete_first_run'),
  pinMessage: (communityId, channelId, messageId, pinned) => call('pin_message', { communityId, channelId, messageId, pinned }),
  pinnedConversations: () => call('pinned_conversations'),
  readMarkers: () => call('read_markers'),
  approveDevice: (link) => call('approve_device', { link }),
  finishDeviceLink: (grant) => call('finish_device_link', { grant }),
  // your own status; untilMs is epoch milliseconds, or null for no end
  getStatus: () => call('get_status'),
  setStatus: (status, untilMs) => call('set_status', { status, untilMs }),
  // Any user input, for auto-idle. The UI calls it at most once every 30 seconds.
  recordActivity: () => call('record_activity'),
};

// Calls handler with each EngineEvent payload, e.g. { kind: 'message', community_id, channel_id }.
export function onEngineEvent(handler) {
  return window.__TAURI__.event.listen('dissent://event', (e) => handler(e.payload));
}

// Subscribes to any named Tauri event. Returns a promise of the unlisten function.
export function listenTo(name, handler) {
  return window.__TAURI__.event.listen(name, (e) => handler(e.payload));
}
