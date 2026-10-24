// Every Tauri command the UI calls, in one place. Args are camelCase (Tauri maps them to snake_case params).
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
  sendFiles: (communityId, channelId, paths, text) => call('send_files', { communityId, channelId, paths, text }),
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
};

// Calls handler with each EngineEvent payload, e.g. { kind: 'message', community_id, channel_id }.
export function onEngineEvent(handler) {
  return window.__TAURI__.event.listen('dissent://event', (e) => handler(e.payload));
}

// Subscribes to any named Tauri event. Returns a promise of the unlisten function.
export function listenTo(name, handler) {
  return window.__TAURI__.event.listen(name, (e) => handler(e.payload));
}
