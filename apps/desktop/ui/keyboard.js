// Keyboard shortcuts: Ctrl/Cmd+K quick switcher, Alt+Up/Down between channels or DMs,
// Alt+Shift+Up/Down to the next or previous unread, and arrow keys through the server rail.
import { state } from './state.js';
import * as actions from './actions.js';
import { openSwitcher } from './views/switcher.js';
import { openSettingsPage } from './views/settings.js';

export function initKeyboard() {
  document.addEventListener('keydown', onKey);
  document.getElementById('rail').addEventListener('keydown', onRailKey);
}

function onKey(e) {
  if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    openSwitcher();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key === ',') {
    e.preventDefault();
    openSettingsPage();
    return;
  }
  if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
  e.preventDefault();
  const dir = e.key === 'ArrowUp' ? -1 : 1;
  if (e.shiftKey) jumpUnread(dir);
  else step(dir);
}

// The conversations in the current context, in the order the sidebar shows them.
function entries() {
  if (state.route.kind === 'server' && state.community) {
    return state.community.channels.map((ch) => ({
      key: `ch:${state.community.id}:${ch.id}`,
      open: () => actions.openChannel(ch.id),
      active: state.channelId === ch.id,
    }));
  }
  return state.conversations.map((conv) => ({
    key: `dm:${conv.peer}`,
    open: () => actions.openDm(conv.peer, conv.display_name),
    active: state.route.kind === 'home' && state.peer === conv.peer,
  }));
}

function step(dir) {
  const list = entries();
  if (list.length === 0) return;
  const current = list.findIndex((x) => x.active);
  const next = current < 0 ? (dir > 0 ? 0 : list.length - 1) : (current + dir + list.length) % list.length;
  list[next].open();
}

function jumpUnread(dir) {
  const list = entries();
  if (list.length === 0) return;
  const start = list.findIndex((x) => x.active);
  for (let i = 1; i <= list.length; i++) {
    const idx = start < 0
      ? (dir > 0 ? i - 1 : list.length - i)
      : (start + dir * i + list.length * 2) % list.length;
    if (state.unread.has(list[idx].key)) {
      list[idx].open();
      return;
    }
  }
}

// Up and Down move focus between rail buttons, wrapping at the ends. Enter or Space activates, as on any button.
function onRailKey(e) {
  if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
  const buttons = [...e.currentTarget.querySelectorAll('button')];
  const i = buttons.indexOf(document.activeElement);
  if (i < 0) return;
  e.preventDefault();
  const next = (i + (e.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length;
  buttons[next].focus();
}
