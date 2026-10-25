// DOM helpers. User-supplied text only ever goes in through textContent or text nodes, never innerHTML.
import { imageOf } from './avatars.js';

export function el(tag, props, ...children) {
  const node = document.createElement(tag);
  let iconName = null;
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === 'text') node.textContent = String(value);
    else if (key === 'icon') iconName = value;
    else if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (typeof value === 'boolean' || key in node) node[key] = value;
    else node.setAttribute(key, String(value));
  }
  if (iconName) node.prepend(icon(iconName));
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const SVG_NS = 'http://www.w3.org/2000/svg';

// A Lucide icon (or the Dissent mark) from the vendored sprite, icons.svg.
// Decorative only: put any accessible name on the button that contains it.
export function icon(name) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'icon');
  svg.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS(SVG_NS, 'use');
  use.setAttribute('href', `icons.svg#i-${name}`);
  svg.append(use);
  return svg;
}

// ---- avatars: a hue from the id's hash with initials on top, or the picture when the id has one ----

export function hueOf(seed) {
  const forced = globalThis.__mockHues?.[String(seed)];
  if (forced !== undefined) return forced;
  let h = 0x811c9dc5;
  for (const ch of String(seed ?? '')) h = Math.imul(h ^ ch.codePointAt(0), 0x01000193);
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  return (h >>> 0) % 360;
}

export function initials(name) {
  const parts = String(name ?? '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return [...parts[0]].slice(0, 2).join('').toUpperCase();
  // Three or more words use the first and last ("Open Source Arabic" is OA, as on the rail).
  const picks = parts.length > 2 ? [parts[0], parts[parts.length - 1]] : parts.slice(0, 2);
  return picks.map((p) => [...p][0]).join('').toUpperCase();
}

// size: '' (32px), 'xs' (16), 'sm' (24), 'lg' (40), 'xl' (80), 'card-avatar' (88). Colour follows the seed.
// presence: a status ('online' | 'idle' | 'dnd' | 'offline') adds a presence dot; so does a boolean (true online, false
// offline). undefined (no presence data) adds nothing.
export function avatar(name, seed, size = '', presence) {
  const node = el('span', { class: `avatar ${size}`.trim(), text: initials(name) });
  node.style.setProperty('--hue', String(hueOf(seed || name)));
  withImage(node, seed);
  if (typeof presence === 'boolean' || PRESENCE.includes(presence)) node.append(presenceDot(presence));
  return node;
}

// Lays the picture for `id` over a node's initials, when the id has one. A picture that fails to load is removed, so
// the initials show again. The node must be positioned (every avatar and mark is).
export function withImage(node, id) {
  const src = imageOf(id);
  if (!src || node.querySelector(':scope > img')) return node;
  const img = el('img', { src, alt: '', onerror: () => img.remove() });
  img.draggable = false;
  node.classList.add('has-image');
  node.append(img);
  return node;
}

// The four presence states. Each has its own glyph shape (style.css), so colour is never the only cue.
export const PRESENCE = ['online', 'idle', 'dnd', 'offline'];
export const PRESENCE_LABEL = { online: 'Online', idle: 'Idle', dnd: 'Do Not Disturb', offline: 'Offline' };

// The presence of a view: its `status` string, else `online` as online or offline. undefined when the view has neither.
export function presenceOf(view) {
  if (PRESENCE.includes(view?.status)) return view.status;
  if (typeof view?.online === 'boolean') return view.online ? 'online' : 'offline';
  return undefined;
}

export function presenceDot(presence) {
  const status = presence === true ? 'online' : presence === false ? 'offline' : presence;
  return el('span', { class: `presence ${status}`, role: 'img', 'aria-label': PRESENCE_LABEL[status] });
}

// Two small avatars, one over the other, for a group conversation.
export function stackAvatar(members, size = '') {
  const two = (members ?? []).slice(0, 2);
  return el('span', { class: `stack-avatar ${size}`.trim() }, two.map((m) => avatar(m.display_name, m.root_id)));
}

// Sets the --hue custom property on any node, for banners and name colours.
export function setHue(node, seed) {
  node.style.setProperty('--hue', String(typeof seed === 'number' ? seed : hueOf(seed)));
  return node;
}

// ---- text: links are built as elements, never parsed as HTML ----

const URL_RE = /\bhttps?:\/\/[^\s<>"']+/g;

// Returns a list of text nodes and <a> elements. Only http and https links become anchors.
export function linkify(text) {
  const out = [];
  let last = 0;
  for (const match of String(text).matchAll(URL_RE)) {
    if (match.index > last) out.push(document.createTextNode(text.slice(last, match.index)));
    out.push(el('a', { href: match[0], target: '_blank', rel: 'noopener noreferrer', text: match[0] }));
    last = match.index + match[0].length;
  }
  if (last < text.length) out.push(document.createTextNode(text.slice(last)));
  return out;
}

// ---- time: 24-hour clock, relative day words for the last week ----

const DAY_MS = 24 * 60 * 60 * 1000;
const startOfDay = (ms) => { const d = new Date(Number(ms)); d.setHours(0, 0, 0, 0); return d.getTime(); };

export function dayKey(ms) {
  const d = new Date(Number(ms));
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

// "Today", "Yesterday", "Thursday, October 22" within the year, else "October 21, 2025".
export function dayLabel(ms) {
  const days = Math.round((startOfDay(Date.now()) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  const d = new Date(Number(ms));
  if (d.getFullYear() === new Date().getFullYear()) return d.toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' });
  return d.toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' });
}

export function clockLabel(ms) {
  return new Date(Number(ms)).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

// "Today 09:14", "Yesterday 22:40", "Thu 18:02" within six days, else "10/21/2026 17:02".
export function whenLabel(ms) {
  const days = Math.round((startOfDay(Date.now()) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return `Today ${clockLabel(ms)}`;
  if (days === 1) return `Yesterday ${clockLabel(ms)}`;
  if (days > 1 && days < 7) return `${new Date(Number(ms)).toLocaleDateString('en-US', { weekday: 'short' })} ${clockLabel(ms)}`;
  return stampLabel(ms);
}

export function stampLabel(ms) {
  const d = new Date(Number(ms));
  return `${d.toLocaleDateString('en-US', { month: '2-digit', day: '2-digit', year: 'numeric' })} ${clockLabel(ms)}`;
}

// "Oct 12" for safety-number and key-change dates.
export function shortDate(ms) {
  return new Date(Number(ms)).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

// ---- feedback ----

export function toast(err, kind = 'error') {
  const text = typeof err === 'string' ? err : (err?.message ?? String(err));
  const host = document.getElementById('toasts');
  const node = el('div', {
    class: `toast ${kind}`,
    role: 'status',
    icon: kind === 'info' ? 'info' : 'circle-alert',
    text,
  });
  host.append(node);
  setTimeout(() => node.remove(), 4500);
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = el('textarea', { value: text });
    document.body.append(ta);
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return ok;
  }
}

// Opens a native modal <dialog>. Returns { dialog, close }. onClose runs once the dialog is dismissed.
// `sub` is a one-line description under the title; `wide` uses the 600 px variant; `footer` is a row of buttons.
export function openDialog({ title, sub, content, footer, onClose, wide = false, className = '' }) {
  const dialog = el('dialog', { class: ['dialog', wide && 'wide', className].filter(Boolean).join(' ') });
  const close = () => dialog.close();
  dialog.append(
    el('header', { class: 'dialog-head' },
      el('div', {}, el('h2', { text: title }), sub ? el('p', { class: 'sub', text: sub }) : null),
      el('button', { class: 'icon-btn', 'aria-label': 'Close', icon: 'x', onclick: close })),
    el('div', { class: 'dialog-body' }, content, footer ? el('div', { class: 'row end' }, footer) : null),
  );
  const opener = document.activeElement;
  dialog.addEventListener('close', () => {
    dialog.remove();
    onClose?.();
    if (opener?.isConnected) opener.focus();
  });
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, close };
}

// Shimmer placeholders while data loads. kind: 'message' | 'channel' | 'member'.
export function skeletonRows(kind, count = 6) {
  return Array.from({ length: count }, (_, i) => {
    if (kind === 'message') {
      return el('div', { class: 'skel-msg', 'aria-hidden': 'true' },
        el('span', { class: 'skel skel-circle' }),
        el('div', { class: 'skel-lines' },
          el('span', { class: 'skel skel-line' }),
          el('span', { class: 'skel skel-line short' })));
    }
    if (kind === 'member') {
      return el('div', { class: 'skel-row', 'aria-hidden': 'true' },
        el('span', { class: 'skel skel-circle sm' }),
        el('span', { class: 'skel skel-line' }));
    }
    return el('div', { class: 'skel-row', 'aria-hidden': 'true' }, el('span', { class: 'skel skel-line' }));
  });
}

// "1 shared server" / "2 shared servers".
export const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
