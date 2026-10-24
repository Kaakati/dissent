// A right-click menu at a point. Items: { label, icon, sub, danger, disabled, onPick, submenu: [...] } or 'sep'.
// Keyboard: Up and Down move, Right opens a submenu, Left closes it, Enter picks, Escape closes everything.
import { el, icon } from '../ui.js';

let open = null; // { root, opener }

export function closeContextMenu() {
  if (!open) return;
  const { root, opener } = open;
  open = null;
  root.remove();
  document.removeEventListener('pointerdown', onOutside, true);
  document.removeEventListener('keydown', onKey, true);
  if (opener?.isConnected) opener.focus();
}

function onOutside(e) {
  if (open && !open.root.contains(e.target)) closeContextMenu();
}

function onKey(e) {
  if (!open) return;
  const active = document.activeElement;
  const menu = active?.closest('.menu') ?? open.root;
  const items = [...menu.querySelectorAll(':scope > .menu-item:not([aria-disabled="true"])')];
  const i = items.indexOf(active);
  switch (e.key) {
    case 'Escape': e.preventDefault(); closeContextMenu(); return;
    case 'ArrowDown': e.preventDefault(); items[(i + 1) % items.length]?.focus(); return;
    case 'ArrowUp': e.preventDefault(); items[(i - 1 + items.length) % items.length]?.focus(); return;
    case 'Home': e.preventDefault(); items[0]?.focus(); return;
    case 'End': e.preventDefault(); items[items.length - 1]?.focus(); return;
    case 'ArrowRight': {
      const sub = active?.querySelector?.(':scope > .submenu');
      if (sub) { e.preventDefault(); sub.querySelector('.menu-item')?.focus(); }
      return;
    }
    case 'ArrowLeft': {
      const parent = menu.classList.contains('submenu') ? menu.parentElement : null;
      if (parent) { e.preventDefault(); parent.focus(); }
      return;
    }
    default:
  }
}

function build(items, isSub = false) {
  const menu = el('div', { class: `menu context${isSub ? ' submenu' : ''}`, role: 'menu' });
  for (const item of items) {
    if (item === 'sep') { menu.append(el('div', { class: 'menu-sep', role: 'separator' })); continue; }
    if (!item) continue;
    const node = el('button', {
      class: `menu-item${item.danger ? ' danger' : ''}${item.submenu ? ' has-sub' : ''}`,
      role: 'menuitem',
      'aria-disabled': item.disabled ? 'true' : null,
      'aria-haspopup': item.submenu ? 'menu' : null,
      tabindex: '-1',
    },
    item.icon ? icon(item.icon) : null,
    el('span', { class: 'menu-text' }, el('span', { text: item.label }), item.sub ? el('span', { class: 'menu-sub', text: item.sub }) : null),
    item.submenu ? el('span', { class: 'icon chev' }, icon('chevron-right')) : null);
    if (item.submenu) {
      const sub = build(item.submenu, true);
      sub.hidden = true;
      node.append(sub);
      const show = () => { sub.hidden = false; placeSub(node, sub); };
      node.addEventListener('pointerenter', show);
      node.addEventListener('focus', show);
      node.addEventListener('pointerleave', () => { sub.hidden = true; });
      node.addEventListener('click', (e) => { if (e.target === node || e.target.closest('.menu-item') === node) { e.stopPropagation(); show(); } });
    } else {
      node.addEventListener('click', (e) => {
        e.stopPropagation();
        if (item.disabled) return;
        closeContextMenu();
        item.onPick?.();
      });
    }
    menu.append(node);
  }
  return menu;
}

function placeSub(parent, sub) {
  sub.style.left = '';
  const r = sub.getBoundingClientRect();
  if (r.right > window.innerWidth - 8) sub.style.left = `${-r.width - 10}px`;
  if (r.bottom > window.innerHeight - 8) sub.style.top = `${window.innerHeight - 8 - r.bottom - 6}px`;
}

// Opens the menu with its top-left corner at (x, y), moved back inside the window when it would overflow.
export function openContextMenu(x, y, items) {
  closeContextMenu();
  const root = build(items);
  root.setAttribute('aria-label', 'Context menu');
  document.body.append(root);
  const r = root.getBoundingClientRect();
  root.style.left = `${Math.round(Math.max(8, Math.min(x, window.innerWidth - r.width - 8)))}px`;
  root.style.top = `${Math.round(Math.max(8, Math.min(y, window.innerHeight - r.height - 8)))}px`;
  open = { root, opener: document.activeElement };
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey, true);
  root.querySelector('.menu-item:not([aria-disabled="true"])')?.focus();
  return root;
}
