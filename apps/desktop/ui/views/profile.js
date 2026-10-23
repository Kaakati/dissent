// Profile popover opened by clicking an author name or a member. Only one is open at a time.
import { state, canModerate, myRole } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar } from '../ui.js';

let current = null;

export function closeProfile() {
  current?.remove();
  current = null;
  document.removeEventListener('pointerdown', onOutside, true);
  document.removeEventListener('keydown', onKey);
}

function onOutside(e) {
  if (current && !current.contains(e.target)) closeProfile();
}

function onKey(e) {
  if (e.key === 'Escape') closeProfile();
}

export async function openProfile(userId, x, y) {
  closeProfile();
  const pop = el('div', { class: 'popover', role: 'dialog', 'aria-label': 'Profile' },
    el('p', { class: 'muted', text: 'Loading…' }));
  document.body.append(pop);
  place(pop, x, y);
  current = pop;
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey);

  const view = await actions.userProfile(userId);
  if (current !== pop) return; // closed or replaced while loading
  if (!view) {
    closeProfile();
    return;
  }
  pop.replaceChildren(...content(view, userId));
  place(pop, x, y);
}

function content(view, userId) {
  const isMe = userId === state.me?.root_id;
  const handles = view.handles.map((h) => el('li', {
    text: `${h.community_name}: ${h.handle ?? 'no handle'}`,
  }));
  const mutual = view.mutual_communities.map((c) => el('li', { text: c.name }));
  const buttons = [];
  if (!isMe) {
    buttons.push(el('button', {
      class: 'primary',
      icon: 'message-square',
      text: 'Message',
      onclick: () => {
        closeProfile();
        actions.openDm(userId, view.display_name);
      },
    }));
  }
  buttons.push(...moderationButtons(userId));
  return [
    el('div', { class: 'pop-head' },
      avatar(view.display_name, userId),
      el('div', {},
        el('strong', { text: view.display_name }),
        el('div', { class: 'muted mono', text: `${view.display_name}·${view.suffix}` }))),
    el('h4', { text: 'Shared servers' }),
    handles.length ? el('ul', { class: 'plain' }, handles) : el('p', { class: 'muted', text: 'No shared servers.' }),
    el('h4', { text: 'Mutual servers' }),
    mutual.length ? el('ul', { class: 'plain' }, mutual) : el('p', { class: 'muted', text: 'None.' }),
    buttons.length ? el('div', { class: 'row wrap' }, buttons) : null,
  ];
}

// Moderation is only offered inside an open server, to owner/admin/moderator, and never on yourself or the owner.
function moderationButtons(userId) {
  const community = state.community;
  if (state.route.kind !== 'server' || !community || !canModerate()) return [];
  if (userId === state.me?.root_id) return [];
  const member = community.members.find((m) => m.root_id === userId);
  if (!member) return [];

  // Mirrors the rules in dissent-events. The engine enforces them too; the UI only hides what it would reject.
  const mine = myRole();
  const target = member.role;
  if (target === 'owner') return [];
  const canBan = mine === 'owner' || (mine === 'admin' && target !== 'admin') || (mine === 'moderator' && target === 'member');
  const canModRole = (mine === 'owner' || mine === 'admin') && (target === 'member' || target === 'moderator');
  const canAdminRole = mine === 'owner';

  const act = (label, action, cls = '') => el('button', {
    class: cls,
    icon: MOD_ICON[action],
    text: label,
    onclick: async () => {
      closeProfile();
      await actions.moderate(community.id, action, userId);
    },
  });
  const out = [];
  if (canBan) out.push(member.banned ? act('Unban', 'unban') : act('Ban', 'ban', 'danger'));
  if (canModRole) out.push(target === 'moderator' ? act('Revoke Moderator', 'revoke_moderator') : act('Grant Moderator', 'grant_moderator'));
  if (canAdminRole) out.push(target === 'admin' ? act('Revoke Admin', 'revoke_admin') : act('Grant Admin', 'grant_admin'));
  return out;
}

const MOD_ICON = {
  ban: 'ban',
  unban: 'undo-2',
  grant_moderator: 'shield-plus',
  revoke_moderator: 'shield-minus',
  grant_admin: 'shield-plus',
  revoke_admin: 'shield-minus',
};

// Keeps the popover inside the window, near the click point.
function place(node, x = 0, y = 0) {
  const r = node.getBoundingClientRect();
  const left = Math.min(Math.max(8, x), window.innerWidth - r.width - 8);
  const top = Math.min(Math.max(8, y), window.innerHeight - r.height - 8);
  node.style.setProperty('left', `${left}px`);
  node.style.setProperty('top', `${top}px`);
}
