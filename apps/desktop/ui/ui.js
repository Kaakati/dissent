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

// A Lucide icon from the vendored sprite (icons.svg, built by scripts/build-icons.mjs).
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

const AVATAR_COLOURS = 6;

export function avClass(seed) {
  let h = 0;
  for (const ch of String(seed ?? '')) h = (h * 31 + ch.codePointAt(0)) >>> 0;
  return `av-${h % AVATAR_COLOURS}`;
}

export function initials(name) {
  const parts = String(name ?? '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  return parts.slice(0, 2).map((p) => [...p][0]).join('').toUpperCase();
}

export function avatar(name, seed) {
  return el('span', { class: `avatar ${avClass(seed || name)}`, text: initials(name) });
}

export function toast(err, kind = 'error') {
  const text = typeof err === 'string' ? err : (err?.message ?? String(err));
  const host = document.getElementById('toasts');
  const node = el('div', { class: `toast ${kind}`, role: 'status', icon: kind === 'info' ? 'info' : 'circle-alert', text });
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
  dialog.addEventListener('close', () => {
    dialog.remove();
    onClose?.();
  });
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, close };
}
