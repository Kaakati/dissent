// Profile panel for the open DM: banner in the peer's colour, large avatar, names, mutual servers and handles.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, setHue, copyText, toast, initials } from '../ui.js';

export function profilePanelNodes() {
  const peer = state.peer;
  const p = state.peerProfile;
  if (!peer) return [];
  if (!p) return [el('p', { class: 'muted', text: 'Loading profile…' })];

  const banner = setHue(el('div', { class: 'banner' }), peer);
  const mutual = p.mutual_communities;
  const handles = p.handles.filter((h) => h.handle);
  const copyLink = el('button', {
    class: 'btn-secondary',
    icon: 'link',
    text: 'Copy contact link',
    onclick: async () => {
      const link = await actions.contactLink();
      if (link && (await copyText(link))) toast('Contact link copied', 'info');
    },
  });

  return [el('div', { class: 'profile-panel' },
    banner,
    el('div', { class: 'profile-body' },
      avatarWithClass(p.display_name, peer),
      el('h3', { text: p.display_name }),
      el('div', { class: 'suffix', text: `${p.display_name}·${p.suffix}` }),
      el('h4', { text: 'Mutual servers' }),
      mutual.length
        ? el('div', {}, mutual.map((c) => el('button', {
          class: 'server-row',
          title: `Open ${c.name}`,
          onclick: () => actions.openServer(c.id),
        }, setHue(el('span', { class: 'mini-server', text: initials(c.name) }), c.id), el('span', { text: c.name }))))
        : el('p', { class: 'muted', text: 'No mutual servers.' }),
      el('h4', { text: 'Handles' }),
      handles.length
        ? el('div', {}, handles.map((h) => el('div', { class: 'handle-row' },
          el('span', { text: `@${h.handle}` }),
          el('span', { class: 'muted', text: h.community_name }))))
        : el('p', { class: 'muted', text: 'No handles claimed.' })),
    el('div', { class: 'profile-actions' }, copyLink),
  )];
}

function avatarWithClass(name, seed) {
  const node = avatar(name, seed, 'xl');
  node.classList.add('profile-avatar');
  return node;
}
