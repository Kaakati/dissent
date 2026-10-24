// DOM helpers. User-supplied text only ever goes in through textContent or text nodes, never innerHTML.

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

// ---- generated avatars: a hue from the id's hash, initials on top. No images. ----

export function hueOf(seed) {
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
  return parts.slice(0, 2).map((p) => [...p][0]).join('').toUpperCase();
}

// size: '' (32px), 'lg' (40px), 'xl' (80px). Colour follows the seed, so one person always gets one colour.
export function avatar(name, seed, size = '') {
  const node = el('span', { class: `avatar ${size}`.trim(), text: initials(name) });
  node.style.setProperty('--hue', String(hueOf(seed || name)));
  return node;
}

// Sets the --hue custom property on any node, for banners and name colours.
export function setHue(node, seed) {
  node.style.setProperty('--hue', String(hueOf(seed)));
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

// ---- time ----

export function dayKey(ms) {
  const d = new Date(Number(ms));
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

export function dayLabel(ms) {
  return new Date(Number(ms)).toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' });
}

export function clockLabel(ms) {
  return new Date(Number(ms)).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
}

export function stampLabel(ms) {
  const d = new Date(Number(ms));
  return `${d.toLocaleDateString('en-US', { month: '2-digit', day: '2-digit', year: 'numeric' })} ${clockLabel(ms)}`;
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
export function openDialog({ title, content, onClose }) {
  const dialog = el('dialog', { class: 'dialog' });
  const close = () => dialog.close();
  dialog.append(
    el('header', { class: 'dialog-head' },
      el('h2', { text: title }),
      el('button', { class: 'icon-btn', 'aria-label': 'Close', icon: 'x', onclick: close })),
    el('div', { class: 'dialog-body' }, content),
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
