// Profile panel for the open DM: a banner with the contact status and More, the 80 px avatar with presence, the name
// and tag, the mutual servers (expandable) and "Contact since". Handles and the safety number live in the full profile
// dialog, one click away.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, setHue, copyText, toast, openDialog, initials, presenceOf, withImage } from '../ui.js';
import { openSafetyDialog } from './dialogs.js';
import { openConversationMenu } from './sidebar.js';

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

  const conv = state.conversations.find((c) => c.peer === peer) ?? { kind: 'dm', peer, display_name: p.display_name, pinned: false };
  const servers = p.mutual_communities ?? [];
  const since = p.contact_since_ms ?? p.first_seen_ms;
  const sinceText = since ? new Date(Number(since)).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' }) : null;
  const big = avatar(p.display_name, peer, 'xl', presenceOf(p) ?? presenceOf(conv));
  big.classList.add('profile-avatar');

  // "N Mutual Servers" opens the list of them under the count. Mutual friends are never shown: contact lists stay private.
  const serverList = el('ul', { class: 'plain mutual-list', hidden: true },
    servers.map((c) => el('li', { class: 'server-row' }, withImage(setHue(el('span', { class: 'mini-server', text: initials(c.name) }), c.id), c.id), el('span', { text: c.name }))));
  const toggle = el('button', {
    class: 'mutual-toggle',
    'aria-expanded': 'false',
    disabled: servers.length === 0,
    text: `${servers.length} Mutual Server${servers.length === 1 ? '' : 's'}`,
    onclick: (e) => {
      serverList.hidden = !serverList.hidden;
      e.currentTarget.setAttribute('aria-expanded', String(!serverList.hidden));
    },
  });

  const status = contactStatus(peer);
  const bannerTools = el('div', { class: 'banner-tools' },
    status ? el('span', { class: 'icon-btn contact-status', role: 'img', 'aria-label': status.label, title: status.label, icon: status.icon }) : null,
    el('button', {
      class: 'icon-btn',
      'aria-label': 'More',
      title: 'More',
      'aria-haspopup': 'menu',
      icon: 'ellipsis',
      onclick: (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openConversationMenu(r.left, r.bottom, conv);
      },
    }));

  return [el('div', { class: 'profile-panel' },
    setHue(el('div', { class: 'banner' }, bannerTools), peer),
    el('div', { class: 'profile-body' },
      big,
      el('h3', { text: p.display_name }),
      el('div', { class: 'suffix', text: `${p.display_name}·${p.suffix}` }),
      el('div', { class: 'mutual' }, toggle, serverList),
      sinceText ? el('h4', { text: 'Contact since' }) : null,
      sinceText ? el('p', { class: 'since', text: sinceText }) : null),
    el('div', { class: 'profile-actions' },
      el('button', { class: 'btn-outline', text: 'View full profile', onclick: () => openFullProfile(peer) }))),
  ];
}

// The contact-status icon on the banner: a contact, or a request waiting either way. Nothing otherwise.
function contactStatus(peer) {
  const contacts = state.contacts;
  if (contacts?.contacts?.some((c) => c.root_id === peer)) return { icon: 'check', label: 'Contact' };
  if (contacts?.outgoing?.some((c) => c.root_id === peer) || contacts?.incoming?.some((c) => c.root_id === peer)) return { icon: 'clock', label: 'Request pending' };
  return null;
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
        ? el('ul', { class: 'plain' }, p.mutual_communities.map((c) => el('li', { class: 'server-row' }, withImage(setHue(el('span', { class: 'mini-server', text: initials(c.name) }), c.id), c.id), el('span', { text: c.name }))))
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
