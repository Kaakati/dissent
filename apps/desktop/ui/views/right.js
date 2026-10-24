// Right column. Shows the server member list, or the DM profile panel, or nothing.
import { state, subscribe } from '../state.js';
import { membersNodes } from './members.js';
import { profilePanelNodes } from './dmpanel.js';
import { skeletonRows } from '../ui.js';

export function initRight() {
  subscribe('right', render);
  render();
}

function render() {
  const host = document.getElementById('right');
  const app = document.getElementById('app');
  let mode = 'none';
  let nodes = [];
  if (state.route.kind === 'server' && state.community && state.ui.membersOpen) {
    mode = 'members';
    nodes = membersNodes(state.community);
  } else if (state.route.kind === 'server' && state.ui.membersOpen) {
    mode = 'members';
    nodes = skeletonRows('member', 5);
  } else if (state.route.kind === 'home' && state.peer && state.ui.profileOpen) {
    mode = 'profile';
    nodes = profilePanelNodes();
  }
  app.dataset.right = mode;
  host.hidden = mode === 'none';
  host.replaceChildren(...nodes);
}
