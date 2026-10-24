// Far-left column: Home (direct messages) on top, one round-square button per server, "+" at the bottom.
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';
import { el, initials, setHue } from '../ui.js';
import { openServerDialog } from './dialogs.js';

export function initRail() {
  subscribe('rail', render);
  render();
}

function render() {
  const onHome = state.route.kind === 'home';
  const servers = state.communities.map((c) => {
    const active = state.route.kind === 'server' && state.route.communityId === c.id;
    const btn = el('button', {
      class: 'rail-btn',
      'aria-label': c.name,
      'aria-current': active ? 'page' : null,
      'data-tip': c.name,
      onclick: () => actions.openServer(c.id),
    }, initials(c.name));
    setHue(btn, c.id);
    return el('div', { class: `rail-item${active ? ' active' : ''}` }, btn);
  });
  document.getElementById('rail').replaceChildren(
    el('div', { class: `rail-item${onHome ? ' active' : ''}` },
      el('button', {
        class: 'rail-btn home',
        'aria-label': 'Direct messages',
        'aria-current': onHome ? 'page' : null,
        'data-tip': 'Direct Messages',
        icon: 'dissent-mark',
        onclick: () => actions.openHome(),
      })),
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
