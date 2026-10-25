// The profile popover on your name in the user bar: your avatar with its status dot, name and tag, then the status row.
// The status row opens a submenu of the four statuses; Idle, Do Not Disturb and Invisible open a duration submenu.
// It is a context menu, so the arrows, Enter, Esc and focus return work as they do everywhere else.
import { state } from '../state.js';
import { setStatus } from '../actions.js';
import { el, avatar, clockLabel } from '../ui.js';
import { shortOf } from '../address.js';
import { openContextMenu } from './contextmenu.js';

const MIN = 60 * 1000;
const DAY = 24 * 60 * MIN;
const DURATIONS = [
  ['For 15 minutes', 15 * MIN],
  ['For 1 hour', 60 * MIN],
  ['For 8 hours', 8 * 60 * MIN],
  ['For 24 hours', DAY],
  ['For 3 days', 3 * DAY],
  ['Forever', null],
];

// Menu order. `presence` picks the glyph shape; `timed` statuses open the duration submenu.
const CHOICES = [
  { status: 'online', label: 'Online', presence: 'online' },
  { status: 'idle', label: 'Idle', presence: 'idle', timed: true },
  { status: 'dnd', label: 'Do Not Disturb', presence: 'dnd', timed: true, sub: 'You will not receive desktop notifications' },
  { status: 'invisible', label: 'Invisible', presence: 'offline', timed: true, sub: 'You will appear offline' },
];

const choiceOf = (status) => CHOICES.find((c) => c.status === status) ?? CHOICES[0];

export const statusLabel = (status) => choiceOf(status).label;

// The glyph of your own status. Invisible keeps its own hollow ring here, while others see you as offline.
export const selfPresence = () => choiceOf(state.status.status).presence;

function glyph(presence) {
  return el('span', { class: `status-glyph presence ${presence}`, 'aria-hidden': 'true' });
}

function statusItems() {
  return CHOICES.map((c) => ({
    label: c.label,
    sub: c.sub,
    leading: glyph(c.presence),
    onPick: c.timed ? undefined : () => setStatus(c.status, null),
    submenu: c.timed ? DURATIONS.map(([label, ms]) => ({
      label,
      onPick: () => setStatus(c.status, ms === null ? null : Date.now() + ms),
    })) : undefined,
  }));
}

// Opens above the user bar, which sits at the foot of the window.
export function openStatusMenu(anchor) {
  const me = state.me;
  const { status, until_ms: untilMs } = state.status;
  const rect = anchor.getBoundingClientRect();
  anchor.focus({ preventScroll: true });
  openContextMenu(rect.left + 8, rect.top - 8, [
    {
      header: el('span', { class: 'status-card' },
        avatar(me?.display_name || '?', me?.root_id, 'xl', selfPresence()),
        el('span', { class: 'status-id' },
          el('span', { class: 'name', text: me?.display_name || 'Set a display name' }),
          el('span', { class: 'tag', text: me ? shortOf('user', me.root_id) : '' }))),
    },
    'sep',
    {
      label: statusLabel(status),
      sub: untilMs ? `Until ${clockLabel(untilMs)}` : undefined,
      leading: glyph(selfPresence()),
      submenu: statusItems(),
    },
  ], { placeAbove: true });
}
