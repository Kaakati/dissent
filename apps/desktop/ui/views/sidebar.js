// Second column. Server: header with its menu, channel list. Home: find box, direct messages.
// Both end with the user panel: my avatar, name, `you·suffix`, and the settings gear.
import { state, subscribe, canManage } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, skeletonRows } from '../ui.js';
import { openSwitcher } from './switcher.js';
import {
  confirmDialog,
  openAddContactDialog,
  openClaimHandleDialog,
  openCreateChannelDialog,
  openInviteDialog,
} from './dialogs.js';
import { openSettingsPage } from './settings.js';

let menu = null;

export function initSidebar() {
  subscribe('sidebar', render);
  render();
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
  column.replaceChildren(top, el('div', { class: 'sidebar-scroll' }, body), userPanel());
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

// ---- server: channels ----

function serverList(community) {
  const manage = canManage();
  const channels = community.channels.map((ch) => {
    const active = ch.id === state.channelId;
    const unread = state.unread.has(`ch:${community.id}:${ch.id}`) && !active;
    return el('button', {
      class: `row-item${active ? ' active' : ''}${unread ? ' unread' : ''}`,
      'aria-current': active ? 'true' : null,
      icon: 'hash',
      onclick: () => actions.openChannel(ch.id),
    }, unread ? el('span', { class: 'unread-dot', 'aria-label': 'unread' }) : null, el('span', { class: 'name', text: ch.name, title: `#${ch.name}` }));
  });
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
    ...(channels.length ? channels : [el('p', { class: 'sidebar-empty', text: 'No channels yet.' })]),
  ];
}

// ---- home: direct messages ----

function homeList() {
  const convos = state.conversations.map((conv) => {
    const active = state.route.kind === 'home' && state.peer === conv.peer;
    const unread = state.unread.has(`dm:${conv.peer}`) && !active;
    return el('button', {
      class: `row-item convo${active ? ' active' : ''}${unread ? ' unread' : ''}`,
      'aria-current': active ? 'true' : null,
      title: conv.display_name,
      onclick: () => actions.openDm(conv.peer, conv.display_name),
    },
    unread ? el('span', { class: 'unread-dot', 'aria-label': 'unread' }) : null,
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
    ...(convos.length ? convos : [el('p', { class: 'sidebar-empty', text: 'No conversations yet. Add a contact to start one.' })]),
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
      el('div', { class: 'me-name', text: me?.display_name || 'Set a display name', title: me?.display_name || '' }),
      el('div', { class: 'me-suffix', text: `you·${me?.suffix ?? ''}` })),
    el('button', { class: 'icon-btn', title: 'Settings', 'aria-label': 'Settings', icon: 'settings', onclick: () => openSettingsPage() }),
  );
}
