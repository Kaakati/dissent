// Profile card: opened by clicking a name in messages or the member list. Compact, placed next to the click,
// kept inside the viewport. Only one card is open at a time. Escape and an outside click close it.
import { state, canModerate, myRole } from '../state.js';
import * as actions from '../actions.js';
import { el, avatar, setHue, presenceOf } from '../ui.js';
import { openSafetyDialog } from './dialogs.js';

const CARD_W = 300;
const ROLE_LABEL = { owner: 'Owner', admin: 'Admin', moderator: 'Moderator' };
const MOD_ICON = {
  ban: 'ban',
  unban: 'undo-2',
  grant_moderator: 'shield-plus',
  revoke_moderator: 'shield-minus',
  grant_admin: 'shield-plus',
  revoke_admin: 'shield-minus',
};

let current = null;
let opener = null;

export function closeProfile() {
  const node = current;
  const back = opener;
  current = null;
  opener = null;
  node?.remove();
  document.removeEventListener('pointerdown', onOutside, true);
  document.removeEventListener('keydown', onKey);
  if (back?.isConnected) back.focus();
}

function onOutside(e) {
  if (current && !current.contains(e.target)) closeProfile();
}

function onKey(e) {
  if (e.key === 'Escape') closeProfile();
}

export async function openProfile(userId, x = 0, y = 0) {
  closeProfile();
  opener = document.activeElement;
  const card = el('div', { class: 'profile-card', role: 'dialog', 'aria-label': 'User profile', tabindex: '-1' },
    el('p', { class: 'muted', text: 'Loading…' }));
  document.body.append(card);
  place(card, x, y);
  current = card;
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey);

  const view = await actions.userProfile(userId);
  if (current !== card) return; // closed or replaced while loading
  if (!view) {
    closeProfile();
    return;
  }
  card.replaceChildren(...cardContent(view, userId));
  place(card, x, y);
}

// The card body, shared by the popup card and the settings preview.
export function cardContent(view, userId) {
  const isMe = userId === state.me?.root_id;
  const member = state.community?.members.find((m) => m.root_id === userId) ?? null;
  const banner = setHue(el('div', { class: 'card-banner' }), userId);
  const mutual = view.mutual_communities.length;
  const chip = member && ROLE_LABEL[member.role]
    ? el('span', { class: `chip role-${member.role}`, text: ROLE_LABEL[member.role] })
    : null;
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
  if (!isMe) {
    buttons.push(el('button', {
      class: 'btn-secondary',
      icon: 'shield-check',
      text: 'Verify safety number',
      onclick: () => {
        closeProfile();
        openSafetyDialog(userId);
      },
    }));
  }
  buttons.push(...moderationButtons(userId));
  return [
    banner,
    el('div', { class: 'card-body' },
      avatar(view.display_name, userId, 'card-avatar', presenceOf(view) ?? presenceOf(member)),
      el('div', { class: 'card-name', text: view.display_name, title: view.display_name }),
      el('div', { class: 'card-suffix mono', text: `${view.display_name}·${view.suffix}` }),
      member?.handle ? el('div', { class: 'card-handle', text: `@${member.handle} in ${state.community.name}` }) : null,
      chip ? el('div', { class: 'chips' }, chip) : null,
      el('div', { class: 'card-meta', text: mutual ? `${mutual} mutual server${mutual === 1 ? '' : 's'}` : 'No mutual servers' }),
      buttons.length ? el('div', { class: 'card-actions row wrap' }, buttons) : null),
  ];
}

// Moderation is offered only inside an open server, to owner, admin and moderator, never on yourself or the owner.
// These rules mirror dissent-events; the engine enforces them too.
function moderationButtons(userId) {
  const community = state.community;
  if (state.route.kind !== 'server' || !community || !canModerate()) return [];
  if (userId === state.me?.root_id) return [];
  const member = community.members.find((m) => m.root_id === userId);
  if (!member) return [];
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

// Next to the click point, moved back inside the window when it would overflow.
function place(node, x = 0, y = 0) {
  node.style.setProperty('width', `${CARD_W}px`);
  const r = node.getBoundingClientRect();
  const left = Math.min(Math.max(8, x + 12), window.innerWidth - r.width - 8);
  const top = Math.min(Math.max(8, y - 12), window.innerHeight - r.height - 8);
  node.style.setProperty('left', `${left}px`);
  node.style.setProperty('top', `${top}px`);
}
