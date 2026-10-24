// Everything that talks to the backend and then updates state. Views call these; they never call api directly.
import { api } from './api.js';
import { state, notify } from './state.js';
import { toast } from './ui.js';
import { seenMs, markSeen } from './unread.js';
import { has } from './features.js';

const MSG_LIMIT = 200;

// Wraps a user-triggered action: errors become toasts and the result is undefined (falsy).
function guard(fn) {
  return async (...args) => {
    try {
      return await fn(...args);
    } catch (err) {
      toast(err);
      return undefined;
    }
  };
}

// ---- loading (plain functions that may throw; callers decide how to report) ----

export async function boot() {
  state.route = { kind: 'home' };
  state.community = null;
  state.channelId = null;
  state.messages = [];
  state.peer = null;
  state.group = null;
  state.dmMessages = [];
  state.ui.page = null;
  state.me = await api.me();
  await loadCommunities();
  await loadConversations();
  await refreshUnread();
  await Promise.all([loadContacts(), loadIgnored(), loadTransfers(), loadCallState()]);
  notify('rail', 'sidebar', 'main', 'right', 'me');
}

export async function loadCommunities() {
  state.communities = await api.listCommunities();
  notify('rail');
  if (state.route.kind === 'server' && !state.communities.some((c) => c.id === state.route.communityId)) {
    await openHome();
  }
}

export async function loadConversations() {
  state.conversations = await api.listConversations();
  if (state.group) state.group = state.conversations.find((c) => c.kind === 'group' && c.id === state.group.id) ?? state.group;
  notify('sidebar', 'right');
}

async function loadCommunity(communityId) {
  const view = await api.communityView(communityId);
  if (state.route.kind !== 'server' || state.route.communityId !== communityId) return;
  state.community = view;
  if (state.voiceChannelId && !(view.voice_channels ?? []).some((c) => c.id === state.voiceChannelId)) state.voiceChannelId = null;
  if (!state.voiceChannelId && !view.channels.some((c) => c.id === state.channelId)) {
    state.channelId = (view.channels.find((c) => c.name === 'general') ?? view.channels[0])?.id ?? null;
  }
  notify('sidebar', 'right');
  await loadMessages();
}

async function loadMessages() {
  const community = state.community;
  const channelId = state.channelId;
  if (!community || !channelId) {
    state.messages = [];
    notify('main');
    return;
  }
  const messages = await api.channelMessages(community.id, channelId, MSG_LIMIT);
  if (state.community?.id !== community.id || state.channelId !== channelId) return;
  state.messages = messages;
  state.historyStart = messages.length < MSG_LIMIT;
  notify('main');
}

let dmTimer = null;

async function loadDm() {
  const peer = state.peer;
  if (!peer) return;
  const group = state.group;
  const items = group ? await api.groupMessages(group.id, MSG_LIMIT) : await api.dmMessages(peer, MSG_LIMIT);
  if (state.peer !== peer) return;
  state.dmMessages = items;
  state.historyStart = items.length < MSG_LIMIT;
  notify('main');
  // Pending DMs are retried by the engine outbox, so re-read while any are still in flight.
  clearTimeout(dmTimer);
  dmTimer = null;
  if (items.some((m) => m.pending)) {
    dmTimer = setTimeout(() => loadDm().catch(() => {}), 5000);
  }
}

// ---- navigation ----

function resetConversation() {
  state.community = null;
  state.channelId = null;
  state.voiceChannelId = null;
  state.messages = [];
  state.ui.page = null;
  state.ui.transferId = null;
}

export const openHome = guard(async () => {
  state.route = { kind: 'home' };
  resetConversation();
  notify('rail', 'sidebar', 'main', 'right', 'title');
  await loadConversations();
});

export const openServer = guard(async (communityId, channelId = null) => {
  state.route = { kind: 'server', communityId };
  resetConversation();
  state.channelId = channelId;
  notify('rail', 'sidebar', 'main', 'right', 'title');
  await loadCommunity(communityId);
  // The first channel is picked automatically; open it the same way a click would, so unread markers apply.
  if (state.route.kind === 'server' && state.channelId) await openChannel(state.channelId);
});

export const openChannel = guard(async (channelId) => {
  const key = `ch:${state.community?.id}:${channelId}`;
  const seenBefore = seenMs(key);
  state.channelId = channelId;
  state.voiceChannelId = null;
  state.ui.page = null;
  state.ui.transferId = null;
  state.messages = [];
  state.newFromId = null;
  state.loading.messages = true;
  notify('sidebar', 'main', 'right', 'title');
  try {
    await loadMessages();
  } finally {
    state.loading.messages = false;
    notify('main');
  }
  if (seenBefore !== null) state.newFromId = state.messages.find((m) => Number(m.ms) > seenBefore)?.id ?? null;
  markLatestRead(key, state.messages);
  notify('sidebar', 'rail', 'main');
});

// Opens a voice channel's call board without joining; Join is a separate step on the board.
export const openVoiceChannel = guard(async (channelId) => {
  state.voiceChannelId = channelId;
  state.ui.page = null;
  state.ui.transferId = null;
  notify('sidebar', 'main', 'right', 'title');
  // Opening a voice channel joins its call (calls feature), so the call pane shows the people in it.
  if (has('calls') && state.community && state.call?.channel_id !== channelId) {
    await joinCall(state.community.id, channelId);
  }
});

export const openDm = guard(async (peer, displayName = '') => {
  state.route = { kind: 'home' };
  resetConversation();
  state.peer = peer;
  state.peerName = displayName;
  state.group = null;
  state.dmMessages = [];
  state.peerProfile = null;
  notify('rail', 'sidebar', 'main', 'right', 'title');
  state.loading.messages = true;
  notify('main');
  try {
    await Promise.all([loadDm(), loadPeerProfile(peer)]);
  } finally {
    state.loading.messages = false;
    notify('main');
  }
  markLatestRead(`dm:${peer}`, state.dmMessages);
  notify('sidebar', 'rail');
});

// A group conversation (groups feature). `conv` is its ConversationSummary.
export const openGroup = guard(async (conv) => {
  state.route = { kind: 'home' };
  resetConversation();
  state.peer = conv.id;
  state.peerName = conv.name;
  state.group = conv;
  state.dmMessages = [];
  state.peerProfile = null;
  notify('rail', 'sidebar', 'main', 'right', 'title');
  state.loading.messages = true;
  notify('main');
  try {
    await loadDm();
  } finally {
    state.loading.messages = false;
    notify('main');
  }
  markLatestRead(`group:${conv.id}`, state.dmMessages);
  notify('sidebar', 'rail');
});

export function openConversation(conv) {
  return conv.kind === 'group' ? openGroup(conv) : openDm(conv.peer, conv.display_name);
}

// Pages replace the chat pane on Home: 'contacts' or 'transfers'. null returns to the conversation.
export const openPage = guard(async (page) => {
  if (page === 'contacts' && !has('contacts')) return;
  if (page === 'transfers' && !has('transfers')) return;
  if (state.route.kind !== 'home') {
    state.route = { kind: 'home' };
    resetConversation();
  }
  state.ui.page = page;
  state.ui.transferId = null;
  notify('rail', 'sidebar', 'main', 'right', 'title');
  if (page === 'contacts') await loadContacts();
  if (page === 'transfers') await loadTransfers();
});

async function loadPeerProfile(peer) {
  try {
    const view = await api.userProfile(peer);
    if (state.peer !== peer) return;
    state.peerProfile = view;
    notify('right', 'main');
  } catch {
    state.peerProfile = null;
  }
}

// ---- servers ----

export const createCommunity = guard(async (name) => {
  const summary = await api.createCommunity(name);
  await loadCommunities();
  await openServer(summary.id);
  return summary;
});

export const joinCommunity = guard(async (invite) => {
  const summary = await api.joinCommunity(invite);
  await loadCommunities();
  await openServer(summary.id);
  return summary;
});

export const leaveCommunity = guard(async (communityId) => {
  await api.leaveCommunity(communityId);
  await loadCommunities();
  return true;
});

export const createInvite = guard((communityId) => api.createInvite(communityId));

export const createChannel = guard(async (communityId, name) => {
  const channel = await api.createChannel(communityId, name);
  await loadCommunity(communityId);
  await openChannel(channel.id);
  return true;
});

export const claimHandle = guard(async (communityId, handle) => {
  await api.claimHandle(communityId, handle);
  await loadCommunity(communityId);
  toast(`Handle @${handle} claimed`, 'info');
  return true;
});

export const moderate = guard(async (communityId, action, user) => {
  await api.moderate(communityId, action, user);
  await loadCommunity(communityId);
  return true;
});

// ---- channel messages ----

export const sendMessage = guard(async (text) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  await api.sendMessage(community.id, state.channelId, text);
  await loadMessages();
  markOwnSend();
  return true;
});

// After sending, the newest message is ours, so the seen marker moves past it and no "New messages" line
// appears above it when the channel is reopened.
function markOwnSend() {
  if (state.route.kind === 'server' && state.community && state.channelId) {
    markLatestRead(`ch:${state.community.id}:${state.channelId}`, state.messages);
  } else if (state.route.kind === 'home' && state.peer) {
    markLatestRead(state.group ? `group:${state.peer}` : `dm:${state.peer}`, state.dmMessages);
  }
}

export const editMessage = guard(async (messageId, text) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  await api.editMessage(community.id, state.channelId, messageId, text);
  await loadMessages();
  return true;
});

export const deleteMessage = guard(async (messageId) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  await api.deleteMessage(community.id, state.channelId, messageId);
  await loadMessages();
  return true;
});

// ---- replies and reactions (only offered when the backend reports them, see views/chat.js) ----

export const sendReply = guard(async (messageId, text) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  await api.sendReply(community.id, state.channelId, messageId, text);
  await loadMessages();
  markOwnSend();
  return true;
});

// Adds the reaction, or removes it when `mine` says the current user already reacted with it.
export const toggleReaction = guard(async (messageId, emoji, mine) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  if (mine) await api.unreact(community.id, state.channelId, messageId, emoji);
  else await api.react(community.id, state.channelId, messageId, emoji);
  await loadMessages();
  return true;
});

// ---- file attachments (only offered when the backend reports them, see views/chat.js) ----

export const pickFiles = guard(() => api.pickFiles());

// The file whose details panel is open in the right column (null closes it).
export function openFileDetail(attachmentId) {
  state.ui.fileDetail = attachmentId;
  notify('right');
}

// Sends files with an optional caption. target: { kind: 'channel', communityId, channelId } | { kind: 'dm', peer }.
// Without a target, the open conversation receives them.
export const sendFiles = guard(async (paths, text, target = null) => {
  const t = target ?? (state.route.kind === 'home' && state.peer
    ? { kind: 'dm', peer: state.peer }
    : { kind: 'channel', communityId: state.community?.id, channelId: state.channelId });
  if (t.kind === 'dm') {
    await api.sendDmFiles(t.peer, paths, text ?? null);
    if (state.peer === t.peer) await loadDm();
    await loadConversations();
  } else {
    if (!t.communityId || !t.channelId) return false;
    await api.sendFiles(t.communityId, t.channelId, paths, text ?? null, { seed: t.seed ?? true });
    if (state.community?.id === t.communityId && state.channelId === t.channelId) await loadMessages();
  }
  markOwnSend();
  if (has('transfers')) await loadTransfers();
  return true;
});

export const downloadAttachment = guard(async (messageId, attachmentId) => {
  const communityId = state.route.kind === 'server' ? state.community?.id ?? null : null;
  await api.downloadAttachment(communityId, messageId, attachmentId);
  return true;
});

export const openAttachment = guard((localPath) => api.openAttachment(localPath));
export const revealAttachment = guard((localPath) => api.revealAttachment(localPath));

// Progress events update the attachment in place; the next message reload brings the final state.
export function handleAttachmentProgress(p) {
  if (!p) return;
  for (const items of [state.messages, state.dmMessages]) {
    const m = items.find((x) => x.id === p.message_id);
    const a = m?.attachments?.find((x) => x.id === p.attachment_id);
    if (a) {
      a.state = 'downloading';
      a.progress = Math.max(0, Math.min(1, Number(p.progress) || 0));
      if (p.rate_bps !== undefined) a.rate_bps = p.rate_bps;
      if (p.eta_s !== undefined) a.eta_s = p.eta_s;
      if (p.transfer !== undefined) a.transfer = p.transfer;
      notify('main');
      return;
    }
  }
}

// ---- direct messages and groups ----

export const sendDm = guard(async (text) => {
  const peer = state.peer;
  if (!peer) return false;
  if (state.group) await api.sendGroupMessage(state.group.id, text);
  else await api.sendDm(peer, text);
  await Promise.all([loadDm(), loadConversations()]);
  markOwnSend();
  return true;
});

export const addContact = guard(async (link) => {
  const user = await api.addContact(link);
  await loadConversations();
  await openDm(user.root_id, user.display_name);
  return true;
});

export const contactLink = guard(() => api.contactLink());

export const createGroup = guard(async (name, peers) => {
  const conv = await api.createGroup(name, peers);
  await loadConversations();
  await openGroup(conv);
  return conv;
});

export const leaveGroup = guard(async (groupId) => {
  await api.leaveGroup(groupId);
  await loadConversations();
  if (state.group?.id === groupId) await openHome();
  return true;
});

export const groupInviteLink = guard((groupId) => api.groupInviteLink(groupId));
export const addGroupMembers = guard(async (groupId, peers) => {
  await api.addGroupMembers(groupId, peers);
  await loadConversations();
  await loadDm();
  return true;
});

// ---- contacts (contacts feature) ----

// Ignored users: their messages render collapsed. Without the command nobody is ignored.
export async function loadIgnored() {
  if (!has('ignore')) return;
  try {
    state.ignored = new Set((await api.ignoredUsers()).map((x) => x.root_id ?? x));
  } catch {
    state.ignored = new Set();
  }
  notify('main', 'sidebar');
}

export const setIgnored = guard(async (rootId, ignore) => {
  if (ignore) await api.ignoreUser(rootId);
  else await api.unignoreUser(rootId);
  await loadIgnored();
  return true;
});

// Your own status (the profile popover). The engine sends dissent://status-changed for every change, including timed ones ending.
const normalizeStatus = (s) => ({ status: s?.status ?? 'online', until_ms: s?.until_ms ?? null });

export async function loadStatus() {
  if (!has('status')) return;
  try {
    state.status = normalizeStatus(await api.getStatus());
    notify('status');
  } catch (err) {
    toast(err);
  }
}

export function applyStatus(payload) {
  state.status = normalizeStatus(payload);
  notify('status');
}

export const setStatus = guard(async (status, untilMs = null) => {
  await api.setStatus(status, untilMs);
  applyStatus({ status, until_ms: untilMs });
  return true;
});

export const closeDm = guard(async (key) => {
  await api.closeConversation(key);
  await loadConversations();
  return true;
});

export async function loadContacts() {
  if (!has('contacts')) return;
  try {
    state.contacts = await api.listContacts();
  } catch {
    state.contacts = null;
  }
  notify('sidebar', 'page');
}

const contactAction = (fn) => guard(async (...args) => {
  await fn(...args);
  await Promise.all([loadContacts(), loadConversations()]);
  return true;
});
export const sendContactRequest = contactAction((link) => api.sendContactRequest(link));
export const acceptContact = contactAction((rootId) => api.acceptContact(rootId));
export const declineContact = contactAction((rootId) => api.declineContact(rootId));
export const cancelContactRequest = contactAction((rootId) => api.cancelContactRequest(rootId));
export const removeContact = contactAction((rootId) => api.removeContact(rootId));
export const blockUser = contactAction((rootId) => api.blockUser(rootId));
export const unblockUser = contactAction((rootId) => api.unblockUser(rootId));
export const peerContactLink = guard((rootId) => api.peerContactLink(rootId));

// ---- transfers (transfers feature) ----

export async function loadTransfers() {
  if (!has('transfers')) return;
  try {
    const result = await api.listTransfers();
    state.transfers = Array.isArray(result) ? result : result?.transfers ?? [];
    state.transferRates = Array.isArray(result) ? null : result?.rates ?? null;
  } catch {
    state.transfers = [];
  }
  try {
    state.transferSettings = await api.transferSettings();
  } catch {
    state.transferSettings = null; // the Storage panel then shows no folder and no Change button
  }
  notify('sidebar', 'page', 'right', 'main');
}

const transferAction = (fn) => guard(async (id) => {
  await fn(id);
  await loadTransfers();
  return true;
});
export const pauseTransfer = transferAction((id) => api.pauseTransfer(id));
export const resumeTransfer = transferAction((id) => api.resumeTransfer(id));
export const cancelTransfer = transferAction((id) => api.cancelTransfer(id));
export const retryTransfer = transferAction((id) => api.retryTransfer(id));
export const removeTransfer = transferAction((id) => api.removeTransfer(id));
export const transferSettings = guard(() => api.transferSettings());
export const setTransferSettings = guard((settings) => api.setTransferSettings(settings));
export const pickDownloadDir = guard(() => api.pickDownloadDir());

// The transfer whose details panel is open in the right column (null closes it).
export function openTransferDetails(transferId) {
  state.ui.transferId = transferId;
  notify('right', 'main');
}

// dissent://transfers carries the full list, or { transfers, rates }.
export function handleTransfersEvent(payload) {
  if (!has('transfers')) return;
  if (Array.isArray(payload)) state.transfers = payload;
  else if (payload && Array.isArray(payload.transfers)) {
    state.transfers = payload.transfers;
    state.transferRates = payload.rates ?? state.transferRates;
  }
  notify('sidebar', 'page', 'right', 'main');
}

// ---- calls (calls feature) ----

export async function loadCallState() {
  if (!has('calls')) return;
  try {
    state.call = await api.callState();
  } catch {
    state.call = null;
  }
  notify('me', 'sidebar', 'main', 'right');
}

export function handleCallEvent(call) {
  if (!has('calls')) return;
  state.call = call ?? null;
  notify('me', 'sidebar', 'main', 'right', 'title');
}

export const joinCall = guard(async (communityId, channelId) => {
  state.call = await api.joinCall(communityId, channelId);
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

export const startCall = guard(async (peer) => {
  state.call = await api.startCall(peer);
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

export const leaveCall = guard(async () => {
  await api.leaveCall();
  state.call = null;
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

// Ringing: answer or decline a direct call. Answering with the mic muted mutes it once the call connects.
export const acceptCall = guard(async (callId, { muted = false } = {}) => {
  await api.acceptCall(callId);
  state.call = await api.callState();
  if (muted && state.call && !state.call.muted) {
    await api.setMuted(true);
    state.call.muted = true;
  }
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

export const declineCall = guard(async (callId) => {
  await api.declineCall(callId);
  return true;
});

// Joining a call that is already live (the lobby). Mic state comes from the lobby.
export const joinGroupCall = guard(async (groupId, { muted = false } = {}) => {
  state.call = await api.joinGroupCall(groupId);
  if (muted && state.call && !state.call.muted) { await api.setMuted(true); state.call.muted = true; }
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

export const joinDmCall = guard(async (peer, { muted = false } = {}) => {
  state.call = await api.joinDmCall(peer);
  if (muted && state.call && !state.call.muted) { await api.setMuted(true); state.call.muted = true; }
  notify('me', 'sidebar', 'main', 'right');
  return true;
});

const callToggle = (field, fn) => guard(async () => {
  if (!state.call) return false;
  const next = !state.call[field];
  await fn(next);
  state.call[field] = next;
  notify('me', 'main');
  return true;
});
export const toggleMuted = callToggle('muted', (v) => api.setMuted(v));
export const toggleDeafened = callToggle('deafened', (v) => api.setDeafened(v));
export const toggleCamera = callToggle('camera', (v) => api.setCamera(v));

export const sendCallMessage = guard(async (text) => {
  await api.sendCallMessage(text);
  await loadCallState();
  return true;
});

export function toggleCallChat() {
  state.ui.callChatOpen = !state.ui.callChatOpen;
  notify('main');
}

export function toggleCallDetails() {
  state.ui.callDetailsOpen = !state.ui.callDetailsOpen;
  notify('right', 'main');
}

// ---- recovery guardians and devices (settings) ----

export const listGuardians = guard(() => api.listGuardians());
export const signGuardians = guard(async (guardians, threshold) => {
  await api.signGuardians(guardians, threshold);
  toast('Guardian list signed and shared with your devices', 'info');
  return true;
});
export const listDevices = guard(() => api.listDevices());
export const revokeDevice = guard((deviceId) => api.revokeDevice(deviceId));
export const linkDeviceCode = guard(() => api.linkDeviceCode());

// ---- safety numbers, pins, read markers ----

export const markSafetyVerified = guard(async (user) => {
  await api.markSafetyVerified(user);
  if (state.peer === user) await loadPeerProfile(user);
  toast('Safety number marked as compared', 'info');
  return true;
});

export const pinnedMessages = guard(() => api.pinnedMessages(state.community?.id, state.channelId));

export const pinConversation = guard(async (key, pinned) => {
  await api.pinConversation(key, pinned);
  await loadConversations();
  return true;
});

export const markRead = guard(async (key) => {
  if (has('pins')) await api.markRead(key);
  state.unread.delete(key);
  notify('sidebar', 'rail');
  return true;
});

// ---- profile and settings ----

export const userProfile = guard((user) => api.userProfile(user));
export const safetyNumber = guard((user) => api.safetyNumber(user));

export const setDisplayName = guard(async (name) => {
  await api.setDisplayName(name);
  state.me = await api.me();
  notify('sidebar', 'me');
  toast('Display name saved', 'info');
  return true;
});

export const loadRecoveryPhrase = guard(() => api.identityExportPhrase());

export const restoreIdentity = guard(async (phrase) => {
  await api.identityImportPhrase(phrase);
  await boot();
  toast('Identity restored', 'info');
  return true;
});

export async function loadVersion() {
  state.version = await api.appVersion();
  notify('sidebar');
}

// ---- engine events: refresh only what the event touched ----

export const handleEngineEvent = guard(async (ev) => {
  switch (ev?.kind) {
    case 'communities_changed':
      await loadCommunities();
      return refreshUnread();
    case 'community_updated':
      if (state.route.kind === 'server' && state.route.communityId === ev.community_id) {
        return loadCommunity(ev.community_id);
      }
      return undefined;
    case 'message': {
      const key = `ch:${ev.community_id}:${ev.channel_id}`;
      if (state.route.kind === 'server' && state.community?.id === ev.community_id && state.channelId === ev.channel_id) {
        await loadMessages();
        markLatestRead(key, state.messages);
        return notify('sidebar', 'rail');
      }
      state.unread.add(key);
      return notify('sidebar', 'rail');
    }
    case 'dm': {
      await loadConversations();
      if (state.route.kind === 'home' && state.peer === ev.peer) {
        await loadDm();
        markLatestRead(`dm:${ev.peer}`, state.dmMessages);
        return notify('sidebar', 'rail');
      }
      state.unread.add(`dm:${ev.peer}`);
      return notify('sidebar', 'rail');
    }
    case 'group': {
      await loadConversations();
      if (state.route.kind === 'home' && state.group?.id === ev.group_id) {
        await loadDm();
        markLatestRead(`group:${ev.group_id}`, state.dmMessages);
        return notify('sidebar', 'rail');
      }
      state.unread.add(`group:${ev.group_id}`);
      return notify('sidebar', 'rail');
    }
    case 'contacts':
      return loadContacts();
    case 'presence':
      await loadConversations();
      if (state.route.kind === 'server' && state.community) await loadCommunity(state.community.id);
      return notify('main');
    default:
      return undefined;
  }
});

// ---- UI toggles (no backend call) ----

export function toggleMembers() {
  state.ui.membersOpen = !state.ui.membersOpen;
  notify('right', 'main');
}

export function toggleProfile() {
  state.ui.profileOpen = !state.ui.profileOpen;
  notify('right', 'main');
}

export function setFilter(text) {
  state.ui.filter = text;
  notify('main');
}

// ---- unread markers ----

// Marks a conversation read up to its newest message. Called when it is on screen.
function markLatestRead(key, items) {
  const last = items[items.length - 1];
  if (!last) return;
  markSeen(key, last.id, Number(last.ms));
  state.unread.delete(key);
}

export const convKey = (conv) => (conv.kind === 'group' ? `group:${conv.id}` : `dm:${conv.peer}`);

// Checks every channel and DM against its last-seen marker. A marker that does not exist yet is set to the
// current newest message, so an existing install does not start with everything unread.
export async function refreshUnread() {
  const openChannelKey = state.route.kind === 'server' && state.community && state.channelId
    ? `ch:${state.community.id}:${state.channelId}` : null;
  for (const c of state.communities) {
    let view;
    try {
      view = await api.communityView(c.id);
    } catch {
      continue;
    }
    for (const ch of view.channels) {
      const key = `ch:${c.id}:${ch.id}`;
      if (key === openChannelKey) continue;
      let latest;
      try {
        latest = (await api.channelMessages(c.id, ch.id, 1))[0];
      } catch {
        continue;
      }
      if (!latest) continue;
      const seen = seenMs(key);
      if (seen === null) markSeen(key, latest.id, Number(latest.ms));
      else if (Number(latest.ms) > seen) state.unread.add(key);
    }
  }
  const openPeer = state.route.kind === 'home' ? state.peer : null;
  for (const conv of state.conversations) {
    const key = convKey(conv);
    if ((conv.kind === 'group' ? conv.id : conv.peer) === openPeer) continue;
    const seen = seenMs(key);
    if (seen === null) markSeen(key, conv.peer ?? conv.id, Number(conv.last_ms));
    else if (Number(conv.last_ms) > seen) state.unread.add(key);
  }
  notify('rail', 'sidebar');
}

// ---- network settings ----

export const loadNetworkSettings = guard(() => api.getNetworkSettings());

// Saves relays and restarts the engine. The community and conversation lists are reloaded after the restart.
export const setNetworkSettings = guard(async (settings) => {
  const saved = await api.setNetworkSettings(settings);
  await loadCommunities();
  await loadConversations();
  toast('Network settings saved. The engine restarted.', 'info');
  return saved;
});

// ---- updates ----

// A newer release was found, by the background check or a manual one. Showing it brings the icon back.
export function setUpdate(info) {
  state.update = info;
  state.updateDismissed = false;
  notify('update');
}

// Private note or nickname on a contact (notes feature). kind is 'note' or 'nickname'.
export const setNote = guard(async (peer, kind, text) => {
  if (!has('notes')) return false;
  await api.setNote(peer, kind, text);
  await loadConversations();
  return true;
});

