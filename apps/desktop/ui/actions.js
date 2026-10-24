// Everything that talks to the backend and then updates state. Views call these; they never call api directly.
import { api } from './api.js';
import { state, notify } from './state.js';
import { toast } from './ui.js';

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
  state.dmMessages = [];
  state.me = await api.me();
  await loadCommunities();
  await loadConversations();
  notify('rail', 'sidebar', 'main', 'right');
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
  notify('sidebar');
}

async function loadCommunity(communityId) {
  const view = await api.communityView(communityId);
  if (state.route.kind !== 'server' || state.route.communityId !== communityId) return;
  state.community = view;
  if (!view.channels.some((c) => c.id === state.channelId)) {
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
  notify('main');
}

let dmTimer = null;

async function loadDm() {
  const peer = state.peer;
  if (!peer) return;
  const items = await api.dmMessages(peer, MSG_LIMIT);
  if (state.peer !== peer) return;
  state.dmMessages = items;
  notify('main');
  // Pending DMs are retried by the engine outbox, so re-read while any are still in flight.
  clearTimeout(dmTimer);
  dmTimer = null;
  if (items.some((m) => m.pending)) {
    dmTimer = setTimeout(() => loadDm().catch(() => {}), 5000);
  }
}

// ---- navigation ----

export const openHome = guard(async () => {
  state.route = { kind: 'home' };
  state.community = null;
  state.channelId = null;
  state.messages = [];
  notify('rail', 'sidebar', 'main', 'right');
  await loadConversations();
});

export const openServer = guard(async (communityId) => {
  state.route = { kind: 'server', communityId };
  state.community = null;
  state.channelId = null;
  state.messages = [];
  notify('rail', 'sidebar', 'main', 'right');
  await loadCommunity(communityId);
});

export const openChannel = guard(async (channelId) => {
  state.channelId = channelId;
  state.messages = [];
  notify('sidebar', 'main');
  await loadMessages();
});

export const openDm = guard(async (peer, displayName = '') => {
  state.route = { kind: 'home' };
  state.community = null;
  state.channelId = null;
  state.messages = [];
  state.peer = peer;
  state.peerName = displayName;
  state.dmMessages = [];
  state.peerProfile = null;
  notify('rail', 'sidebar', 'main', 'right');
  await Promise.all([loadDm(), loadPeerProfile(peer)]);
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
  return true;
});

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

// ---- direct messages ----

export const sendDm = guard(async (text) => {
  const peer = state.peer;
  if (!peer) return false;
  await api.sendDm(peer, text);
  await Promise.all([loadDm(), loadConversations()]);
  return true;
});

export const addContact = guard(async (link) => {
  const user = await api.addContact(link);
  await loadConversations();
  await openDm(user.root_id, user.display_name);
  return true;
});

export const contactLink = guard(() => api.contactLink());

// ---- profile and settings ----

export const userProfile = guard((user) => api.userProfile(user));

export const setDisplayName = guard(async (name) => {
  await api.setDisplayName(name);
  state.me = await api.me();
  notify('sidebar');
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
      return loadCommunities();
    case 'community_updated':
      if (state.route.kind === 'server' && state.route.communityId === ev.community_id) {
        return loadCommunity(ev.community_id);
      }
      return undefined;
    case 'message':
      if (state.community?.id === ev.community_id && state.channelId === ev.channel_id) {
        return loadMessages();
      }
      return undefined;
    case 'dm':
      await loadConversations();
      if (state.peer === ev.peer) await loadDm();
      return undefined;
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
