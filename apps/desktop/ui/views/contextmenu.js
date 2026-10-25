// A right-click menu at a point. Items: { label, icon, leading, sub, danger, disabled, onPick, submenu: [...] }, 'sep',
// or { header } for a non-item block (a node that the arrow keys skip).
// Keyboard: Up and Down move (closing any open submenu), Right opens a submenu, Left closes it, Enter picks, Escape
// closes everything. A submenu opens on hover, click, Enter or Right, not on focus alone.
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
    case 'ArrowDown': e.preventDefault(); moveTo(menu, items[(i + 1) % items.length]); return;
    case 'ArrowUp': e.preventDefault(); moveTo(menu, items[(i - 1 + items.length) % items.length]); return;
    case 'Home': e.preventDefault(); moveTo(menu, items[0]); return;
    case 'End': e.preventDefault(); moveTo(menu, items[items.length - 1]); return;
    case 'ArrowRight': {
      const sub = active?.querySelector?.(':scope > .submenu');
      if (sub) { e.preventDefault(); sub.hidden = false; placeSub(active, sub); sub.querySelector('.menu-item')?.focus(); }
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

// Focuses a row of this menu and shuts the submenus of its sibling rows.
function moveTo(menu, item) {
  for (const sub of menu.querySelectorAll(':scope > .menu-item > .submenu')) sub.hidden = true;
  item?.focus();
}

function build(items, isSub = false) {
  const menu = el('div', { class: `menu context${isSub ? ' submenu' : ''}`, role: 'menu' });
  for (const item of items) {
    if (item === 'sep') { menu.append(el('div', { class: 'menu-sep', role: 'separator' })); continue; }
    if (!item) continue;
    if (item.header) { menu.append(el('div', { class: 'menu-head', role: 'none' }, item.header)); continue; }
    const node = el('button', {
      class: `menu-item${item.danger ? ' danger' : ''}${item.submenu ? ' has-sub' : ''}`,
      role: 'menuitem',
      'aria-disabled': item.disabled ? 'true' : null,
      'aria-haspopup': item.submenu ? 'menu' : null,
      tabindex: '-1',
    },
    item.leading ?? (item.icon ? icon(item.icon) : null),
    el('span', { class: 'menu-text' }, el('span', { text: item.label }), item.sub ? el('span', { class: 'menu-sub', text: item.sub }) : null),
    item.submenu ? el('span', { class: 'icon chev' }, icon('chevron-right')) : null);
    if (item.submenu) {
      const sub = build(item.submenu, true);
      sub.hidden = true;
      node.append(sub);
      const show = () => { sub.hidden = false; placeSub(node, sub); };
      node.addEventListener('pointerenter', show);
      node.addEventListener('pointerleave', () => { sub.hidden = true; });
      node.addEventListener('click', (e) => { if (e.target === node || e.target.closest('.menu-item') === node) { e.stopPropagation(); show(); } });
    } else {
      node.addEventListener('click', (e) => {
        e.stopPropagation();
        if (item.disabled) return;
        closeContextMenu();
        item.onPick?.(e); // the click event, so a pick can read Shift (Delete message skips its confirm)
      });
    }
    menu.append(node);
  }
  return menu;
}

function placeSub(parent, sub) {
  sub.style.left = '';
  sub.style.top = '';
  sub.classList.remove('flip');
  const r = sub.getBoundingClientRect();
  if (r.right > window.innerWidth - 8) { sub.style.left = `${-r.width - 6}px`; sub.classList.add('flip'); }
  if (r.bottom > window.innerHeight - 8) sub.style.top = `${window.innerHeight - 8 - r.bottom - 6}px`;
}

// Opens the menu with its top-left corner at (x, y), moved back inside the window when it would overflow.
// placeAbove puts the bottom-left corner at (x, y) instead, for menus that open from a control at the window's foot.
export function openContextMenu(x, y, items, { placeAbove = false } = {}) {
  closeContextMenu();
  const root = build(items);
  root.setAttribute('aria-label', 'Context menu');
  document.body.append(root);
  const r = root.getBoundingClientRect();
  root.style.left = `${Math.round(Math.max(8, Math.min(x, window.innerWidth - r.width - 8)))}px`;
  const top = placeAbove ? y - r.height : y;
  root.style.top = `${Math.round(Math.max(8, Math.min(top, window.innerHeight - r.height - 8)))}px`;
  open = { root, opener: document.activeElement };
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey, true);
  root.querySelector('.menu-item:not([aria-disabled="true"])')?.focus();
  return root;
}
