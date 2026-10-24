// Member list for the right column (servers). Grouped by role, banned members hidden.
import { avatar, el } from '../ui.js';
import { openProfile } from './profile.js';

const GROUPS = [
  ['Owner', 'owner'],
  ['Admins', 'admin'],
  ['Moderators', 'moderator'],
  ['Members', 'member'],
];

export function membersNodes(community) {
  const nodes = [];
  for (const [label, role] of GROUPS) {
    const people = community.members.filter((m) => m.role === role && !m.banned);
    if (people.length === 0) continue;
    nodes.push(el('h3', { class: 'group-title', text: `${label.toUpperCase()} — ${people.length}` }));
    for (const m of people) {
      nodes.push(el('button', {
        class: 'member',
        title: `${m.display_name}·${m.suffix}`,
        onclick: (e) => openProfile(m.root_id, e.clientX, e.clientY),
      },
      avatar(m.display_name, m.root_id),
      el('span', { class: `member-name role-${m.role}`, text: m.display_name }),
      m.handle ? el('span', { class: 'handle', text: `@${m.handle}` }) : null));
    }
  }
  return nodes;
}
