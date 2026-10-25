// Far-left column: Home (direct messages) on top, one round button per server, "+" at the end of the list.
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';
import { el, initials, setHue, icon, withImage } from '../ui.js';
import { openServerDialog } from './dialogs.js';

export function initRail() {
  subscribe('rail', render);
  render();
}

function render() {
  const onHome = state.route.kind === 'home';
  const dmUnread = [...state.unread].filter((k) => k.startsWith('dm:') || k.startsWith('group:')).length;
  const servers = state.communities.map((c) => {
    const active = state.route.kind === 'server' && state.route.communityId === c.id;
    const btn = el('button', {
      class: 'rail-btn',
      role: 'tab',
      'aria-selected': active ? 'true' : 'false',
      'aria-label': c.name,
      'aria-current': active ? 'page' : null,
      'data-tip': c.name,
      onclick: () => actions.openServer(c.id),
    }, initials(c.name));
    setHue(btn, c.hue ?? c.id);
    withImage(btn, c.id);
    const unreadKeys = [...state.unread].filter((k) => k.startsWith(`ch:${c.id}:`));
    const badge = typeof c.mentions === 'number' && c.mentions > 0
      ? el('span', { class: 'rail-badge', text: String(c.mentions), 'aria-label': `${c.mentions} mentions` })
      : null;
    return el('div', { class: `rail-item${active ? ' active' : ''}${unreadKeys.length ? ' unread' : ''}` }, btn, badge);
  });
  const rail = document.getElementById('rail');
  rail.setAttribute('role', 'tablist');
  rail.setAttribute('aria-orientation', 'vertical');
  rail.setAttribute('aria-label', 'Servers and direct messages');
  rail.replaceChildren(
    el('div', { class: `rail-item${onHome ? ' active' : ''}` },
      el('button', {
        class: 'rail-btn home',
        role: 'tab',
        'aria-selected': onHome ? 'true' : 'false',
        'aria-label': 'Direct messages',
        'aria-current': onHome ? 'page' : null,
        'data-tip': 'Direct Messages',
        onclick: () => actions.openHome(),
      }, icon('dissent-mark')),
      dmUnread > 0 ? el('span', { class: 'rail-badge', text: String(dmUnread), 'aria-label': `${dmUnread} unread conversations` }) : null),
    el('hr', { class: 'rail-sep' }),
    ...servers,
    el('div', { class: 'rail-item' },
      el('button', {
        class: 'rail-btn add',
        'aria-label': 'Create or join a server',
        'data-tip': 'Add a Server',
        icon: 'plus',
        onclick: () => openServerDialog(),
      })),
  );
}
