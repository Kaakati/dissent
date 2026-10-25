// Second column. Server: header with its menu, quick links, text and voice channels. Home: find box, quick links,
// direct messages. The user panel (#me) floats over the rail and the sidebar and is rendered here too.
import { state, subscribe, canManage, textChannelsOf, voiceChannelsOf } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, stackAvatar, skeletonRows, icon, copyText, toast, plural, presenceDot, presenceOf } from '../ui.js';
import { has } from '../features.js';
import { openSwitcher } from './switcher.js';
import {
  confirmDialog,
  openAddContactDialog,
  openClaimHandleDialog,
  openContactDialog,
  openCreateChannelDialog,
  openInviteDialog,
  openNewGroupDialog,
  openSafetyDialog,
  openNoteDialog,
} from './dialogs.js';
import { openSettingsPage } from './settings.js';
import { openChannelSettings } from './channel-settings.js';
import { openProfile } from './profile.js';
import { openContextMenu } from './contextmenu.js';
import { openServerIconDialog } from './avatar.js';
import { openStatusMenu, selfPresence, statusLabel } from './statusmenu.js';

let menu = null;

// The second line of a direct row, for the presence states that have one worth saying.
const PRESENCE_TEXT = { idle: 'Idle', dnd: 'Do Not Disturb' };

export function initSidebar() {
  subscribe('sidebar', render);
  subscribe('me', renderMe);
  subscribe('status', renderMe);
  subscribe('sidebar', renderMe);
  render();
  renderMe();
}

function render() {
  closeMenu();
  const column = document.getElementById('sidebar');
  const serverRoute = state.route.kind === 'server';
  const serverOpen = serverRoute && state.community;
  let top;
  let body;
  if (!serverRoute) {
    top = homeTop();
    body = homeList();
  } else if (serverOpen) {
    top = serverTop(state.community);
    body = serverList(state.community);
  } else {
    top = el('div', { class: 'sidebar-top' }, el('span', { class: 'skel skel-line tall' }));
    body = skeletonRows('channel', 5);
  }
  column.replaceChildren(top, el('div', { class: 'sidebar-scroll' }, body, el('div', { class: 'sidebar-spacer' })));
}

// ---- top: find box (Home) or server header (server) ----

function homeTop() {
  return el('div', { class: 'sidebar-top' },
    el('button', {
      class: 'find-btn',
      'aria-label': 'Find or start a conversation',
      'aria-keyshortcuts': 'Control+K Meta+K',
      onclick: () => openSwitcher(),
      text: 'Find or start a conversation',
    }));
}

function serverTop(community) {
  const menuBtn = el('button', {
    class: 'server-menu-btn',
    'aria-haspopup': 'menu',
    'aria-label': `${community.name} menu`,
    onclick: (e) => toggleMenu(e.currentTarget.closest('.sidebar-top'), community),
  }, el('span', { class: 'server-name', text: community.name, title: community.name }), el('span', { class: 'caret', icon: 'chevron-down' }));
  return el('div', { class: 'sidebar-top' }, menuBtn);
}

// ---- quick links above the lists ----

function quickLink(iconName, label, onclick, extra = null, active = false) {
  return el('button', { class: `row-item link${active ? ' active' : ''}`, 'aria-current': active ? 'page' : null, icon: iconName, onclick },
    el('span', { class: 'name', text: label }), extra);
}

function transfersLink() {
  if (!has('transfers')) return null;
  const active = state.transfers.filter((t) => ['active', 'paused', 'waiting'].includes(t.state)).length;
  return quickLink('arrow-up-down', 'Transfers', () => actions.openPage('transfers'),
    active ? el('span', { class: 'pill accent', text: `${active} active` }) : null,
    state.ui.page === 'transfers');
}

// ---- server: channels ----

function serverList(community) {
  const manage = canManage();
  const channels = textChannelsOf(community).map((ch) => {
    const active = ch.id === state.channelId && !state.voiceChannelId && !state.ui.page;
    const unread = state.unread.has(`ch:${community.id}:${ch.id}`) && !active;
    return withChannelActions(community, el('button', {
      class: `row-item${active ? ' active' : ''}${unread ? ' unread' : ''}`,
      'aria-current': active ? 'true' : null,
      icon: 'hash',
      onclick: () => actions.openChannel(ch.id),
    }, unread ? el('span', { class: 'unread-dot', 'aria-label': 'unread' }) : null, el('span', { class: 'name', text: ch.name, title: `#${ch.name}` })), ch);
  });
  const addChannel = manage
    ? el('button', { class: 'icon-btn', 'aria-label': 'Create channel', title: 'Create channel', icon: 'plus', onclick: () => openCreateChannelDialog(community.id) })
    : null;
  const voice = has('calls') ? voiceSection(community) : [];
  return [
    quickLink('user-plus', 'Invite people', () => openInviteDialog(community.id)),
    transfersLink(),
    el('div', { class: 'sidebar-sep', role: 'separator' }),
    el('div', { class: 'section-head' }, el('span', { text: 'Text channels' }), addChannel),
    ...(channels.length ? channels : [el('p', { class: 'sidebar-empty', text: 'No channels yet.' })]),
    ...voice,
  ];
}

// A channel row with the owner's hover actions: Create invite and Edit Channel. They show on hover and focus.
function withChannelActions(community, row, ch) {
  if (!canManage()) return row;
  return el('div', { class: 'channel-row' }, row,
    el('span', { class: 'row-actions' },
      el('button', { class: 'row-action', icon: 'user-plus', 'aria-label': 'Create invite', title: 'Create invite', onclick: () => openInviteDialog(community.id) }),
      el('button', { class: 'row-action', icon: 'settings', 'aria-label': 'Edit Channel', title: 'Edit Channel', onclick: () => openChannelSettings(community.id, ch.id) })));
}

// Voice channels (calls feature): a row per channel that joins it on click, and the people in it under the row.
function voiceSection(community) {
  const addVoice = canManage()
    ? el('button', { class: 'icon-btn', 'aria-label': 'Create voice channel', title: 'Create voice channel', icon: 'plus', onclick: () => openCreateChannelDialog(community.id, 'voice') })
    : null;
  const rows = [];
  for (const ch of voiceChannelsOf(community)) {
    const active = state.voiceChannelId === ch.id;
    const connected = state.voiceConnection?.channelId === ch.id;
    rows.push(withChannelActions(community, el('button', {
      class: `row-item voice${active ? ' active' : ''}`,
      'aria-current': active ? 'true' : null,
      icon: 'volume-2',
      title: connected ? `Connected to ${ch.name}` : `Join ${ch.name}`,
      onclick: () => actions.openVoiceChannel(ch.id),
    }, el('span', { class: 'name', text: ch.name, title: ch.name })), ch));
    if (ch.participants.length) {
      rows.push(el('div', { role: 'list', 'aria-label': `In ${ch.name}` }, ch.participants.map(voiceMember)));
    }
  }
  return [el('div', { class: 'section-head spaced' }, el('span', { text: 'Voice channels' }), addVoice), ...rows];
}

// One person in a voice channel: avatar with a speaking ring, name, and mute and deafen flags.
function voiceMember(p) {
  const a = avatar(p.display_name, p.root_id);
  if (p.speaking) a.classList.add('speaking');
  return el('div', { class: 'voice-member', role: 'listitem' },
    a,
    el('span', { class: 'name', text: p.display_name, title: p.display_name }),
    p.muted ? el('span', { class: 'voice-flag', title: 'Muted', icon: 'mic-off' }, el('span', { class: 'sr-only', text: 'Muted' })) : null,
    p.deafened ? el('span', { class: 'voice-flag danger', title: 'Deafened', icon: 'headphones' }, el('span', { class: 'sr-only', text: 'Deafened' })) : null);
}

// ---- home: direct messages ----

function homeList() {
  const incoming = state.contacts?.incoming?.length ?? 0;
  const links = [
    has('contacts')
      ? quickLink('users', 'Friends', () => actions.openPage('contacts'),
        incoming ? el('span', { class: 'count', text: String(incoming), 'aria-label': `${incoming} contact requests` }) : null,
        state.ui.page === 'contacts')
      : null,
    quickLink('link', 'Invite links', () => openContactDialog()),
    transfersLink(),
    el('div', { class: 'sidebar-sep', role: 'separator' }),
  ];
  const convos = state.conversations.map(conversationRow);
  const add = has('groups')
    ? el('button', {
      class: 'icon-btn',
      'aria-label': 'New conversation',
      'aria-haspopup': 'menu',
      title: 'New conversation',
      icon: 'plus',
      onclick: (e) => openContextMenu(e.clientX || e.currentTarget.getBoundingClientRect().left, e.clientY || e.currentTarget.getBoundingClientRect().bottom, [
        { label: 'Add contact', icon: 'user-plus', onPick: () => openAddContactDialog() },
        { label: 'Create a group', icon: 'users-round', onPick: () => openNewGroupDialog() },
      ]),
    })
    : el('button', { class: 'icon-btn', 'aria-label': 'Add contact', title: 'Add contact', icon: 'plus', onclick: () => openAddContactDialog() });
  return [
    ...links,
    el('div', { class: 'section-head' }, el('span', { text: 'Direct messages' }), add),
    ...(convos.length ? convos : [el('p', { class: 'sidebar-empty', text: 'No conversations yet. Add a contact to start one.' })]),
  ];
}

function conversationRow(conv) {
  const group = conv.kind === 'group';
  const key = actions.convKey(conv);
  const active = state.route.kind === 'home' && !state.ui.page && state.peer === (group ? conv.id : conv.peer);
  const unread = state.unread.has(key) && !active;
  const name = group ? conv.name : conv.display_name;
  // A live call on the row replaces the second line: "Call · 3 in", or "In a call with you" for a direct call.
  // Otherwise a direct row shows its status (Idle, Do Not Disturb) or its last message, as before.
  const live = conv.live_call ?? null;
  const sub = live
    ? (live.scope === 'dm' ? 'In a call with you' : `Call · ${live.participants?.length ?? 0} in`)
    : (group ? plural(conv.members?.length ?? 0, 'member') : (PRESENCE_TEXT[presenceOf(conv)] ?? conv.last_text ?? ''));
  const row = el('button', {
    class: `row-item convo${active ? ' active' : ''}${unread ? ' unread' : ''}`,
    'aria-current': active ? 'true' : null,
    title: name,
    onclick: () => actions.openConversation(conv),
    oncontextmenu: (e) => { e.preventDefault(); openConversationMenu(e.clientX, e.clientY, conv); },
  },
  unread ? el('span', { class: 'unread-dot', 'aria-label': 'unread' }) : null,
  group ? groupAvatar(conv) : avatar(name, conv.peer, '', presenceOf(conv)),
  el('span', { class: 'convo-text' },
    el('span', { class: 'name', text: name }),
    sub ? el('span', { class: `convo-last${live ? ' call-live' : ''}`, text: sub }) : null),
  conv.pinned ? el('span', { class: 'icon pinned', title: 'Pinned' }, icon('pin')) : null,
  actions.isMuted(key) ? el('span', { class: 'icon muted-mark', title: 'Muted', 'aria-label': 'Muted' }, icon('bell-off')) : null);
  return row;
}

function groupAvatar(conv) {
  const node = stackAvatar(conv.members ?? []);
  const presence = presenceOf(conv);
  if (presence) node.append(presenceDot(presence));
  return node;
}

// The right-click menu on a conversation. Items the engine does not support are left out, not disabled.
export function openConversationMenu(x, y, conv) {
  const group = conv.kind === 'group';
  const key = actions.convKey(conv);
  const name = group ? conv.name : conv.display_name;
  const items = [
    { label: 'Mark as read', disabled: !state.unread.has(key), onPick: () => actions.markRead(key) },
    'sep',
    has('pins') ? { label: conv.pinned ? 'Unpin' : 'Pin', onPick: () => actions.pinConversation(key, !conv.pinned) } : null,
    has('pins') ? { label: conv.pinned ? 'Remove from favorites' : 'Add to favorites', onPick: () => actions.pinConversation(key, !conv.pinned) } : null,
    has('pins') ? 'sep' : null,
  ];
  if (!group) {
    items.push(
      { label: 'Profile', onPick: () => openProfile(conv.peer, x, y) },
      has('calls') ? { label: 'Start a call', onPick: () => actions.startCall(conv.peer) } : null,
      { label: 'Compare safety number', onPick: () => openSafetyDialog(conv.peer, name) },
      has('notes') ? { label: 'Add note', sub: 'Only visible to you', onPick: () => openNoteDialog(conv.peer, name, 'note') } : null,
      has('notes') ? { label: 'Set nickname', onPick: () => openNoteDialog(conv.peer, name, 'nickname') } : null,
      has('contacts') ? {
        label: 'Copy contact link', onPick: async () => {
          const link = await actions.peerContactLink(conv.peer);
          if (link && (await copyText(link))) toast(`${name}'s contact link copied`, 'info');
        },
      } : null,
      group ? null : (has('close_dm') ? { label: 'Close DM', onPick: () => actions.closeDm(key) } : null),
      'sep',
      state.communities.length ? {
        label: 'Invite to server',
        submenu: state.communities.map((c) => ({
          label: c.name, mini: c, onPick: async () => {
            const link = await actions.createInvite(c.id);
            if (!link) return;
            await actions.openDm(conv.peer, name);
            if (await actions.sendDm(`Join ${c.name}: ${link}`)) toast(`Invite to ${c.name} sent to ${name}`, 'info');
          },
        })),
      } : null,
      has('contacts') ? {
        label: 'Remove contact', onPick: async () => {
          if (await confirmDialog(`Remove ${name} from your contacts? Your conversation stays on this device.`, 'Remove')) actions.removeContact(conv.peer);
        },
      } : null,
      !group && has('ignore') ? (state.ignored.has(conv.peer)
        ? { label: 'Unignore', onPick: () => actions.setIgnored(conv.peer, false) }
        : { label: 'Ignore', sub: 'Hide their messages without telling them', onPick: () => actions.setIgnored(conv.peer, true) }) : null,
      has('contacts') ? {
        label: 'Block', danger: true, onPick: async () => {
          if (await confirmDialog(`Block ${name}? They won't be told, and their messages stop reaching you.`, 'Block')) actions.blockUser(conv.peer);
        },
      } : null,
    );
  } else {
    items.push(
      has('groups') ? {
        label: 'Copy group invite link', onPick: async () => {
          const link = await actions.groupInviteLink(conv.id);
          if (link && (await copyText(link))) toast('Group invite link copied', 'info');
        },
      } : null,
      has('groups') ? {
        label: 'Leave group', danger: true, onPick: async () => {
          if (await confirmDialog(`Leave ${name}? You'll stop receiving its messages.`, 'Leave')) actions.leaveGroup(conv.id);
        },
      } : null,
    );
  }
  if (has('mute')) {
    items.push('sep', actions.isMuted(key) ? {
      label: `Unmute ${group ? name : `@${name}`}`,
      onPick: () => actions.unmuteConversation(key),
    } : {
      label: `Mute ${group ? name : `@${name}`}`,
      submenu: [15, 60, 480, 0].map((mins) => ({
        label: mins === 0 ? 'Until I turn it back on' : mins >= 60 ? plural(mins / 60, 'hour') : `${mins} minutes`,
        onPick: () => actions.muteConversation(key, mins === 0 ? null : Date.now() + mins * 60_000),
      })),
    });
  }
  openContextMenu(x, y, items);
}

// ---- server menu (existing actions) ----

function toggleMenu(anchor, community) {
  if (menu) {
    closeMenu();
    return;
  }
  const item = (label, iconName, onPick, cls = '') => el('button', {
    class: `menu-item ${cls}`.trim(),
    role: 'menuitem',
    icon: iconName,
    text: label,
    onclick: () => {
      closeMenu();
      onPick();
    },
  });
  const manage = canManage();
  menu = el('div', { class: 'menu', role: 'menu' },
    item('Create invite', 'user-plus', () => openInviteDialog(community.id)),
    item('Claim handle', 'at-sign', () => openClaimHandleDialog(community.id)),
    manage ? item('Create channel', 'plus', () => openCreateChannelDialog(community.id)) : null,
    ['owner', 'admin'].includes(community.my_role) ? item('Server icon', 'image', () => openServerIconDialog(community)) : null,
    item('Leave server', 'log-out', async () => {
      if (await confirmDialog(`Leave ${community.name}? You can rejoin with an invite link.`, 'Leave')) {
        actions.leaveCommunity(community.id);
      }
    }, 'danger'),
  );
  anchor.append(menu);
  document.addEventListener('pointerdown', onMenuOutside, true);
}

function onMenuOutside(e) {
  if (menu && !menu.contains(e.target) && !e.target.closest?.('.server-menu-btn')) closeMenu();
}

function closeMenu() {
  menu?.remove();
  menu = null;
  document.removeEventListener('pointerdown', onMenuOutside, true);
}

// ---- user panel: floats over the rail and the sidebar ----

function renderMe() {
  const host = document.getElementById('me');
  const me = state.me;
  const call = state.call;
  const name = me?.display_name || 'Set a display name';
  const status = call ? 'In a call' : statusLabel(state.status.status);
  // The name and avatar open the profile popover with the status picker; without the status feature they open Settings.
  const self = el('button', {
    class: 'me-self',
    'aria-label': `${name}, ${status}. Profile and status`,
    'aria-haspopup': has('status') ? 'menu' : null,
    title: me ? `${me.display_name}·${me.suffix}` : '',
    onclick: (e) => (has('status') ? openStatusMenu(e.currentTarget) : openSettingsPage('identity')),
  },
  avatar(me?.display_name || '?', me?.root_id, '', selfPresence()),
  el('span', { class: 'me-text' },
    el('span', { class: 'me-name', text: name }),
    el('span', { class: 'me-status', text: status })));
  const tools = [];
  if (has('calls')) {
    const muted = Boolean(call?.muted);
    const deaf = Boolean(call?.deafened);
    tools.push(
      el('button', { class: `icon-btn${muted ? ' on' : ''}`, 'aria-label': muted ? 'Unmute' : 'Mute', 'aria-pressed': muted ? 'true' : 'false', title: muted ? 'Unmute' : 'Mute', icon: muted ? 'mic-off' : 'mic', disabled: !call, onclick: () => actions.toggleMuted() }),
      el('button', { class: 'icon-btn narrow', 'aria-label': 'Microphone options', title: 'Microphone options', icon: 'chevron-down', disabled: true }),
      el('button', { class: `icon-btn${deaf ? ' on' : ''}`, 'aria-label': deaf ? 'Undeafen' : 'Deafen', 'aria-pressed': deaf ? 'true' : 'false', title: deaf ? 'Undeafen' : 'Deafen', icon: 'headphones', disabled: !call, onclick: () => actions.toggleDeafened() }),
      el('button', { class: 'icon-btn narrow', 'aria-label': 'Output options', title: 'Output options', icon: 'chevron-down', disabled: true }),
    );
  }
  tools.push(el('button', { class: 'icon-btn', title: 'Settings', 'aria-label': 'Settings', 'aria-keyshortcuts': 'Control+, Meta+,', icon: 'settings', onclick: () => openSettingsPage() }));
  const connected = state.voiceConnection && state.community?.id === state.voiceConnection.communityId
    ? state.community.channels.find((c) => c.id === state.voiceConnection.channelId)
    : null;
  const voice = call
    ? el('div', { class: 'me-voice' },
      el('div', { class: 'text' },
        el('div', { class: 'state', text: 'Voice connected' }),
        el('div', { class: 'where', text: connected ? `#${connected.name} · ${state.community.name}` : call.where ?? [call.channel_name, call.community_name].filter(Boolean).join(' / ') })),
      el('button', { class: 'icon-btn', 'aria-label': 'Disconnect', title: 'Disconnect', icon: 'phone-off', onclick: () => actions.leaveCall() }))
    : null;
  // A re-render drops focus with the old button, so the user bar keeps it when it had it.
  const hadFocus = host.contains(document.activeElement);
  host.replaceChildren(...[voice, el('div', { class: 'me-row' }, self, ...tools)].filter(Boolean));
  if (hadFocus) host.querySelector('.me-self')?.focus();
}
