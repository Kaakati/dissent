// Everything that talks to the backend and then updates state. Views call these; they never call api directly.
import { api } from './api.js';
import { state, notify } from './state.js';
import { toast } from './ui.js';
import { seenMs, markSeen } from './unread.js';

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
  await refreshUnread();
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
  state.historyStart = messages.length < MSG_LIMIT;
  notify('main');
}

let dmTimer = null;

async function loadDm() {
  const peer = state.peer;
  if (!peer) return;
  const items = await api.dmMessages(peer, MSG_LIMIT);
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
  // The first channel is picked automatically; open it the same way a click would, so unread markers apply.
  if (state.route.kind === 'server' && state.channelId) await openChannel(state.channelId);
});

export const openChannel = guard(async (channelId) => {
  const key = `ch:${state.community?.id}:${channelId}`;
  const seenBefore = seenMs(key);
  state.channelId = channelId;
  state.messages = [];
  state.newFromId = null;
  state.loading.messages = true;
  notify('sidebar', 'main');
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

// ---- replies and reactions (only offered when the backend reports them, see views/chat.js) ----

export const sendReply = guard(async (messageId, text) => {
  const community = state.community;
  if (!community || !state.channelId) return false;
  await api.sendReply(community.id, state.channelId, messageId, text);
  await loadMessages();
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
export const safetyNumber = guard((user) => api.safetyNumber(user));

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
    const key = `dm:${conv.peer}`;
    if (conv.peer === openPeer) continue;
    const seen = seenMs(key);
    if (seen === null) markSeen(key, conv.peer, Number(conv.last_ms));
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
