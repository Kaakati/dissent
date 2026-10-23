// Right column (servers only): members grouped by role. Banned members are hidden.
import { state, subscribe } from '../state.js';
import { el, avatar } from '../ui.js';
import { openProfile } from './profile.js';

const GROUPS = [
  ['Owner', 'owner'],
  ['Admins', 'admin'],
  ['Moderators', 'moderator'],
  ['Members', 'member'],
];

export function initMembers() {
  subscribe('members', render);
  render();
}

function render() {
  const column = document.getElementById('members');
  const community = state.route.kind === 'server' ? state.community : null;
  column.hidden = !community;
  if (!community) {
    column.replaceChildren();
    return;
  }
  const nodes = [];
  for (const [label, role] of GROUPS) {
    const people = community.members.filter((m) => m.role === role && !m.banned);
    if (people.length === 0) continue;
    nodes.push(el('h3', { class: 'group-title', text: `${label} (${people.length})` }));
    for (const m of people) {
      nodes.push(el('button', {
        class: 'member',
        title: `${m.display_name}·${m.suffix}`,
        onclick: (e) => openProfile(m.root_id, e.clientX, e.clientY),
      }, avatar(m.display_name, m.root_id),
      el('span', { class: 'member-name', text: m.display_name }),
      m.handle ? el('span', { class: 'handle', text: `@${m.handle}` }) : null));
    }
  }
  column.replaceChildren(...nodes);
}
