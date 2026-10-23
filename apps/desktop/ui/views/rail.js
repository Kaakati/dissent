// Far-left column: Home (direct messages) on top, one round button per server, "+" at the bottom.
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';
import { el, avClass, initials } from '../ui.js';
import { openServerDialog } from './dialogs.js';

export function initRail() {
  subscribe('rail', render);
  render();
}

function render() {
  const onHome = state.route.kind === 'home';
  const servers = state.communities.map((c) => {
    const active = state.route.kind === 'server' && state.route.communityId === c.id;
    return el('button', {
      class: `rail-btn ${avClass(c.id)}${active ? ' active' : ''}`,
      title: c.name,
      'aria-label': c.name,
      onclick: () => actions.openServer(c.id),
    }, initials(c.name));
  });
  document.getElementById('rail').replaceChildren(
    el('button', {
      class: `rail-btn home${onHome ? ' active' : ''}`,
      title: 'Home (direct messages)',
      'aria-label': 'Home (direct messages)',
      icon: 'house',
      onclick: () => actions.openHome(),
    }),
    el('hr', { class: 'rail-sep' }),
    ...servers,
    el('button', {
      class: 'rail-btn add',
      title: 'Create or join a server',
      'aria-label': 'Create or join a server',
      icon: 'plus',
      onclick: () => openServerDialog(),
    }),
  );
}
