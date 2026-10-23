// Second column. For a server: name header with menu, channel list. For Home: conversations and contact actions.
// Both end with the footer showing my name and the settings gear.
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar } from '../ui.js';
import {
  confirmDialog,
  openAddContactDialog,
  openClaimHandleDialog,
  openContactDialog,
  openCreateChannelDialog,
  openInviteDialog,
  openSettings,
} from './dialogs.js';

let menu = null;

export function initSidebar() {
  subscribe('sidebar', render);
  render();
}

function render() {
  closeMenu();
  const column = document.getElementById('sidebar');
  const body = state.route.kind === 'server' && state.community ? serverBody(state.community) : homeBody();
  column.replaceChildren(body, footer());
}

// ---- server ----

function serverBody(community) {
  const canManage = ['owner', 'admin'].includes(community.my_role);
  const menuBtn = el('button', {
    class: 'server-menu-btn',
    'aria-haspopup': 'menu',
    onclick: (e) => toggleMenu(e.currentTarget, community, canManage),
  }, el('span', { class: 'server-name', text: community.name }), el('span', { class: 'caret', icon: 'chevron-down' }));
  const channels = community.channels.map((ch) => el('button', {
    class: `channel${ch.id === state.channelId ? ' active' : ''}`,
    icon: 'hash',
    text: ch.name,
    onclick: () => actions.openChannel(ch.id),
  }));
  return el('div', { class: 'sidebar-body' },
    el('header', { class: 'sidebar-head server-header' }, menuBtn),
    el('div', { class: 'section-label', text: 'Channels' }),
    el('div', { class: 'list' }, channels.length ? channels : el('p', { class: 'muted', text: 'No channels.' })),
  );
}

function toggleMenu(anchor, community, canManage) {
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
  menu = el('div', { class: 'menu', role: 'menu' },
    item('Create invite', 'user-plus', () => openInviteDialog(community.id)),
    item('Claim handle', 'at-sign', () => openClaimHandleDialog(community.id)),
    canManage ? item('Create channel', 'plus', () => openCreateChannelDialog(community.id)) : null,
    item('Leave server', 'log-out', async () => {
      if (await confirmDialog(`Leave ${community.name}? You can rejoin with an invite link.`, 'Leave')) {
        actions.leaveCommunity(community.id);
      }
    }, 'danger'),
  );
  anchor.parentElement.append(menu);
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

// ---- home ----

function homeBody() {
  const convos = state.conversations.map((conv) => {
    const active = state.route.kind === 'home' && state.peer === conv.peer;
    return el('button', {
      class: `convo${active ? ' active' : ''}`,
      onclick: () => actions.openDm(conv.peer, conv.display_name),
    },
      avatar(conv.display_name, conv.peer),
      el('span', { class: 'convo-text' },
        el('span', { class: 'convo-name', text: conv.display_name }),
        el('span', { class: 'convo-last', text: conv.last_text ?? '' })));
  });
  return el('div', { class: 'sidebar-body' },
    el('header', { class: 'sidebar-head' }, el('h2', { text: 'Direct messages' })),
    el('div', { class: 'row-btns' },
      el('button', { class: 'btn-small', icon: 'user-plus', onclick: () => openAddContactDialog() }, 'Add contact'),
      el('button', { class: 'btn-small', icon: 'copy', onclick: () => openContactDialog() }, 'My contact link')),
    el('div', { class: 'list' }, convos.length ? convos : el('p', { class: 'muted', text: 'No conversations yet.' })),
  );
}

// ---- footer: me + settings ----

function footer() {
  const me = state.me;
  return el('div', { class: 'me' },
    avatar(me?.display_name || '?', me?.root_id),
    el('div', { class: 'me-text' },
      el('div', { class: 'me-name', text: me?.display_name || 'Set a display name' }),
      el('div', { class: 'me-suffix', text: `you·${me?.suffix ?? ''}` })),
    el('button', { class: 'icon-btn', title: 'Settings', 'aria-label': 'Settings', icon: 'settings', onclick: () => openSettings() }),
  );
}
