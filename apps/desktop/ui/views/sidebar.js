// Second column. Server: header with its menu, channel list. Home: find box, direct messages.
// Both end with the user panel: my avatar, name, `you·suffix`, and the settings gear.
import { state, subscribe, canManage } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar } from '../ui.js';
import {
  confirmDialog,
  openAddContactDialog,
  openClaimHandleDialog,
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
  const serverOpen = state.route.kind === 'server' && state.community;
  column.replaceChildren(
    serverOpen ? serverTop(state.community) : homeTop(),
    el('div', { class: 'sidebar-scroll' }, serverOpen ? serverList(state.community) : homeList()),
    userPanel(),
  );
}

// ---- top: find box (Home) or server header (server) ----

function homeTop() {
  return el('div', { class: 'sidebar-top' },
    el('button', {
      class: 'find-btn',
      'aria-label': 'Find or start a conversation',
      onclick: () => openAddContactDialog(),
      text: 'Find or start a conversation',
    }));
}

function serverTop(community) {
  const menuBtn = el('button', {
    class: 'server-menu-btn',
    'aria-haspopup': 'menu',
    'aria-label': `${community.name} menu`,
    onclick: (e) => toggleMenu(e.currentTarget.closest('.sidebar-top'), community),
  }, el('span', { class: 'server-name', text: community.name }), el('span', { class: 'caret', icon: 'chevron-down' }));
  return el('div', { class: 'sidebar-top' }, menuBtn);
}

// ---- server: channels ----

function serverList(community) {
  const manage = canManage();
  const channels = community.channels.map((ch) => el('button', {
    class: `row-item${ch.id === state.channelId ? ' active' : ''}`,
    'aria-current': ch.id === state.channelId ? 'true' : null,
    icon: 'hash',
    onclick: () => actions.openChannel(ch.id),
  }, el('span', { class: 'name', text: ch.name })));
  return [
    el('div', { class: 'section-head' },
      el('span', { text: 'Channels' }),
      manage
        ? el('button', {
          class: 'icon-btn',
          'aria-label': 'Server actions',
          title: 'Server actions',
          icon: 'settings',
          onclick: (e) => toggleMenu(e.currentTarget.closest('.sidebar').querySelector('.sidebar-top'), community),
        })
        : null),
    ...(channels.length ? channels : [el('p', { class: 'muted', text: 'No channels.' })]),
  ];
}

// ---- home: direct messages ----

function homeList() {
  const convos = state.conversations.map((conv) => {
    const active = state.route.kind === 'home' && state.peer === conv.peer;
    return el('button', {
      class: `row-item convo${active ? ' active' : ''}`,
      'aria-current': active ? 'true' : null,
      onclick: () => actions.openDm(conv.peer, conv.display_name),
    },
    avatar(conv.display_name, conv.peer),
    el('span', { class: 'convo-text' },
      el('span', { class: 'name', text: conv.display_name }),
      el('span', { class: 'convo-last', text: conv.last_text ?? '' })));
  });
  return [
    el('div', { class: 'section-head' },
      el('span', { text: 'Direct Messages' }),
      el('button', {
        class: 'icon-btn',
        'aria-label': 'Add contact',
        title: 'Add contact',
        icon: 'plus',
        onclick: () => openAddContactDialog(),
      })),
    ...(convos.length ? convos : [el('p', { class: 'muted', text: 'No conversations yet.' })]),
  ];
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
  if (menu && !menu.contains(e.target) && !e.target.closest?.('.server-menu-btn, [aria-label="Server actions"]')) closeMenu();
}

function closeMenu() {
  menu?.remove();
  menu = null;
  document.removeEventListener('pointerdown', onMenuOutside, true);
}

// ---- user panel ----

function userPanel() {
  const me = state.me;
  return el('div', { class: 'me' },
    avatar(me?.display_name || '?', me?.root_id),
    el('div', { class: 'me-text' },
      el('div', { class: 'me-name', text: me?.display_name || 'Set a display name' }),
      el('div', { class: 'me-suffix', text: `you·${me?.suffix ?? ''}` })),
    el('button', { class: 'icon-btn', title: 'Settings', 'aria-label': 'Settings', icon: 'settings', onclick: () => openSettings() }),
  );
}
