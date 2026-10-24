// Custom 30 px title bar: in-app history (Back and Forward), the context label in the middle, and the window
// controls. The bar is a drag region. In the browser preview there is no window, so the controls do nothing.
import { el, icon, initials, setHue } from '../ui.js';
import { state, subscribe } from '../state.js';
import * as actions from '../actions.js';

const win = () => window.__TAURI__?.window?.getCurrentWindow?.();
const HISTORY_MAX = 50;

// ---- history: one entry per place the user has been ----

const back = [];
const forward = [];
let restoring = false;
let lastKey = null;

// Where the user is, as a replayable entry.
function place() {
  if (state.ui.page) return { kind: 'page', page: state.ui.page };
  if (state.route.kind === 'server') {
    if (state.voiceChannelId) return { kind: 'voice', communityId: state.route.communityId, channelId: state.voiceChannelId };
    return { kind: 'server', communityId: state.route.communityId, channelId: state.channelId };
  }
  if (state.group) return { kind: 'group', id: state.group.id };
  if (state.peer) return { kind: 'dm', peer: state.peer, name: state.peerName };
  return { kind: 'home' };
}

const keyOf = (p) => JSON.stringify(p);

function go(p) {
  switch (p.kind) {
    case 'page': return actions.openPage(p.page);
    case 'server': return actions.openServer(p.communityId, p.channelId);
    case 'voice': return actions.openServer(p.communityId).then(() => actions.openVoiceChannel(p.channelId));
    case 'group': {
      const conv = state.conversations.find((c) => c.kind === 'group' && c.id === p.id);
      return conv ? actions.openGroup(conv) : actions.openHome();
    }
    case 'dm': return actions.openDm(p.peer, p.name);
    default: return actions.openHome();
  }
}

async function travel(from, to) {
  const entry = from.pop();
  if (!entry) return;
  to.push(place());
  restoring = true;
  try {
    await go(entry);
    lastKey = keyOf(place());
  } finally {
    restoring = false;
  }
  renderNav();
}

function onNavigate() {
  const now = place();
  const key = keyOf(now);
  if (key === lastKey || restoring) return;
  // The entry pushed is where we were, so Back returns there.
  if (lastKey !== null) {
    back.push(JSON.parse(lastKey));
    if (back.length > HISTORY_MAX) back.shift();
    forward.length = 0;
  }
  lastKey = key;
  renderNav();
}

function renderNav() {
  document.getElementById('tb-back').disabled = back.length === 0;
  document.getElementById('tb-forward').disabled = forward.length === 0;
}

// ---- context label: server with its mark, Direct Messages, a page, or a call ----

function renderTitle() {
  const logo = document.getElementById('tb-logo');
  const title = document.getElementById('tb-title');
  let text = 'Direct Messages';
  let mark = el('span', { class: 'titlebar-icon brand' }, icon('dissent-mark'));
  if (state.ui.page === 'contacts') { text = 'Contacts'; mark = icon('users'); }
  else if (state.ui.page === 'transfers') { text = 'Transfers'; mark = icon('arrow-up-down'); }
  else if (state.route.kind === 'server' && state.community) {
    text = state.community.name;
    mark = setHue(el('span', { class: 'mini-server xs', text: initials(state.community.name) }), state.community.id);
  } else if (state.route.kind === 'server') {
    text = '';
    mark = icon('server');
  }
  logo.replaceChildren(mark);
  logo.setAttribute('data-tauri-drag-region', '');
  title.textContent = text;
}

// Settings and onboarding take the bar over while they are open.
export function setTitleOverride(text, iconName) {
  if (text === null) {
    renderTitle();
    return;
  }
  document.getElementById('tb-logo').replaceChildren(iconName === 'dissent-mark' ? el('span', { class: 'titlebar-icon brand' }, icon(iconName)) : icon(iconName));
  document.getElementById('tb-title').textContent = text;
}

function control(id, name, label, act) {
  const node = document.getElementById(id);
  node.append(name === 'minus' ? minimizeGlyph() : name === 'x' ? closeGlyph() : name === 'square' ? windowGlyph() : icon(name));
  node.setAttribute('aria-label', label);
  if (!win()) node.classList.add('is-noop');
  node.addEventListener('click', () => {
    try {
      act(win());
    } catch {
      // the window API is unavailable or refused the call; nothing else to do here
    }
  });
}

export function initTitlebar() {
  const backBtn = document.getElementById('tb-back');
  const fwdBtn = document.getElementById('tb-forward');
  backBtn.append(icon('arrow-left'));
  fwdBtn.append(icon('arrow-right'));
  backBtn.addEventListener('click', () => travel(back, forward));
  fwdBtn.addEventListener('click', () => travel(forward, back));
  control('tb-min', 'minus', 'Minimize', (w) => w?.minimize());
  control('tb-max', 'square', 'Maximize', async (w) => {
    await w?.toggleMaximize();
    syncMaximize();
  });
  // The button shows restore (two overlapping squares) while the window is maximized.
  window.addEventListener('resize', syncMaximize);
  syncMaximize();
  control('tb-close', 'x', 'Close', (w) => w?.close());
  // Double-click on the bar toggles maximize, as a native title bar does.
  document.getElementById('titlebar').addEventListener('dblclick', (e) => {
    if (e.target.closest('button')) return;
    try { win()?.toggleMaximize(); } catch { /* no window */ }
  });
  document.addEventListener('keydown', (e) => {
    if (!e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === 'ArrowLeft' && back.length) { e.preventDefault(); travel(back, forward); }
    if (e.key === 'ArrowRight' && forward.length) { e.preventDefault(); travel(forward, back); }
  });
  subscribe('title', () => { renderTitle(); onNavigate(); });
  subscribe('main', renderTitle);
  renderTitle();
  lastKey = keyOf(place());
  renderNav();
}

export const historyBack = () => travel(back, forward);
export const historyForward = () => travel(forward, back);

// Swaps the maximize glyph and label to match the window state. Browser preview: stays on maximize.
async function syncMaximize() {
  const btn = document.getElementById('tb-max');
  let maximized = false;
  try {
    maximized = Boolean(await win()?.isMaximized());
  } catch {
    maximized = false;
  }
  btn.replaceChildren(maximized ? restoreGlyph() : windowGlyph());
  btn.setAttribute('aria-label', maximized ? 'Restore' : 'Maximize');
  btn.title = maximized ? 'Restore' : 'Maximize';
}

// Window controls use 1 px strokes at 12 px, as in the board. Restore is the standard two overlapping squares.
const SVG_NS = 'http://www.w3.org/2000/svg';
function glyph(children) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('width', '12');
  svg.setAttribute('height', '12');
  svg.setAttribute('viewBox', '0 0 12 12');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '1');
  for (const child of children) svg.append(child);
  return svg;
}
function rect(x, y, w, h) {
  const r = document.createElementNS(SVG_NS, 'rect');
  for (const [k, v] of Object.entries({ x, y, width: w, height: h })) r.setAttribute(k, String(v));
  return r;
}
function restoreGlyph() {
  // back square: top-right; front square: bottom-left, with its top edge over the back square.
  return glyph([
    (() => { const p = document.createElementNS(SVG_NS, 'path'); p.setAttribute('d', 'M3 3V1.5h7.5V9H9'); return p; })(),
    rect(1.5, 3.5, 7, 7),
  ]);
}
function windowGlyph() {
  return glyph([rect(1.5, 1.5, 9, 9)]);
}
function minimizeGlyph() {
  const p = document.createElementNS(SVG_NS, 'path');
  p.setAttribute('d', 'M1 6h10');
  return glyph([p]);
}
function closeGlyph() {
  const p = document.createElementNS(SVG_NS, 'path');
  p.setAttribute('d', 'M1 1l10 10M11 1 1 11');
  return glyph([p]);
}
