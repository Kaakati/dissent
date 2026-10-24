// Quick switcher (Ctrl+K / Cmd+K): one search field over servers, channels and direct messages.
import { state } from '../state.js';
import * as actions from '../actions.js';
import { api } from '../api.js';
import { el, icon, avatar, initials, setHue } from '../ui.js';

let panel = null; // { overlay, input, list, items, active, opener, all }

// Lower rank sorts first: prefix matches, then word starts, then any substring.
function rank(label, meta, q) {
  const l = label.toLowerCase();
  if (l.startsWith(q)) return 0;
  if (l.split(/[\s#·]+/).some((w) => w.startsWith(q))) return 1;
  if (l.includes(q) || meta.toLowerCase().includes(q)) return 2;
  return -1;
}

export function openSwitcher() {
  if (panel) return;
  const input = el('input', {
    type: 'search',
    placeholder: 'Where do you want to go?',
    'aria-label': 'Search servers, channels and people',
    'aria-controls': 'switcher-list',
    'aria-autocomplete': 'list',
    autocomplete: 'off',
  });
  const list = el('ul', { class: 'switcher-list', id: 'switcher-list', role: 'listbox', 'aria-label': 'Results' });
  const overlay = el('div', { class: 'switcher-overlay' },
    el('div', { class: 'switcher', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Quick switcher' },
      el('div', { class: 'switcher-field' }, icon('search'), input),
      list,
      el('p', { class: 'switcher-hint muted', text: '↑↓ to move · Enter to open · Esc to close' })));
  overlay.addEventListener('pointerdown', (e) => {
    if (e.target === overlay) closeSwitcher();
  });
  document.body.append(overlay);
  panel = { overlay, input, list, items: [], active: 0, opener: document.activeElement, all: buildBase() };
  input.addEventListener('input', () => render());
  input.addEventListener('keydown', onKey);
  input.focus();
  render();
  // Channels of every server come from one request each. They are added when they arrive.
  loadChannels().then(() => {
    if (panel) render();
  });
}

export function closeSwitcher() {
  const back = panel?.opener;
  panel?.overlay.remove();
  panel = null;
  if (back?.isConnected) back.focus();
}

function onKey(e) {
  if (e.key === 'Escape') {
    e.preventDefault();
    closeSwitcher();
  } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    const n = panel.items.length;
    if (!n) return;
    panel.active = (panel.active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
    paintActive();
  } else if (e.key === 'Enter') {
    e.preventDefault();
    const item = panel?.items[panel.active];
    if (item) runItem(item);
  }
}

function runItem(item) {
  closeSwitcher();
  item.run();
}

// Servers and direct messages are known immediately. Channels are added once loaded.
function buildBase() {
  const servers = state.communities.map((c) => ({
    kind: 'server',
    seed: c.id,
    label: c.name,
    meta: 'Server',
    unread: [...state.unread].some((k) => k.startsWith(`ch:${c.id}:`)),
    run: () => actions.openServer(c.id),
  }));
  const dms = state.conversations.map((conv) => (conv.kind === 'group'
    ? {
      kind: 'group',
      label: conv.name,
      meta: 'Group',
      unread: state.unread.has(`group:${conv.id}`),
      run: () => actions.openGroup(conv),
    }
    : {
      kind: 'dm',
      label: conv.display_name,
      meta: 'Direct message',
      peer: conv.peer,
      unread: state.unread.has(`dm:${conv.peer}`),
      run: () => actions.openDm(conv.peer, conv.display_name),
    }));
  return [...servers, ...dms];
}

async function loadChannels() {
  const channels = [];
  for (const c of state.communities) {
    let view;
    try {
      view = await api.communityView(c.id);
    } catch {
      continue;
    }
    for (const ch of view.channels) {
      channels.push({
        kind: 'channel',
        label: ch.name,
        meta: c.name,
        unread: state.unread.has(`ch:${c.id}:${ch.id}`),
        run: async () => {
          await actions.openServer(c.id);
          await actions.openChannel(ch.id);
        },
      });
    }
  }
  if (panel) panel.all = [...buildBase().slice(0, state.communities.length), ...channels, ...buildBase().slice(state.communities.length)];
}

function render() {
  if (!panel) return;
  const q = panel.input.value.trim().toLowerCase();
  const scored = panel.all
    .map((item) => ({ item, score: q ? rank(item.label, item.meta, q) : 0 }))
    .filter((s) => s.score >= 0)
    .sort((a, b) => a.score - b.score);
  panel.items = scored.slice(0, 50).map((s) => s.item);
  panel.active = Math.min(panel.active, Math.max(0, panel.items.length - 1));
  panel.list.replaceChildren(...(panel.items.length
    ? panel.items.map((item, i) => row(item, i))
    : [el('li', { class: 'switcher-empty muted', role: 'presentation', text: 'Nothing matches.' })]));
  paintActive();
}

function row(item, index) {
  const option = el('li', {
    class: 'switcher-item',
    role: 'option',
    id: `switcher-opt-${index}`,
    'aria-selected': 'false',
    onclick: () => runItem(item),
    onmousemove: () => {
      if (panel && panel.active !== index) {
        panel.active = index;
        paintActive();
      }
    },
  });
  let lead;
  if (item.kind === 'dm') lead = avatar(item.label, item.peer, 'sm');
  else if (item.kind === 'server') lead = setHue(el('span', { class: 'mini-server', text: initials(item.label) }), item.seed);
  else lead = icon('hash');
  option.append(...[
    lead,
    el('span', { class: 'switcher-label', text: item.label }),
    el('span', { class: 'switcher-meta muted', text: item.meta }),
    item.unread ? el('span', { class: 'unread-dot inline', 'aria-label': 'unread' }) : null,
  ].filter(Boolean));
  return option;
}

function paintActive() {
  if (!panel) return;
  panel.input.setAttribute('aria-activedescendant', panel.items.length ? `switcher-opt-${panel.active}` : '');
  [...panel.list.querySelectorAll('[role="option"]')].forEach((node, i) => {
    const on = i === panel.active;
    node.setAttribute('aria-selected', on ? 'true' : 'false');
    node.classList.toggle('active', on);
    if (on) node.scrollIntoView({ block: 'nearest' });
  });
}
