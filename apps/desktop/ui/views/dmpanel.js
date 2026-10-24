// Profile panel for the open DM, as on the DMRich board: banner, large avatar with presence, name and tag, the
// mutual-contacts and shared-servers line, and "First seen". The action is pinned to the bottom. Handles and the
// safety number live in the full profile dialog, one click away.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, setHue, copyText, toast, openDialog, initials, presenceOf } from '../ui.js';
import { openSafetyDialog } from './dialogs.js';

const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
const shortDate = (ms) => new Date(Number(ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });

async function copyContactLink() {
  const link = await actions.contactLink();
  if (link && (await copyText(link))) toast('Contact link copied', 'info');
}

// Amber when the key changed and nobody compared it again, green when a comparison is on record, muted before any.
function safetyNode(p) {
  if (p.key_changed_ms) return el('div', { class: 'verified-line warn', icon: 'shield-check', text: `Key changed ${shortDate(p.key_changed_ms)}. Compare again.` });
  if (p.verified_ms) return el('div', { class: 'verified-line', icon: 'check', text: `Verified ${shortDate(p.verified_ms)}` });
  return el('p', { class: 'muted', text: 'Not compared yet' });
}

export function profilePanelNodes() {
  const peer = state.peer;
  const p = state.peerProfile;
  if (!peer) return [];
  if (!p) return [el('p', { class: 'muted', text: 'Loading profile…' })];

  const conv = state.conversations.find((c) => c.peer === peer);
  const shared = p.mutual_communities?.length ?? 0;
  const mutual = p.mutual_contacts ?? 0;
  const first = p.first_seen_ms
    ? new Date(Number(p.first_seen_ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
    : null;
  const big = avatar(p.display_name, peer, 'xl', presenceOf(p) ?? presenceOf(conv));
  big.classList.add('profile-avatar');
  return [el('div', { class: 'profile-panel' },
    setHue(el('div', { class: 'banner' }), peer),
    el('div', { class: 'profile-body' },
      big,
      el('h3', { text: p.display_name }),
      el('div', { class: 'suffix', text: `${p.display_name}·${p.suffix}` }),
      el('p', { class: 'profile-meta', text: `${plural(mutual, 'mutual contact')} · ${plural(shared, 'shared server')}` }),
      first ? el('dl', { class: 'kv profile-kv' }, el('dt', { text: 'First seen' }), el('dd', { text: first })) : null),
    el('div', { class: 'profile-actions' },
      el('button', { class: 'btn-outline', text: 'View full profile', onclick: () => openFullProfile(peer) }))),
  ];
}

// The full profile: mutual servers, handles, the safety number, copy the contact link, and verify the safety number.
export function openFullProfile(peer) {
  const p = state.peerProfile;
  if (!p || p.root_id !== peer) return;
  const handles = (p.handles ?? []).filter((h) => h.handle);
  const copyLink = el('button', { class: 'ghost', icon: 'link', text: 'Copy contact link', onclick: copyContactLink });
  const verify = el('button', { class: 'primary', icon: 'shield-check', text: 'Verify safety number', onclick: () => { profile.close(); openSafetyDialog(peer); } });
  const profile = openDialog({
    title: p.display_name,
    content: el('div', { class: 'full-profile' },
      el('p', { class: 'suffix', text: `${p.display_name}·${p.suffix}` }),
      el('h4', { text: 'Mutual servers' }),
      (p.mutual_communities ?? []).length
        ? el('ul', { class: 'plain' }, p.mutual_communities.map((c) => el('li', { class: 'server-row' }, setHue(el('span', { class: 'mini-server', text: initials(c.name) }), c.id), el('span', { text: c.name }))))
        : el('p', { class: 'muted', text: 'No mutual servers.' }),
      el('h4', { text: 'Handles' }),
      handles.length
        ? el('ul', { class: 'plain' }, handles.map((h) => el('li', { class: 'handle-row' }, el('span', { class: 'handle-name', text: `@${h.handle}` }), el('span', { class: 'muted', text: h.community_name }))))
        : el('p', { class: 'muted', text: 'No handles claimed.' }),
      el('h4', { text: 'Safety number' }),
      safetyNode(p),
      el('div', { class: 'row end' }, copyLink, verify)),
  });
}
